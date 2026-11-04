//! `meta git clone-missing`, `clone-prune` and `clone-sync`: clone the
//! projects the `.meta.yaml` files list that aren't there, and remove the
//! repos that are there that none lists, with the `.meta.yaml` files as they
//! are on disk.

use crate::update::{self, Leftover};
use colored::Colorize;
use meta_git_lib::clone_queue::{CloneQueue, MetaRepo, Work};
use meta_plugin::display::{escape, escape_path};
use meta_plugin::outln;
use meta_plugin::{CommandResult, PluginRequestOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;

macro_rules! no_pull {
    () => {
        "\n\nNo repo that is cloned is pulled, fetched or checked out; `meta git \
                       update` also pulls them. With -r the same goes for the meta repos nested \
                       in it (those cloned, and those it clones). --include, --exclude and --tag \
                       choose the projects it clones; projects the new clones list are cloned \
                       only without them."
    };
}

macro_rules! prune {
    () => {
        "\n\nA repo is removed only when git shows nothing in it that isn't on a \
                     remote: no uncommitted or untracked files, stashes, commits on a branch \
                     or tag that no remote has, or linked worktrees, and no git repo inside it. \
                     A repo that holds anything is kept and named with what it holds. Files git \
                     ignores (.env, target/) are deleted with it, and named first; commits only \
                     a reflog holds are not kept, as git counts them discarded. Folders that are no git \
                     repo of their own, links, and the projects a .meta.yaml lists are never \
                     removed. With -r the meta repos nested in it are searched too; --include, \
                     --exclude and --tag choose the meta repos searched."
    };
}

/// What the clone commands take of git's options: there is no such git
/// command.
const NO_GIT_OPTIONS: &str = "Git options: none; git has no command of this name.";

/// `meta git clone-missing`
#[derive(clap::Parser)]
#[command(
    after_help = NO_GIT_OPTIONS,
    about = "Clone each project in .meta.yaml that isn't cloned; change no repo that is",
    long_about = concat!(
        "Clone each project in .meta.yaml that isn't cloned; change no repo that is.",
        no_pull!()
    )
)]
pub(crate) struct CloneMissingArgs {}

/// `meta git clone-prune`
#[derive(clap::Parser)]
#[command(
    after_help = NO_GIT_OPTIONS,
    about = "Remove each repo in the workspace that no .meta.yaml lists and that holds nothing unpushed",
    long_about = concat!(
        "Remove each repo in the workspace's folder that no .meta.yaml lists and that holds \
         nothing unpushed; name each one it keeps, with what it holds.",
        prune!()
    )
)]
pub(crate) struct ClonePruneArgs {}

/// `meta git clone-sync`
#[derive(clap::Parser)]
#[command(
    after_help = NO_GIT_OPTIONS,
    about = "clone-missing, then clone-prune: the clones become what the .meta.yaml files list",
    long_about = concat!(
        "clone-missing, then clone-prune: clone each project in .meta.yaml that isn't cloned, \
         then remove each repo no .meta.yaml lists that holds nothing unpushed.",
        no_pull!(),
        prune!()
    )
)]
pub(crate) struct CloneSyncArgs {}

/// What a command does: clone, prune, or both.
#[derive(Clone, Copy)]
pub(crate) enum Steps {
    Clone,
    Prune,
    Sync,
}

impl Steps {
    fn command(self) -> &'static str {
        match self {
            Self::Clone => "git clone-missing",
            Self::Prune => "git clone-prune",
            Self::Sync => "git clone-sync",
        }
    }
    fn clones(self) -> bool {
        !matches!(self, Self::Prune)
    }
    fn prunes(self) -> bool {
        !matches!(self, Self::Clone)
    }
}

impl From<CloneMissingArgs> for Steps {
    fn from(CloneMissingArgs {}: CloneMissingArgs) -> Self {
        Self::Clone
    }
}

impl From<ClonePruneArgs> for Steps {
    fn from(ClonePruneArgs {}: ClonePruneArgs) -> Self {
        Self::Prune
    }
}

impl From<CloneSyncArgs> for Steps {
    fn from(CloneSyncArgs {}: CloneSyncArgs) -> Self {
        Self::Sync
    }
}

