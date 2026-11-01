use crate::clone_worker::clone_with_queue;
use crate::ssh;
use console::style;
use indicatif::MultiProgress;
use meta_core::config;
use meta_core::display::{escape, escape_path};
use meta_core::git::GitUrl;
use meta_core::gitdir::Checkout;
use meta_core::{errln, outln};
use meta_git_lib::clone_queue::{CloneQueue, MetaRepo, Work};
use meta_plugin_protocol::{CommandResult, PluginRequestOptions};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;

/// `meta git update`
#[derive(clap::Parser)]
#[command(
    about = "Bring the workspace up to date: fast-forward each repo that is cloned (git pull --ff-only) and clone each project in .meta.yaml that isn't",
    long_about = "Bring the workspace up to date: fast-forward each repo that is cloned \
                  (git pull --ff-only) and clone each project in .meta.yaml that isn't.\n\n\
                  The workspace repo is pulled first, so projects added to its .meta.yaml \
                  upstream are cloned in the same run. A repo that can't be pulled \
                  (fast-forward only) is named with git's error and the command exits 1; \
                  nothing is merged or rebased. A repo on no branch, or whose branch has no \
                  upstream, is named and left alone. With -r the same goes for the meta repos \
                  nested in it, each pulled before its .meta.yaml is read."
)]
pub(crate) struct UpdateArgs {
    /// Also update the projects of nested meta repos
    #[arg(short, long)]
    recursive: bool,
}

