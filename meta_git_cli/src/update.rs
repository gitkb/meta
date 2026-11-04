use crate::clone_worker::clone_with_queue;
use crate::ssh;
use colored::Colorize;
use indicatif::MultiProgress;
use meta_core::config;
use meta_core::display::{escape, escape_path};
use meta_core::git::GitUrl;
use meta_core::gitdir::Checkout;
use meta_core::{errln, outln};
use meta_git_lib::clone_queue::{CloneQueue, CloneTask, MetaRepo, Work};
use meta_plugin_protocol::{CommandResult, PluginRequestOptions};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
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
                  upstream, is named and left alone. With meta's -r (`meta -r git update`) the same goes for the meta repos \
                  nested in it, each pulled before its .meta.yaml is read.\n\n\
                  To clone the missing projects and touch nothing else, run \
                  `meta git clone-missing`.",
    after_help = "Git options: none of git pull's; each repo is pulled with `git pull --ff-only`."
)]
pub(crate) struct UpdateArgs {}

/// The workspace at or above `cwd` that `meta <command>` acts on. Refused
/// inside a worktree set meta made: the workspace around the set is not
/// the command's to change from in it (`in_set`: what to run there
/// instead). A plain git worktree of a meta repo under .worktrees is a
/// workspace of its own: one that meta didn't make, whose .meta.yaml is its
/// own.
pub(crate) fn workspace(
    cwd: &Path,
    command: &str,
    in_set: &str,
) -> anyhow::Result<config::Workspace> {
    let Some(workspace) = config::Workspace::find(cwd, None)? else {
        anyhow::bail!("No .meta.yaml found in {} or above it", escape_path(cwd));
    };
    if let Some((set, set_dir)) = meta_cli::worktree::detect_worktree_location(cwd)? {
        let real = |dir: &Path| dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        if !real(&workspace.dir).starts_with(real(&set_dir))
            || meta_cli::worktree::holds_meta_checkout(&set_dir)
        {
            anyhow::bail!(
                "`meta {command}` acts on the workspace, not worktree set '{}'; run it in the \
                 workspace{in_set}",
                escape(&set)
            );
        }
    }
    Ok(workspace)
}

/// The name the workspace at `root` goes by in progress and errors.
pub(crate) fn root_name(root: &Path) -> String {
    root.file_name()
        .map_or_else(|| ".".to_string(), |n| n.to_string_lossy().into_owned())
}

/// Clone and pull `tasks`, and the tasks their `.meta.yaml` files add, four
/// at a time, over SSH masters set up for `urls` (the remotes the run
/// contacts first).
pub(crate) fn run(
    queue: &CloneQueue,
    tasks: Vec<CloneTask>,
    urls: &[GitUrl],
) -> anyhow::Result<()> {
    let mut parallel = 4_usize;
    let ssh_cmd = match ssh::establish_ssh_masters(urls) {
        ssh::SshMasters::OurSockets(dir) => Some(ssh::git_ssh_command(&dir)),
        ssh::SshMasters::UserManaged => None, // parallel OK
        ssh::SshMasters::Failed => {
            log::debug!(
                "SSH multiplexing could not be set up: working on one repository at a time"
            );
            parallel = 1;
            None
        }
    };
    clone_with_queue(
        queue,
        tasks,
        parallel,
        &MultiProgress::new(),
        ssh_cmd.as_deref(),
    )
}

