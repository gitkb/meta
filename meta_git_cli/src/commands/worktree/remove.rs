use anyhow::{Context, Result};
use colored::*;
use meta_core::display::{escape, escape_path};
use meta_core::out;
use meta_core::outln;
use meta_core::set_record as record;
use meta_core::ExecutionMode;

use meta_git_lib::worktree::helpers::*;
use meta_git_lib::worktree::hooks::fire_post_destroy;
use meta_git_lib::worktree::lease;
use meta_git_lib::worktree::safety::{check_worktree_removal, delete_checked_set};
use meta_git_lib::worktree::types::*;
use meta_git_lib::worktree::workspace::Workspace;

use super::cli_types::RemoveArgs;

pub(crate) fn handle_remove(
    args: RemoveArgs,
    mode: ExecutionMode,
    verbose: bool,
    json: bool,
    strict: bool,
) -> Result<()> {
    validate_worktree_name(&args.name)?;
    // Hooks come from the owner: inside the set being removed, its own
    // config goes with it
    let cwd = std::env::current_dir()?;
    let configs = Configs::find(&cwd)?;
    let wt_dir = resolve_worktree_root(Some(&configs.owner), &cwd)?.join(&args.name);
    if !wt_dir.exists() {
        anyhow::bail!(
            "Worktree '{}' not found at {}",
            args.name,
            escape_path(&wt_dir)
        );
    }
    remove_set(&args, &wt_dir, &configs, mode, verbose, json, strict)
}

