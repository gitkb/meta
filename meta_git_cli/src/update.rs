//! `meta git update`: clone the projects the `.meta.yaml` files list that
//! aren't there and, with `--prune` (or instead, `--prune-only`), remove the
//! repos there that none lists,
//! with the `.meta.yaml` files as they are on disk. It never pulls. And what
//! `meta git clone` and `worktree create` share with it.

use crate::clone_queue::{CloneQueue, CloneTask, MetaRepo};
use crate::clone_worker::clone_with_queue;
use crate::here::{Here, Listing};
use crate::ssh;
use colored::Colorize;
use indicatif::MultiProgress;
use meta_core::gitdir::Checkout;
use meta_plugin::display::{escape, escape_path};
use meta_plugin::outln;
use meta_plugin::RepoUrl;
use meta_plugin::{Ctx, ErrorCode, ErrorInfo, ProjectResult, Request, Response};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `meta git update`
#[derive(clap::Parser)]
#[command(
    after_help = "Git options: none; git has no command of this name.",
    about = "Clone each project in .meta.yaml that isn't cloned; never pull (meta git pull --ff-only does)",
    long_about = "Clone each project in .meta.yaml that isn't cloned, and name each repo in the \
                  workspace's folder that none lists, with what deleting it would lose.\n\n\
                  No repo that is cloned is pulled, fetched or checked out, the workspace repo \
                  included: `meta git pull --ff-only` brings them up to date. With -r the same \
                  goes for the meta repos nested in it (those cloned, and those it clones). \
                  --include, --exclude and --tag choose the projects it clones, and the meta \
                  repos --prune and --prune-only search; projects the new clones list are cloned only without \
                  them.\n\n\
                  With --prune, then, or --prune-only, instead of cloning, a repo no .meta.yaml \
                  lists is removed only when git shows \
                  nothing in it that isn't on a remote: no uncommitted or untracked files, \
                  stashes, commits on a branch or tag that no remote has, or linked worktrees, \
                  and no git repo inside it. A repo that holds anything is kept and named with \
                  what it holds. Files git ignores (.env, target/) are deleted with it, and \
                  named first; commits only a reflog holds are not kept, as git counts them \
                  discarded. Folders that are no git repo of their own, links, and the projects \
                  a .meta.yaml lists are never removed."
)]
pub(crate) struct UpdateArgs {
    /// Then remove each repo no .meta.yaml lists that holds nothing unpushed
    #[arg(long)]
    prune: bool,
    /// Only remove them: clone nothing
    #[arg(long, conflicts_with = "prune")]
    prune_only: bool,
}

/// What became of one repo, for `--json`: the record of the repo at its
/// path from the workspace root.
#[derive(serde::Serialize)]
struct Record {
    /// cloned, removed, kept or failed (with --dry-run: what would be)
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// `meta git update` (meta's `-r`: nested meta repos too) in the workspace
/// `request` runs in, on the selected projects when meta's options filter
/// them.
pub(crate) fn execute(
    UpdateArgs { prune, prune_only }: UpdateArgs,
    ctx: &Ctx,
    request: &Request,
) -> anyhow::Result<Response> {
    let cwd = request.cwd.as_path();
    // Without -r only the workspace's own .meta.yaml is read
    let meta_depth = request.depth;
    let workspace = workspace(request, "git update", "")?;
    // the workspace's own projects as --config lists them, and those its
    // default config lists counted as listed too. Read from disk, as the
    // nested ones are: in a plain git worktree of the workspace, meta's
    // members are the root alone
    let config = request.workspace()?.config.clone();
    // (with -A, the config is the nearest one, not the outermost's)
    let own = config.parent() == Some(workspace.dir.as_path());
    let workspace =
        Listing::of(&workspace.dir, own.then_some(config.as_path()))?.unwrap_or(workspace);
    let root = workspace.dir.as_path();
    if let Some(mismatches) = ssh::remote_mismatches(&workspace) {
        ctx.warn(mismatches);
    }
    let mut queue = CloneQueue::new(None, meta_depth)
        .config(root, &config)
        .raw(&request.workspace()?.members);
    // a selection the user chose with meta's filters: only those, and none
    // the new clones list (the selection alone can't say: in a plain git
    // worktree of the workspace it holds the root alone)
    if request.meta_options.typed_filter() {
        queue = queue.only(request.projects.iter().map(|p| p.abs.clone()));
    }
    let top = Arc::from([MetaRepo::at(&root_name(root), root, None)]);
    // The whole tree on disk is read here; only clones add to it
    let tasks = queue.discover_in(&workspace, 0, &top)?;
    let (clones, prunes) = (!prune_only, prune || prune_only);
    let tasks = if clones { tasks } else { Vec::new() };
    let preview = ctx.dry_run();
    let json = ctx.meta().json;
    let say = |line: String| -> anyhow::Result<()> {
        if !json {
            outln!("{line}")?;
        }
        Ok(())
    };
    // records name each repo by its path from the workspace root, as meta does
    let named = named(request, root);
    let project = |path: &Path| match path.strip_prefix(named) {
        Ok(inside) if inside.as_os_str().is_empty() => ".".to_string(),
        Ok(inside) => inside.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    };
    let record = |path: &Path, action: &'static str, reason: Option<String>| {
        let data = Record { action, reason };
        match action {
            "failed" => ProjectResult {
                error: Some(ErrorInfo::new(
                    ErrorCode::Failed,
                    data.reason.clone().unwrap_or_default(),
                )),
                ..ProjectResult::record(project(path), data)
            },
            _ => ProjectResult::record(project(path), data),
        }
        .at(path)
    };
    let mut records = Vec::new();