/// What became of one repo, for `--json`.
#[derive(serde::Serialize)]
struct Record {
    /// From the workspace root
    path: String,
    /// cloned, removed, kept or failed (with --dry-run: what would be)
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// Run `steps` (meta's `-r`: nested meta repos too) in the workspace at or
/// above `cwd`, on the selected `projects` when meta's options filter them.
pub(crate) fn execute(
    steps: Steps,
    projects: &[String],
    options: &PluginRequestOptions,
    cwd: &Path,
) -> anyhow::Result<CommandResult> {
    let recursive = options.recursive || options.all;
    // Without -r only the workspace's own .meta.yaml is read
    let meta_depth = if recursive { options.depth } else { Some(0) };
    let workspace = update::workspace(cwd, steps.command(), "")?;
    // -A: from the outermost workspace
    let workspace = if options.all {
        workspace.root()?
    } else {
        workspace
    };
    let root = workspace.dir.as_path();
    let mut queue = CloneQueue::new(None, meta_depth, false);
    if options.filtered() {
        queue = queue.only(projects.iter().map(PathBuf::from));
    }
    let top = Arc::from([MetaRepo::at(&update::root_name(root), root, None)]);
    // The whole tree on disk is read here; only clones add to it
    let tasks = queue.discover_in(&workspace, 0, &top)?;
    let tasks = if steps.clones() { tasks } else { Vec::new() };
    let preview = options.mode().is_preview();
    let json = options.json_output;
    let say = |line: String| -> anyhow::Result<()> {
        if !json {
            outln!("{line}")?;
        }
        Ok(())
    };
    let from_root = |path: &Path| escape_path(path.strip_prefix(root).unwrap_or(path));
    let mut records = Vec::new();

    if preview && steps.clones() && tasks.is_empty() {
        say(format!(
            "{} Nothing to clone: every project the .meta.yaml files list is cloned",
            "[DRY RUN]".cyan()
        ))?;
    } else if preview && steps.clones() {
        say(format!(
            "{} Would run (from the .meta.yaml files as they are now; the clones may add projects):",
            "[DRY RUN]".cyan()
        ))?;
        for task in &tasks {
            let argv = crate::clone_worker::git_argv(task, None, |url| url.to_string().into());
            let argv: Vec<String> = argv.iter().map(|a| a.to_string_lossy().into()).collect();
            say(format!("  {}", escape(&loop_lib::display_command(&argv))))?;
            records.push(Record {
                path: from_root(&task.target_path),
                action: "cloned",
                reason: None,
            });
        }
    } else if !tasks.is_empty() {
        let urls: Vec<_> = tasks
            .iter()
            .filter_map(|task| match &task.work {
                Work::Clone(url) => Some(url.clone()),
                Work::Pull => None,
            })
            .collect();
        update::run(&queue, tasks, &urls)?;
    }
    let outcome = queue.outcome();
    if !preview {
        records.extend(outcome.cloned.iter().map(|path| Record {
            path: from_root(path),
            action: "cloned",
            reason: None,
        }));
    }
    records.extend(outcome.failures.iter().map(|failure| Record {
        path: from_root(&failure.path),
        action: "failed",
        reason: Some(failure.error.clone()),
    }));
    records.extend(outcome.refused.iter().map(|(_, path, why)| Record {
        path: from_root(path),
        action: "failed",
        reason: Some(format!("not a git repository ({why})")),
    }));

    let (mut removed, mut kept, mut failed_removals) = (0, 0, 0);
    if steps.prunes() {
        for path in &outcome.unlisted {
            let shown = update::shown(path, cwd);
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
            records.push(Record {
                path: from_root(path),
                action,
                reason,
            });
        }
    }

    if json {
        let report = serde_json::json!({"dry_run": preview, "repos": records});
        outln!("{report}")?;
    }
    let failed = steps
        .clones()
        .then(|| crate::clone_worker::report_failures(&queue, steps.command()))
        .flatten();
    if let Some(error) = failed {
        return Ok(CommandResult::Error(error));
    }
    if failed_removals > 0 {
        return Ok(CommandResult::Error(format!(
            "{failed_removals} repositories could not be removed"
        )));
    }
    if !preview {
        let cloned = format!("{} cloned", outcome.cloned.len());
        let pruned = format!("{removed} removed, {kept} kept");
        say(match steps {
            Steps::Clone => format!("Clone completed ({cloned})"),
            Steps::Prune => format!("Prune completed ({pruned})"),
            Steps::Sync => format!("Sync completed ({cloned}, {pruned})"),
        })?;
    }
    Ok(CommandResult::Message(String::new()))
}

/// Clone the projects at `dirs` that the `.meta.yaml` files of `workspace`
/// (and of the meta repos nested in it) list and that aren't cloned, the way
/// `clone-missing` does, for `command` (`git worktree create`). The folders
/// it cloned, or the error naming each clone that failed.
pub(crate) fn clone_dirs(
    workspace: &meta_core::config::Workspace,
    dirs: &[PathBuf],
    command: &str,
) -> anyhow::Result<Result<Vec<PathBuf>, String>> {
    let root = workspace.dir.as_path();
    let queue = CloneQueue::new(None, None, false).only(dirs.iter().cloned());
    let top = Arc::from([MetaRepo::at(&update::root_name(root), root, None)]);
    let tasks = queue.discover_in(workspace, 0, &top)?;
    let urls: Vec<_> = tasks
        .iter()
        .filter_map(|task| match &task.work {
            Work::Clone(url) => Some(url.clone()),
            Work::Pull => None,
        })
        .collect();
    if !tasks.is_empty() {
        update::run(&queue, tasks, &urls)?;
    }
    if let Some(error) = crate::clone_worker::report_failures(&queue, command) {
        return Ok(Err(error));
    }
    Ok(Ok(queue.outcome().cloned))
}