pub(crate) fn execute_git_update(
    UpdateArgs {}: UpdateArgs,
    options: &PluginRequestOptions,
    cwd: &Path,
) -> anyhow::Result<CommandResult> {
    let recursive = options.recursive;
    // Without -r only the workspace's own .meta.yaml is read
    let meta_depth = if recursive { options.depth } else { Some(0) };
    // A worktree set's checkouts are pulled with `meta git pull`
    let workspace = workspace(cwd, "git update", ", or 'meta git pull' in the set")?;
    ssh::warn_remote_mismatches(&workspace);

    let queue = CloneQueue::new(None, meta_depth, true); // No git depth for update
    let root = workspace.dir.as_path();
    let name = root_name(root);
    let top = || Arc::from([MetaRepo::at(&name, root, None)]);
    // The workspace repo first: its .meta.yaml is read once it is pulled
    let tasks = match Checkout::of(root) {
        Checkout::Repo { .. } => {
            let url = meta_core::git::remote_url(root)?;
            vec![queue.root(&name, root, url, Work::Pull)]
        }
        _ => queue.discover_in(&workspace, 0, &top())?,
    };

    if options.mode().is_preview() {
        // The .meta.yaml files as they are now: nothing is pulled, so what
        // the pulls would add isn't known
        outln!(
            "{} Would run (from the .meta.yaml files as they are now; the pulls may add projects):",
            "[DRY RUN]".cyan()
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
        report_unlisted(&queue, root, cwd, recursive);
        if let Some(error) = crate::clone_worker::report_failures(&queue, "git update") {
            return Ok(CommandResult::Error(error));
        }
        return Ok(CommandResult::Message(String::new()));
    }

    // Progress counts to the repos the .meta.yaml files on disk list, the
    // workspace's own first; pulls that add projects add to it
    let on_disk = CloneQueue::new(None, meta_depth, false);
    if on_disk.discover_in(&workspace, 0, &top()).is_ok() {
        let own = usize::from(matches!(Checkout::of(root), Checkout::Repo { .. }));
        queue.expect(on_disk.found() + own);
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
    run(&queue, tasks, &urls)?;
    report_unlisted(&queue, root, cwd, recursive);
    let outcome = queue.outcome();
    for (name, why) in &outcome.not_pulled {
        errln!("{} {}: {why}", "Not pulled".yellow(), escape(name));
    }
    if let Some(error) = crate::clone_worker::report_failures(&queue, "git update") {
        return Ok(CommandResult::Error(error));
    }
    let (pulled, cloned) = (outcome.pulled, outcome.cloned.len());
    let not_pulled = match outcome.not_pulled.len() {
        0 => String::new(),
        n => format!(", {n} with nothing to pull from"),
    };
    outln!("Update completed ({pulled} pulled, {cloned} cloned{not_pulled})")?;
    Ok(CommandResult::Message(String::new()))
}

/// What deleting a repository no `.meta.yaml` lists would lose.
pub(crate) enum Leftover {
    /// Nothing git shows that isn't on a remote, maybe with files git
    /// ignores ("ignored files: .env, target/"). Those are deleted with it,
    /// as `git worktree remove` deletes them: ignoring a file says it isn't
    /// work. They are named, so none goes unseen.
    Removable(Option<String>),
    /// What it holds, or why it isn't deleted
    Holds(String),
}

impl Leftover {
    /// The repository at `path`, a leftover in the workspace at `root`.
    pub(crate) fn of(path: &Path, root: &Path) -> Self {
        let real = |dir: &Path| dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        let inside = real(path);
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Self::Holds("it is a symbolic link".to_string());
        }
        if !inside.starts_with(real(root)) || inside == real(root) {
            return Self::Holds(format!(
                "it is outside the workspace ({})",
                escape_path(&inside)
            ));
        }
        // Deleting the folder must delete its repository, not leave it
        if let Checkout::Repo { git_dir } = Checkout::of(path) {
            if !real(&path.join(&git_dir)).starts_with(&inside) {
                return Self::Holds(format!(
                    "its git directory is elsewhere ({})",
                    escape_path(&git_dir)
                ));
            }
        }
        // git sees a repo inside it as untracked, or not at all if ignored
        match nested_repo(path) {
            Ok(Some(inner)) => {
                return Self::Holds(format!(
                    "it holds another git repository: {}",
                    escape_path(inner.strip_prefix(path).unwrap_or(&inner))
                ))
            }
            Ok(None) => {}
            Err(e) => return Self::Holds(format!("it can't be searched ({e})")),
        }
        let held = meta_git_lib::worktree::safety::repo_holds(path)
            .and_then(|held| Ok((held, meta_git_lib::worktree::safety::repo_ignored(path)?)));
        match held {
            Ok((held, ignored)) if held.is_empty() => Self::Removable(ignored),
            Ok((held, ignored)) => Self::Holds(format!(
                "it holds {}",
                held.into_iter()
                    .chain(ignored)
                    .collect::<Vec<_>>()
                    .join("; ")
            )),
            Err(e) => Self::Holds(format!(
                "what it holds can't be told ({})",
                meta_core::display::escape_lines(&format!("{e:#}"))
            )),
        }
    }
}

/// The first git repository inside the folder `dir` (not `dir` itself): a
/// folder with a `.git`, or a bare one (`HEAD`, `objects` and `refs`);
/// links aren't followed.
fn nested_repo(dir: &Path) -> std::io::Result<Option<PathBuf>> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_name() == ".git" || !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path();
        let bare = ["HEAD", "objects", "refs"]
            .iter()
            .all(|part| path.join(part).symlink_metadata().is_ok());
        if bare || path.join(".git").symlink_metadata().is_ok() {
            return Ok(Some(path));
        }
        if let Some(inner) = nested_repo(&path)? {
            return Ok(Some(inner));
        }
    }
    Ok(None)
}

/// `path` as shown: from `cwd` where it is inside it.
pub(crate) fn shown(path: &Path, cwd: &Path) -> String {
    escape_path(path.strip_prefix(cwd).unwrap_or(path))
}

/// Name each repository in a meta repo's folder that no `.meta.yaml` lists,
/// with what deleting it would lose (`root`: the workspace's folder; `-r`:
/// whether nested meta repos were read).
fn report_unlisted(queue: &CloneQueue, root: &Path, cwd: &Path, recursive: bool) {
    let prune = if recursive {
        "meta -r git clone-prune"
    } else {
        "meta git clone-prune"
    };
    for path in queue.outcome().unlisted {
        let shown = shown(&path, cwd);
        let warning = match Leftover::of(&path, root) {
            Leftover::Removable(None) => {
                format!("{shown} isn't in .meta.yaml (nothing unpushed; {prune} removes it)")
            }
            Leftover::Removable(Some(ignored)) => format!(
                "{shown} isn't in .meta.yaml (nothing unpushed; {prune} removes it, and its \
                 {ignored})"
            ),
            Leftover::Holds(why) => format!("{shown} isn't in .meta.yaml; {why}"),
        };
        meta_core::out::warn(warning);
    }
}