/// Remove the set named `args.name` at `wt_dir`, after checking it.
pub(crate) fn remove_set(
    args: &RemoveArgs,
    wt_dir: &std::path::Path,
    configs: &Configs,
    mode: ExecutionMode,
    verbose: bool,
    json: bool,
    strict: bool,
) -> Result<()> {
    let name = &args.name;
    let force = args.force;
    let wt_dir = wt_dir.to_path_buf();

    let workspace = Workspace::of(&configs.here)?;
    // A set being made is its creator's, and one in use by its ephemeral
    // run is the run's (a run removing its own set holds its lease itself).
    // Otherwise the set's lease is held until the set is deleted, so no
    // create of it starts meanwhile; one its creator left marks a set it
    // never finished, which is meta's to remove, recorded or not. A preview
    // only looks.
    let set = wt_dir
        .canonicalize()
        .with_context(|| format!("Cannot resolve {}", escape_path(&wt_dir)))?;
    let found = lease::find(&set);
    let mine = found.as_ref().is_some_and(|found| found.is_mine());
    let held = match () {
        _ if mine => None,
        _ if mode.is_preview() => match &found {
            Some(found) if found.live => anyhow::bail!(
                "Refusing to remove worktree '{name}': it is {}. Wait for that to finish, or \
                 stop it first.",
                found.doing()
            ),
            _ => None,
        },
        _ => Some(lease::hold(&set).map_err(|e| {
            anyhow::anyhow!(
                "Refusing to remove worktree '{name}': {e:#}. Wait for that to finish, or stop \
                 it first."
            )
        })?),
    };
    let left = match (&held, &found) {
        (Some((left, _)), _) => left.clone(),
        (None, Some(found)) if !mine => found.lease.clone(),
        _ => None,
    };
    // another workspace's lease, made from a repository both share, is no
    // authority here
    let abandoned = left.is_some_and(|lease| workspace.made_here(&lease.project));

    let check = check_worktree_removal(&wt_dir)?;
    // In an abandoned set, worktrees not recorded yet are the creator's: meta's
    let blockers: Vec<&str> = check.blockers_but(abandoned).map(String::as_str).collect();
    // Only this workspace's sets: a shared worktree directory may hold
    // another workspace's. No flag overrides this.
    let foreign = workspace.foreign_members(&wt_dir, &check);
    if !foreign.is_empty() {
        // Repositories meta couldn't read make sets from them look foreign
        let unreadable: String = workspace
            .unreadable
            .iter()
            .map(|(path, e)| {
                format!(
                    "\n  {}: {}",
                    escape_path(path),
                    meta_core::display::escape_lines(&format!("{e:#}"))
                )
            })
            .collect();
        anyhow::bail!(
            "Refusing to remove worktree '{name}': it is not a set of this workspace ({}):\n  {}\n\
             Remove it from the workspace that made it.{}",
            escape_path(&workspace.root),
            foreign.join("\n  "),
            if unreadable.is_empty() {
                String::new()
            } else {
                format!("\nParts of this workspace couldn't be read:{unreadable}")
            }
        );
    }
    if !blockers.is_empty() {
        anyhow::bail!(
            "Refusing to remove worktree '{name}':\n  {}\n\
             meta only deletes linked worktrees it can check; move or delete these yourself first.",
            blockers.join("\n  ")
        );
    }
    // Only delete what meta created: it records each worktree it makes.
    if check.record().is_none() && !abandoned {
        anyhow::bail!(
            "Refusing to remove {}: meta has no record of creating a worktree set there.\n{}",
            escape_path(&wt_dir),
            if check.members.is_empty() {
                "It holds no worktrees (any it had are gone, and their records with them); \
                 delete it yourself if you don't need what is left in it."
            } else {
                "If it was made by hand or by an older meta (which kept its records elsewhere), \
                 run 'git worktree remove' for each worktree in it, then delete it."
            }
        );
    }
    // A set folder may be a link, but to this set: one swapped for a link to
    // another set must not remove that one
    if let Some(made) = check.record().filter(|made| made.name() != *name) {
        anyhow::bail!(
            "Refusing to remove worktree '{name}': {} leads to worktree set '{}'; remove that \
             by its own name",
            escape_path(&wt_dir),
            escape(&made.name())
        );
    }
    let preview = mode.is_preview();
    if !check.unsaved.is_empty() {
        let found = check.unsaved.join("\n  ");
        if !force {
            anyhow::bail!(
                "Worktree '{name}' has work that removing it would lose:\n  {found}\n\
                 Commit and push it, or use --force to discard it."
            );
        }
        out::warn(format!(
            "{} work in worktree '{name}':\n  {found}",
            if preview {
                "would discard"
            } else {
                "discarding"
            }
        ));
    }

    // The branches meta created for the set, read before git deletes the
    // notes with the worktrees
    // (the others' branches, which it leaves, are named too)
    let mut branches: Vec<(std::path::PathBuf, String)> = Vec::new();
    let mut left: Vec<(std::path::PathBuf, String)> = Vec::new();
    for member in &check.members {
        let Ok(linked) = meta_core::gitdir::linked(&member.path) else {
            continue;
        };
        let git_dir = linked
            .common
            .canonicalize()
            .unwrap_or(linked.common.clone());
        match record::created_branch(&linked.admin) {
            Some(branch) => branches.push((git_dir, branch)),
            None => {
                if let Ok(Some(branch)) = meta_core::git::current_branch(&member.path) {
                    left.push((git_dir, branch));
                }
            }
        }
    }

    if !preview {
        // Remove exactly what was checked, nested worktrees first
        delete_checked_set(&wt_dir, &check, force, verbose)?;
        if let Some((_, claim)) = held {
            claim.delete()?;
        }
    }

    if json {
        let output = DestroyOutput {
            name: name.to_string(),
            path: wt_dir.display().to_string(),
            repos_removed: check.members.len(),
            dry_run: preview,
        };
        outln!("{}", serde_json::to_string_pretty(&output)?)?;
    } else if preview {
        outln!(
            "[DRY RUN] Would remove worktree '{}' at {}:",
            escape(name),
            escape_path(&wt_dir)
        )?;
        for member in &check.members {
            outln!("  {}", escape_path(&member.path))?;
        }
    } else {
        outln!("{} Removed worktree '{}'", "✓".green(), name.bold())?;
    }
    if !json {
        report_branches(&branches, preview)?;
        for (git_dir, branch) in &left {
            outln!(
                "  {} branch '{}' in {} (this set didn't make it)",
                if preview { "would leave" } else { "left" },
                escape(branch),
                repo_shown(git_dir)
            )?;
        }
    } else if !preview {
        for (git_dir, branch) in &branches {
            delete_branch(git_dir, branch)?;
        }
    }
    if preview {
        return Ok(());
    }
    // What was done is reported first: a failing hook doesn't undo it
    fire_post_destroy(name, &wt_dir, force, Some(&configs.owner))
        .or_else(|e| out::warn_or_bail(strict, format!("{e:#}")))
}