    if preview && clones && tasks.is_empty() {
        say(format!(
            "{} Nothing to clone: every project the .meta.yaml files list is cloned",
            "[DRY RUN]".cyan()
        ))?;
    } else if preview && clones {
        say(format!(
            "{} Would run (from the .meta.yaml files as they are now; the clones may add projects):",
            "[DRY RUN]".cyan()
        ))?;
        for task in &tasks {
            let argv = crate::clone_worker::git_argv(task, None, |url| url.to_string().into());
            let argv: Vec<String> = argv.iter().map(|a| a.to_string_lossy().into()).collect();
            say(format!(
                "  {}",
                escape(&meta_plugin::display::display_command(&argv))
            ))?;
            records.push(record(&task.target_path, "cloned", None));
        }
    } else if !tasks.is_empty() {
        let urls: Vec<_> = tasks.iter().map(|task| task.url.clone()).collect();
        run(&queue, tasks, &urls)?;
    }
    let outcome = queue.outcome();
    if !preview {
        records.extend(
            outcome
                .cloned
                .iter()
                .map(|path| record(path, "cloned", None)),
        );
    }
    // each with git's stderr, as `meta --json exec` gives a failed step's
    records.extend(outcome.failures.iter().map(|failure| ProjectResult {
        stderr: Some(failure.error.clone()),
        ..record(&failure.path, "failed", Some(failure.error.clone()))
    }));
    records.extend(outcome.refused.iter().map(|(_, path, why)| {
        record(
            path,
            "failed",
            Some(format!("not a git repository ({why})")),
        )
    }));

    if !prunes {
        report_unlisted(ctx, &queue, root, cwd, meta_depth != Some(0));
    }
    let (mut removed, mut kept, mut failed_removals) = (0, 0, 0);
    if prunes {
        for path in &outcome.unlisted {
            let shown = shown(path, cwd);
            let leftover = match Leftover::of(path, root) {
                Leftover::Removable(ignored) => Ok(ignored),
                Leftover::Holds(why) => Err(why),
            };
            // what is deleted along with it is said, in the preview too
            let note = |ignored: &Option<String>| match ignored {
                Some(ignored) => format!("nothing unpushed; {ignored} deleted with it"),
                None => "nothing unpushed".to_string(),
            };
            let (action, reason) = match leftover {
                Ok(ignored) if preview => {
                    say(format!("Would remove {shown} ({})", note(&ignored)))?;
                    ("removed", ignored.as_ref().map(|_| note(&ignored)))
                }
                Ok(ignored) => match std::fs::remove_dir_all(path) {
                    Ok(()) => {
                        say(format!(
                            "{} {shown} ({})",
                            "Removed".green(),
                            note(&ignored)
                        ))?;
                        ("removed", ignored.as_ref().map(|_| note(&ignored)))
                    }
                    // what was removed before the error is gone
                    Err(e) => {
                        let why = format!("it was removed only in part ({e})");
                        say(format!("{} {shown}: {why}", "Not removed".red()))?;
                        ("failed", Some(why))
                    }
                },
                Err(why) => {
                    let verb = if preview { "Would keep" } else { "Kept" };
                    say(format!("{} {shown}: {why}", verb.yellow()))?;
                    ("kept", Some(why))
                }
            };
            match action {
                "removed" => removed += 1,
                "kept" => kept += 1,
                _ => failed_removals += 1,
            }
            records.push(record(path, action, reason));
        }
    }

    let failed = clones
        .then(|| crate::clone_worker::report_failures(&queue, "git update"))
        .flatten();
    if let Some(error) = failed {
        return crate::printed(ctx, records, Some(anyhow::anyhow!("{error}")));
    }
    if failed_removals > 0 {
        let error = anyhow::anyhow!("{failed_removals} repositories could not be removed");
        return crate::printed(ctx, records, Some(error));
    }
    if !preview && clones {
        if let Some(rest) = deferred(ctx, request, cwd, &queue, false)? {
            records.extend(match rest.answer {
                meta_plugin::Answer::Results(results) => results.records,
                _ => Vec::new(),
            });
        }
    }
    if !preview {
        let cloned = outcome.cloned.len();
        say(match (clones, prunes) {
            (true, false) => format!("Update completed ({cloned} cloned)"),
            (true, true) => {
                format!("Update completed ({cloned} cloned, {removed} removed, {kept} kept)")
            }
            (false, _) => format!("Update completed ({removed} removed, {kept} kept)"),
        })?;
    }
    Ok(Response::printed(records))
}

