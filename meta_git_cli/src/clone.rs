use crate::clone_worker::clone_with_queue;
use crate::ssh;
use anyhow::Context;
use console::style;
use indicatif::MultiProgress;
use meta_core::config;
use meta_core::git::GitUrl;
use meta_git_lib::clone_queue::CloneQueue;
use meta_plugin_protocol::{CommandResult, PluginRequestOptions};
use std::sync::Arc;

/// Clone a meta repository, then the repos its .meta.yaml lists.
#[derive(clap::Parser)]
pub(crate) struct CloneArgs {
    /// The meta repository to clone
    url: GitUrl,
    /// Where to clone it (default: the repository's name)
    dir: Option<String>,
    /// Also clone the repos of the meta repos nested in it
    #[arg(short, long)]
    recursive: bool,
    /// How many levels of nested meta repos to follow with --recursive
    #[arg(long, value_name = "N")]
    meta_depth: Option<usize>,
    /// How many repos to clone at once
    #[arg(long, value_name = "N", default_value = "4")]
    parallel: std::num::NonZeroUsize,
    /// Clone only this many commits of each repo (git clone --depth)
    #[arg(long, value_name = "N")]
    depth: Option<usize>,
}

pub(crate) fn execute_git_clone(
    args: CloneArgs,
    options: &PluginRequestOptions,
    cwd: &std::path::Path,
) -> anyhow::Result<CommandResult> {
    // meta's own --recursive and --depth, taken before the plugin ran
    let recursive = args.recursive || options.recursive;
    let depth = args.depth.or(options.depth).map(|d| d.to_string());
    let meta_depth = args.meta_depth;
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
            meta_core::display::escape_path(&clone_dir_path)
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
        let workspace = matches!(config::find_meta_config_in(&clone_dir_path), Ok(Some(_)));
        return Ok(CommandResult::Error(format!(
            "{} already exists{}",
            meta_core::display::escape_path(&clone_dir_path),
            if workspace {
                "; to clone the projects its .meta.yaml lists that aren't cloned yet, run \
                 'meta git update' in it"
            } else {
                " and is not an empty directory; clone somewhere else, or name another \
                 directory"
            }
        )));
    }

    if options.mode().is_preview() {
        // Output what we know - just the meta repo clone command
        // (Child repos are in .meta.yaml file which hasn't been cloned yet)
        println!("{} Would clone meta repository:", style("[DRY RUN]").cyan());
        let shown = loop_lib::display_command(&clone_argv(url.to_string()));
        println!("  {}", meta_core::display::escape(&shown));
        return Ok(CommandResult::Message(String::new()));
    }

    // Set up SSH ControlMaster for the meta repo's host before cloning
    let mut ssh_cmd = match ssh::establish_ssh_masters(std::slice::from_ref(&url)) {
        ssh::SshMasters::OurSockets(dir) => Some(ssh::git_ssh_command(&dir)),
        ssh::SshMasters::UserManaged | ssh::SshMasters::Failed => None,
    };

    println!("Cloning meta repository: {url}");
    let mut clone_cmd = meta_core::git::git(cwd);
    clone_cmd.args(
        clone_argv(url.as_git_arg().to_string_lossy().into_owned())
            .iter()
            .skip(1),
    );
    if let Some(ref ssh) = ssh_cmd {
        clone_cmd.env("GIT_SSH_COMMAND", ssh);
    }
    let status = clone_cmd.status()?;
    if !status.success() {
        return Ok(CommandResult::Error(
            "Failed to clone meta repository".to_string(),
        ));
    }

    // Parse meta config inside cloned repo
    if config::find_meta_config_in(&clone_dir_path)?.is_none() {
        return Ok(CommandResult::Message(
            "No .meta.yaml found in cloned repository".to_string(),
        ));
    }

    // Create the clone queue with depth settings
    // For non-recursive mode, set meta_depth to 0 (only first level)
    let effective_meta_depth = if recursive { meta_depth } else { Some(0) };
    let queue = Arc::new(CloneQueue::new(depth.clone(), effective_meta_depth));

    // Seed the queue with first-level children
    let initial_count = queue.push_from_meta(&clone_dir_path, 0)?;

    if initial_count == 0 {
        if let Some(error) = crate::clone_worker::report_failures(&queue) {
            return Ok(CommandResult::Error(error));
        }
        return Ok(CommandResult::Message(
            "No child repositories to clone".to_string(),
        ));
    }

    // Establish SSH masters for any additional hosts in the queue
    let queue_urls = queue.peek_urls();
    if !queue_urls.is_empty() {
        match ssh::establish_ssh_masters(&queue_urls) {
            ssh::SshMasters::OurSockets(sockets_dir) => {
                ssh_cmd = Some(ssh::git_ssh_command(&sockets_dir));
            }
            ssh::SshMasters::UserManaged => {} // parallel OK, no override needed
            ssh::SshMasters::Failed if ssh_cmd.is_none() => {
                ssh::warn("SSH multiplexing could not be set up: cloning one repository at a time");
                parallel = 1;
            }
            ssh::SshMasters::Failed => {} // initial host works
        }
    }

    println!(
        "Cloning {} child repositories{}",
        initial_count,
        if recursive { " (recursive mode)" } else { "" }
    );

    let mp = MultiProgress::new();

    // Use the queue-based cloning system
    clone_with_queue(Arc::clone(&queue), parallel, &mp, ssh_cmd.as_deref())?;
    if let Some(error) = crate::clone_worker::report_failures(&queue) {
        return Ok(CommandResult::Error(error));
    }

    let (completed, total) = queue.get_counts();
    if total > initial_count {
        println!(
            "Meta-repo clone completed ({} repos cloned, {} discovered via nested .meta.yaml files)",
            completed,
            total - initial_count
        );
    } else {
        println!("Meta-repo clone completed ({completed} repos cloned)");
    }

    Ok(CommandResult::Message(String::new()))
}
