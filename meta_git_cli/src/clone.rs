use crate::clone_queue::{CloneQueue, MetaRepo, Work};
use crate::clone_worker::clone_with_queue;
use crate::ssh;
use anyhow::Context;
use colored::Colorize;
use indicatif::MultiProgress;
use meta_plugin::RepoUrl;
use meta_plugin::{errln, outln};
use meta_plugin::{Ctx, Request, Response};
use std::sync::Arc;

/// Clone a meta repository, then the repos its .meta.yaml lists.
#[derive(clap::Parser)]
#[command(
    after_help = "Git options: of git clone's own, only --depth, given to each clone. \
                         meta's options go before the command: -r also clones the repos of the \
                         meta repos nested in it, --depth N (before) follows N levels of them."
)]
pub(crate) struct CloneArgs {
    /// The meta repository to clone
    url: RepoUrl,
    /// Where to clone it (default: the repository's name)
    dir: Option<String>,
    /// How many repos to clone at once
    #[arg(long, value_name = "N", default_value = "4")]
    parallel: std::num::NonZeroUsize,
    /// Clone only this many commits of each repo (git clone --depth)
    #[arg(long, value_name = "N")]
    depth: Option<usize>,
}

pub(crate) fn execute_git_clone(
    args: CloneArgs,
    ctx: &Ctx,
    request: &Request,
) -> anyhow::Result<Response> {
    let cwd = request.cwd.as_path();
    // meta's -r and --depth (how deep into nested meta repos); the
    // command's --depth is git's history depth. A new workspace is its own
    // root: -A is -r
    let meta_depth = request.depth;
    let recursive = meta_depth != Some(0);
    let depth = args.depth.map(|d| d.to_string());
    let mut parallel = args.parallel.get();
    let url = args.url;
    let git_clone_args: Vec<String> = depth
        .iter()
        .flat_map(|d| ["--depth".to_string(), d.clone()])
        .collect();

    // The directory git would name: a bundle file's name without
    // `.bundle`, else the repository's name (a `.git` file is a checkout's)
    let clone_dir = args.dir.unwrap_or_else(|| {
        let source = cwd.join(url.as_git_arg());
        let gitfile = || {
            let mut head = [0; 8];
            std::fs::File::open(&source)
                .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut head))
                .is_ok_and(|()| &head == b"gitdir: ")
        };
        match source
            .file_name()
            .filter(|_| source.is_file() && !gitfile())
        {
            Some(bundle) => {
                let bundle = bundle.to_string_lossy();
                // git takes what follows a `:` as the name, as for a URL
                let bundle = bundle.rsplit_once(':').map_or(&*bundle, |(_, name)| name);
                bundle.strip_suffix(".bundle").unwrap_or(bundle).to_string()
            }
            None => url.name().unwrap_or("meta").to_string(),
        }
    });

    // The clone, with the URL as git gets it or as meta shows it: after
    // `--`, so it can't be read as an option, and the directory meta reads
    // next named explicitly
    let clone_argv = |url: String| -> Vec<String> {
        ["git", "clone"]
            .into_iter()
            .map(str::to_string)
            .chain(git_clone_args.iter().cloned())
            .chain(["--".to_string(), url, clone_dir.clone()])
            .collect()
    };

    // git refuses a destination that exists and isn't an empty directory;
    // said here, before anything runs, with what to do instead
    let clone_dir_path = cwd.join(&clone_dir);
    let inspect = || {
        format!(
            "Cannot inspect {}",
            meta_plugin::display::escape_path(&clone_dir_path)
        )
    };
    // (a link is what it leads to, as git sees it; one that leads nowhere
    // is there all the same)
    let occupied = match std::fs::metadata(&clone_dir_path) {
        Ok(meta) if meta.is_dir() => std::fs::read_dir(&clone_dir_path)
            .with_context(inspect)?
            .next()
            .is_some(),
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            clone_dir_path.symlink_metadata().is_ok()
        }
        Err(e) => return Err(e).with_context(inspect),
    };
    if occupied {
        let workspace = clone_dir_path.join(".meta.yaml").is_file();
        anyhow::bail!(
            "{} already exists{}",
            meta_plugin::display::escape_path(&clone_dir_path),
            if workspace {
                "; to clone the projects its .meta.yaml lists that aren't cloned yet (and \
                 fast-forward the others), run 'meta git update' in it"
            } else {
                " and is not an empty directory; clone somewhere else, or name another \
                 directory"
            }
        );
    }

    if ctx.dry_run() {
        // Output what we know - just the meta repo clone command
        // (Child repos are in .meta.yaml file which hasn't been cloned yet)
        outln!("{} Would clone meta repository:", "[DRY RUN]".cyan())?;
        let shown = meta_plugin::display::display_command(&clone_argv(url.to_string()));
        outln!("  {}", meta_plugin::display::escape(&shown))?;
        return Ok(Response::done());
    }

    // Set up SSH ControlMaster for the meta repo's host before cloning
    let mut ssh_cmd = match ssh::establish_ssh_masters(std::slice::from_ref(&url)) {
        ssh::SshMasters::OurSockets(dir) => Some(ssh::git_ssh_command(&dir)),
        ssh::SshMasters::UserManaged | ssh::SshMasters::Failed => None,
    };

    // progress, on stderr beside the clones' own: meta shows a plugin's
    // stdout only once it ends
    errln!("Cloning meta repository: {url}");
    let mut clone_cmd = meta_core::git::git(cwd);
    clone_cmd.arg("clone");
    clone_cmd.args(
        clone_argv(url.as_git_arg().to_string_lossy().into_owned())
            .iter()
            .skip(2),
    );
    if let Some(ref ssh) = ssh_cmd {
        clone_cmd.env("GIT_SSH_COMMAND", ssh);
    }
    // a child meta owns: stopping meta stops the clone. Under --json git's
    // stderr is kept for the row of the failure; in text the user sees it
    if ctx.meta().json {
        let out = meta_plugin::process::output(&mut clone_cmd)?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            let row = meta_plugin::ProjectResult {
                stderr: Some(stderr),
                ..meta_plugin::ProjectResult::error(
                    ".",
                    meta_plugin::ErrorInfo::new(
                        meta_plugin::ErrorCode::ToolFailed,
                        format!("Failed to clone meta repository ({})", out.status),
                    ),
                )
                .at(&clone_dir_path)
            };
            return crate::printed(
                ctx,
                vec![row],
                Some(anyhow::anyhow!("Failed to clone meta repository")),
            );
        }
    } else {
        let status = meta_plugin::process::status(&mut clone_cmd)?;
        anyhow::ensure!(status.success(), "Failed to clone meta repository");
    }

    // The projects the clone's config lists, as meta reads them
    let Some(listing) = crate::here::Listing::of(&clone_dir_path, None)? else {
        return Ok(Response::message(
            "No .meta.yaml found in cloned repository",
        ));
    };

    // For non-recursive mode, read only the cloned repo's own .meta.yaml
    let queue = CloneQueue::new(depth.clone(), meta_depth, false);
    let name = clone_dir_path
        .file_name()
        .map_or_else(|| clone_dir.clone(), |n| n.to_string_lossy().into_owned());
    // (its origin as git recorded it: a relative URL resolved)
    let top = MetaRepo::at(&name, &clone_dir_path, Some(url.clone()));
    let tasks = queue.discover_in(&listing, 0, &Arc::from([top]))?;
    let initial_count = tasks.len();

    if initial_count == 0 {
        if let Some(error) = crate::clone_worker::report_failures(&queue, "git clone-missing") {
            anyhow::bail!("{error}");
        }
        // only ones whose URL holds credentials: through meta, as configured
        if let Some(rest) = crate::update::deferred(ctx, request, &clone_dir_path, &queue, true)? {
            return Ok(rest);
        }
        return Ok(Response::message("No child repositories to clone"));
    }

    // Establish SSH masters for any additional hosts in the queue
    let queue_urls: Vec<_> = tasks
        .iter()
        .filter_map(|t| match &t.work {
            Work::Clone(url) => Some(url.clone()),
            Work::Pull => None,
        })
        .collect();
    if !queue_urls.is_empty() {
        match ssh::establish_ssh_masters(&queue_urls) {
            ssh::SshMasters::OurSockets(sockets_dir) => {
                ssh_cmd = Some(ssh::git_ssh_command(&sockets_dir));
            }
            ssh::SshMasters::UserManaged => {} // parallel OK, no override needed
            ssh::SshMasters::Failed if ssh_cmd.is_none() => {
                log::debug!(
                    "SSH multiplexing could not be set up: cloning one repository at a time"
                );
                parallel = 1;
            }
            ssh::SshMasters::Failed => {} // initial host works
        }
    }

    errln!(
        "Cloning {} child repositories{}",
        initial_count,
        if recursive { " (recursive mode)" } else { "" }
    );

    let mp = MultiProgress::new();

    clone_with_queue(&queue, tasks, parallel, &mp, ssh_cmd.as_deref())?;
    if let Some(error) = crate::clone_worker::report_failures(&queue, "git clone-missing") {
        let rows = crate::clone_worker::failure_rows(&queue, &clone_dir_path);
        return crate::printed(ctx, rows, Some(anyhow::anyhow!("{error}")));
    }

    // the projects whose URL holds credentials: through meta, as configured
    if let Some(rest) = crate::update::deferred(ctx, request, &clone_dir_path, &queue, true)? {
        outln!("Meta-repo clone completed; cloning the rest")?;
        return Ok(rest);
    }

    let (completed, total) = queue.get_counts();
    if total > initial_count {
        outln!(
            "Meta-repo clone completed ({} repos cloned, {} discovered via nested .meta.yaml files)",
            completed,
            total - initial_count
        )?;
    } else {
        outln!("Meta-repo clone completed ({completed} repos cloned)")?;
    }

    Ok(Response::done())
}