/// Clone the projects at `dirs` that the `.meta.yaml` files of `workspace`
/// (and of the meta repos nested in it) list and that aren't cloned, the way
/// `update` does, for `command` (`git worktree create`). The folders
/// it cloned, or the error naming each clone that failed.
pub(crate) fn clone_dirs(
    workspace: &crate::here::Listing,
    members: &[meta_plugin::Project],
    dirs: &[PathBuf],
    command: &str,
) -> anyhow::Result<Result<Vec<PathBuf>, String>> {
    let root = workspace.dir.as_path();
    let queue = CloneQueue::new(None, None)
        .only(dirs.iter().cloned())
        .raw(members);
    let top = Arc::from([MetaRepo::at(&root_name(root), root, None)]);
    let tasks = queue.discover_in(workspace, 0, &top)?;
    let urls: Vec<_> = tasks.iter().map(|task| task.url.clone()).collect();
    if !tasks.is_empty() {
        run(&queue, tasks, &urls)?;
    }
    if let Some(error) = crate::clone_worker::report_failures(&queue, command) {
        return Ok(Err(error));
    }
    Ok(Ok(queue.outcome().cloned))
}

/// The workspace `request` runs in that `meta <command>` acts on (with
/// `-A`, the outermost): its config's folder and the projects it lists.
/// Refused inside a worktree set meta made: the workspace around the set is
/// not the command's to change from in it (`in_set`: what to run there
/// instead). A plain git worktree of a meta repo under .worktrees is a
/// workspace of its own: one that meta didn't make, whose .meta.yaml is its
/// own.
pub(crate) fn workspace(request: &Request, command: &str, in_set: &str) -> anyhow::Result<Listing> {
    let cwd = request.cwd.as_path();
    let Some(here) = Here::of(request) else {
        anyhow::bail!("No .meta.yaml found in {} or above it", escape_path(cwd));
    };
    if let Some(set) = &request.set {
        let real = |dir: &Path| dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        if !real(&here.dir).starts_with(real(&set.dir))
            || crate::worktree::scan::holds_meta_checkout(&set.dir)
        {
            anyhow::bail!(
                "`meta {command}` acts on the workspace, not worktree set '{}'; run it in the \
                 workspace{in_set}",
                escape(&set.name)
            );
        }
    }
    Ok(here.tree().listing())
}

/// Clone the projects `queue` left for another run (their URL holds
/// credentials meta hides in what it reads back): `meta git update`
/// from `cwd` with meta's options as typed, whose request has each URL as
/// configured. `None` when there are none. One whose config didn't change
/// in this run (`fresh`: or all of them, for a run whose configs are none
/// of meta's request) another run can't do better on: an error naming it.
pub(crate) fn deferred(
    ctx: &Ctx,
    request: &Request,
    cwd: &Path,
    queue: &CloneQueue,
    fresh: bool,
) -> anyhow::Result<Option<Response>> {
    let deferred = queue.outcome().deferred;
    if deferred.is_empty() {
        return Ok(None);
    }
    if !fresh && !deferred.iter().any(|(_, _, new)| *new) {
        let names: Vec<String> = deferred
            .iter()
            .map(|(name, _, _)| escape(name).into_owned())
            .collect();
        anyhow::bail!(
            "Not cloned: {}; meta's view of the workspace doesn't give their URLs, which hold \
             credentials. Run 'meta git update' in the meta repo that lists them.",
            names.join(", ")
        );
    }
    let words = ["git".to_string(), "update".to_string()];
    crate::through_meta(ctx, request, cwd, &words).map(Some)
}

/// The folder projects are named from in records: the workspace root meta
/// names them from, else `root`.
pub(crate) fn named<'a>(request: &'a Request, root: &'a Path) -> &'a Path {
    request
        .workspace
        .as_ref()
        .map_or(root, |w| w.root.as_path())
}

/// The name the workspace at `root` goes by in progress and errors.
pub(crate) fn root_name(root: &Path) -> String {
    root.file_name()
        .map_or_else(|| ".".to_string(), |n| n.to_string_lossy().into_owned())
}

/// Clone `tasks`, and the tasks their `.meta.yaml` files add, four
/// at a time, over SSH masters set up for `urls` (the remotes the run
/// contacts first).
pub(crate) fn run(
    queue: &CloneQueue,
    tasks: Vec<CloneTask>,
    urls: &[RepoUrl],
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
        let held = crate::worktree::safety::repo_holds(path)
            .and_then(|held| Ok((held, crate::worktree::safety::repo_ignored(path)?)));
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
                meta_plugin::display::escape_lines(&format!("{e:#}"))
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
pub(crate) fn report_unlisted(
    ctx: &Ctx,
    queue: &CloneQueue,
    root: &Path,
    cwd: &Path,
    recursive: bool,
) {
    let prune = if recursive {
        "meta -r git update --prune"
    } else {
        "meta git update --prune"
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
        ctx.warn(warning);
    }
}