/// The checkout the git directory `git_dir` belongs to, as the user knows it.
fn repo_shown(git_dir: &std::path::Path) -> String {
    let dir = match git_dir.file_name() {
        Some(name) if name == ".git" => git_dir.parent().unwrap_or(git_dir),
        _ => git_dir,
    };
    escape_path(dir)
}

/// What became of a branch meta created for a set being removed.
enum BranchFate {
    /// Deleted (or, in a preview, would be): every commit on it is on a
    /// remote, another branch or a tag
    Deleted,
    /// Gone already
    Gone,
    /// Kept, and why
    Kept(String),
}

/// Delete `branch` from the repository whose git directory is `git_dir` if
/// no commit on it would be lost; `preview`: only say whether it would be.
fn branch_fate(git_dir: &std::path::Path, branch: &str, preview: bool) -> Result<BranchFate> {
    let full = format!("refs/heads/{branch}");
    if meta_core::git::lookup(git_dir, ["rev-parse", "--verify", "--quiet", &full])?.is_none() {
        return Ok(BranchFate::Gone);
    }
    // (--branches matches names without refs/heads/; a branch name holds no
    // glob characters)
    let exclude = format!("--exclude={branch}");
    let unique = meta_core::git::run(
        git_dir,
        [
            "log",
            "--oneline",
            &full,
            "--not",
            "--remotes",
            "--tags",
            &exclude,
            "--branches",
        ],
    )?;
    let unique: Vec<&str> = unique.lines().collect();
    if let Some(first) = unique.first() {
        return Ok(BranchFate::Kept(format!(
            "{} on no remote, other branch or tag (first: {})",
            match unique.len() {
                1 => "1 commit".to_string(),
                n => format!("{n} commits"),
            },
            escape(first)
        )));
    }
    if preview {
        return Ok(BranchFate::Deleted);
    }
    // git refuses a branch checked out in another worktree
    match meta_core::git::run(git_dir, ["branch", "-D", "--", branch]) {
        Ok(_) => Ok(BranchFate::Deleted),
        Err(e) => Ok(BranchFate::Kept(
            meta_core::display::escape_lines(&format!("{e:#}")).into_owned(),
        )),
    }
}

/// [`branch_fate`] without the report, for `--json`.
fn delete_branch(git_dir: &std::path::Path, branch: &str) -> Result<()> {
    branch_fate(git_dir, branch, false).map(|_| ())
}

/// Delete each branch meta created for the set (`branches`: the repository's
/// git directory and the branch) that holds nothing that isn't elsewhere,
/// and name each one kept, with why.
fn report_branches(branches: &[(std::path::PathBuf, String)], preview: bool) -> Result<()> {
    let repo = repo_shown;
    for (git_dir, branch) in branches {
        match branch_fate(git_dir, branch, preview)? {
            BranchFate::Deleted if preview => outln!(
                "  would delete branch '{}' in {} (all its commits are on a remote or another branch)",
                escape(branch),
                repo(git_dir)
            )?,
            BranchFate::Deleted => outln!(
                "  deleted branch '{}' in {} (all its commits are on a remote or another branch)",
                escape(branch),
                repo(git_dir)
            )?,
            BranchFate::Gone => {}
            BranchFate::Kept(why) => outln!(
                "  {} branch '{}' in {}: {why}",
                if preview { "would keep" } else { "kept" }.yellow(),
                escape(branch),
                repo(git_dir)
            )?,
        }
    }
    Ok(())
}