pub(crate) fn execute_git_update(
    args: UpdateArgs,
    options: &PluginRequestOptions,
    cwd: &Path,
) -> anyhow::Result<CommandResult> {
    let recursive = args.recursive || options.recursive;
    // Without -r only the workspace's own .meta.yaml is read
    let meta_depth = if recursive { options.depth } else { Some(0) };
    let Some(workspace) = config::Workspace::find(cwd, None)? else {
        anyhow::bail!("No .meta.yaml found in {} or above it", escape_path(cwd));
    };
    // A worktree set's checkouts are pulled with `meta git pull`; the
    // workspace around the set is not this command's to change from in it
    // (A plain git worktree of a meta repo under .worktrees is a workspace
    // of its own: one that meta didn't make, whose .meta.yaml is its own.)
    if let Some((set, set_dir)) = meta_cli::worktree::detect_worktree_location(cwd)? {
        let real = |dir: &Path| dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        if !real(&workspace.dir).starts_with(real(&set_dir))
            || meta_cli::worktree::holds_meta_checkout(&set_dir)
        {
            anyhow::bail!(
                "meta git update brings the workspace up to date, not worktree set '{}'; \
                 run it in the workspace, or 'meta git pull' in the set",
                escape(&set)
            );
        }
    }
    ssh::warn_remote_mismatches(&workspace);

    let queue = CloneQueue::new(None, meta_depth, true); // No git depth for update
    let root = workspace.dir.as_path();
    let name = root
        .file_name()
        .map_or_else(|| ".".to_string(), |n| n.to_string_lossy().into_owned());
    // The workspace repo first: its .meta.yaml is read once it is pulled
    let tasks = match Checkout::of(root) {
        Checkout::Repo { .. } => {
            let url = meta_core::git::remote_url(root)?;
            vec![queue.root(&name, root, url, Work::Pull)]
        }
        _ => {
            let top = MetaRepo::at(&name, root, None);
            queue.discover_in(&workspace, 0, &Arc::from([top]))?
        }
    };

    if options.mode().is_preview() {
        // The .meta.yaml files as they are now: nothing is pulled, so what
        // the pulls would add isn't known
        outln!(
            "{} Would run (from the .meta.yaml files as they are now; the pulls may add projects):",
            style("[DRY RUN]").cyan()
        )?;
        let mut tasks = VecDeque::from(tasks);
        while let Some(task) = tasks.pop_front() {
            let argv = crate::clone_worker::git_argv(&task, None, |url| url.to_string().into());
            let argv: Vec<String> = argv.iter().map(|a| a.to_string_lossy().into()).collect();
            let shown = escape(&loop_lib::display_command(&argv)).into_owned();
            match task.work {
                Work::Clone(_) => outln!("  {shown}")?,
                Work::Pull => match crate::clone_worker::nothing_to_pull(&task.target_path) {
                    Some(why) => {
                        outln!("  ({} not pulled: {why})", escape(&task.name))?;
                        tasks.extend(queue.mark_not_pulled(&task, why));
                    }
                    None => {
                        outln!("  {shown}")?;
                        tasks.extend(queue.mark_completed(&task));
                    }
                },
            }
        }
        report_unlisted(&queue, cwd);
        if let Some(error) = crate::clone_worker::report_failures(&queue) {
            return Ok(CommandResult::Error(error));
        }
        return Ok(CommandResult::Message(String::new()));
    }

    // Establish SSH multiplexing before parallel clones and pulls, for the
    // remotes this run contacts: a checkout's own origin (what it pulls
    // from, if it has something to pull from), the listed URL of a missing
    // project
    let contacted = |dir: &Path, listed: Option<&GitUrl>| match Checkout::of(dir) {
        Checkout::Repo { .. } if crate::clone_worker::nothing_to_pull(dir).is_none() => {
            meta_core::git::remote_url(dir).ok().flatten()
        }
        Checkout::Repo { .. } => None,
        // git clones into an empty folder too
        Checkout::Missing => listed.cloned(),
        Checkout::NotARepo(_) if meta_git_lib::clone_queue::is_empty_dir(dir) => listed.cloned(),
        Checkout::NotARepo(_) => None,
    };
    let projects = workspace.projects.iter();
    let urls: Vec<_> = std::iter::once(contacted(root, None))
        .chain(projects.map(|p| contacted(&root.join(&p.path), p.repo.as_ref())))
        .flatten()
        .collect();
    let mut parallel = 4_usize;
    let ssh_cmd = match ssh::establish_ssh_masters(&urls) {
        ssh::SshMasters::OurSockets(dir) => Some(ssh::git_ssh_command(&dir)),
        ssh::SshMasters::UserManaged => None, // parallel OK
        ssh::SshMasters::Failed => {
            meta_core::out::warn(
                "SSH multiplexing could not be set up: updating one repository at a time",
            );
            parallel = 1;
            None
        }
    };

    let mp = MultiProgress::new();
    clone_with_queue(&queue, tasks, parallel, &mp, ssh_cmd.as_deref())?;
    report_unlisted(&queue, cwd);
    let outcome = queue.outcome();
    for (name, why) in &outcome.not_pulled {
        errln!("{} {}: {why}", style("Not pulled").yellow(), escape(name));
    }
    if let Some(error) = crate::clone_worker::report_failures(&queue) {
        return Ok(CommandResult::Error(error));
    }
    let (pulled, cloned) = (outcome.pulled, outcome.cloned);
    let not_pulled = match outcome.not_pulled.len() {
        0 => String::new(),
        n => format!(", {n} with nothing to pull from"),
    };
    outln!("Update completed ({pulled} pulled, {cloned} cloned{not_pulled})")?;
    Ok(CommandResult::Message(String::new()))
}

/// Name each repository in a meta repo's folder that its `.meta.yaml`
/// doesn't list, with what deleting it would lose.
fn report_unlisted(queue: &CloneQueue, cwd: &Path) {
    for path in queue.outcome().unlisted {
        let shown = escape_path(path.strip_prefix(cwd).unwrap_or(&path));
        let holds = match meta_git_lib::worktree::safety::repo_holds(&path) {
            Ok(held) if held.is_empty() => "it has no uncommitted or untracked files, stashes, \
                 worktrees, or commits on a branch, tag or HEAD that its remote branches lack \
                 (ignored files and the reflog not checked)"
                .to_string(),
            Ok(held) => format!(
                "it holds {} (ignored files and the reflog not checked)",
                held.join("; ")
            ),
            Err(e) => format!(
                "meta can't tell what it holds ({})",
                escape(&format!("{e:#}"))
            ),
        };
        meta_core::out::warn(format!("{shown} is not in .meta.yaml; {holds}"));
    }
}
