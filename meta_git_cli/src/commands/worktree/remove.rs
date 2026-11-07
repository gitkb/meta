use anyhow::{Context, Result};
use colored::*;
use meta_core::set_record as record;
use meta_plugin::display::{escape, escape_path};
use meta_plugin::outln;
use meta_plugin::{Ctx, ProjectResult, Request, Response};

use crate::worktree::hooks::fire_post_destroy;
use crate::worktree::safety::{check_worktree_removal, delete_checked_set};
use crate::worktree::types::*;
use crate::worktree::workspace::Workspace;
use meta_plugin::sets::lease;

use super::cli_types::RemoveArgs;

pub(crate) fn handle_remove(args: RemoveArgs, ctx: &Ctx, request: &Request) -> Result<Response> {
    meta_plugin::sets::validate_name(&args.name)?;
    // Hooks come from the owner: inside the set being removed, its own
    // config goes with it
    let here = crate::here::Here::need(request)?;
    let worktree_root = crate::here::worktree_root(request)?;
    meta_plugin::sets::owners_dir(&worktree_root)?;
    let wt_dir = worktree_root.join(&args.name);
    if !wt_dir.exists() {
        anyhow::bail!(
            "Worktree '{}' not found at {}",
            args.name,
            escape_path(&wt_dir)
        );
    }
    let how = How {
        preview: ctx.dry_run(),
        verbose: request.meta.verbose,
        json: request.meta.json,
        strict: ctx.meta().strict,
    };
    let mut records = Vec::new();
    let removed = remove_set(&args, &wt_dir, &here, ctx, how, &mut records);
    crate::printed_with_git(ctx, request, records, removed.err())
}

/// How a remove runs: meta's options.
#[derive(Clone, Copy)]
pub(crate) struct How {
    /// `--dry-run`: only say what would be removed
    pub preview: bool,
    pub verbose: bool,
    /// Say nothing: the record is for `--json`
    pub json: bool,
    /// A failing hook is an error
    pub strict: bool,
}

/// Remove the set named `args.name` at `wt_dir`, after checking it; its
/// record (what was removed) into `records`, also when the hook that
/// follows fails.
pub(crate) fn remove_set(
    args: &RemoveArgs,
    wt_dir: &std::path::Path,
    here: &crate::here::Here<'_>,
    ctx: &Ctx,
    how: How,
    records: &mut Vec<ProjectResult>,
) -> Result<()> {
    let How {
        preview,
        verbose,
        json,
        strict,
    } = how;
    let name = &args.name;
    let force = args.force;
    let wt_dir = wt_dir.to_path_buf();

    let workspace = Workspace::of(&here.home)?;
    // A set being made is its creator's. Otherwise the set's lease is held
    // until the set is deleted, so no create of it starts meanwhile; one its creator left marks a set it
    // never finished, which is meta's to remove, recorded or not. A preview
    // only looks.
    let set = wt_dir
        .canonicalize()
        .with_context(|| format!("Cannot resolve {}", escape_path(&wt_dir)))?;
    let found = lease::find(&set);
    let held = match () {
        _ if preview => match &found {
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
        (None, Some(found)) => found.lease.clone(),
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
                    meta_plugin::display::escape_lines(&format!("{e:#}"))
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
    if !check.unsaved.is_empty() {
        let found = check.unsaved.join("\n  ");
        if !force {
            anyhow::bail!(
                "Worktree '{name}' has work that removing it would lose:\n  {found}\n\
                 Commit and push it, or use --force to discard it."
            );
        }
        ctx.warn(format!(
            "{} work in worktree '{name}':\n  {found}",
            if preview {
                "would discard"
            } else {
                "discarding"
            }
        ));
    }

    // Read before git deletes the worktrees (their notes with them): the
    // branches meta created for the set, and the others' branches, which are
    // left, and named
    let mut made: Vec<(std::path::PathBuf, String)> = Vec::new();
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
            Some(branch) => made.push((git_dir, branch)),
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

    // Once the worktrees are gone (a preview only looks); what goes wrong
    // here keeps a branch, never stops the remove
    let mut branches: Vec<BranchReport> = made
        .into_iter()
        .filter_map(|(git_dir, branch)| {
            let (action, reason) = match branch_fate(&git_dir, &branch, preview) {
                BranchFate::Deleted => ("deleted", None),
                BranchFate::Gone => return None,
                BranchFate::Kept(why) => ("kept", Some(why)),
            };
            Some(BranchReport {
                repo: repo_of(&git_dir).display().to_string(),
                branch,
                action,
                reason,
            })
        })
        .collect();
    branches.extend(left.into_iter().map(|(git_dir, branch)| BranchReport {
        repo: repo_of(&git_dir).display().to_string(),
        branch,
        action: "left",
        reason: Some("no record says meta made it for this set".to_string()),
    }));

    // with --json the record says it
    if !json {
        if preview {
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
        for report in &branches {
            let (verb, why) = match (report.action, preview) {
                ("deleted", true) => ("would delete", None),
                ("deleted", false) => ("deleted", None),
                ("kept", true) => ("would keep", report.reason.as_deref()),
                ("kept", false) => ("kept", report.reason.as_deref()),
                (_, true) => ("would leave", report.reason.as_deref()),
                (_, false) => ("left", report.reason.as_deref()),
            };
            let shown = format!(
                "branch '{}' in {}",
                escape(&report.branch),
                escape_path(std::path::Path::new(&report.repo))
            );
            match (report.action, why) {
                ("kept", Some(why)) => outln!("  {} {shown}: {why}", verb.yellow())?,
                (_, Some(why)) => outln!("  {verb} {shown} ({why})")?,
                (_, None) => {
                    outln!("  {verb} {shown} (all its commits are on a remote or another branch)")?
                }
            }
        }
    }
    let output = DestroyOutput {
        name: name.to_string(),
        path: wt_dir.display().to_string(),
        repos_removed: check.members.len(),
        dry_run: preview,
        branches,
    };
    records.push(ProjectResult::about_workspace(output).at(&wt_dir));
    if preview {
        return Ok(());
    }
    // What was done is reported first: a failing hook doesn't undo it
    match fire_post_destroy(name, &wt_dir, force, Some(here.worktree)) {
        Err(e) if strict => anyhow::bail!("{e:#} (strict mode)"),
        Err(e) => ctx.warn(format!("{e:#}")),
        Ok(()) => {}
    }
    Ok(())
}

/// The checkout the git directory `git_dir` belongs to, as the user knows it.
fn repo_of(git_dir: &std::path::Path) -> &std::path::Path {
    match git_dir.file_name() {
        Some(name) if name == ".git" => git_dir.parent().unwrap_or(git_dir),
        _ => git_dir,
    }
}

/// What becomes of a branch meta created for a set being removed.
enum BranchFate {
    /// Deleted (or, in a preview, would be): nothing on it is lost
    Deleted,
    /// Gone already
    Gone,
    /// Kept, and why
    Kept(String),
}

/// Delete `branch` from the repository whose git directory is `git_dir` if
/// every commit on it is on a remote, a tag or another branch, as `git
/// branch -d` would; `preview`: only say whether it would be. A git error
/// keeps it. Commits only its reflog holds (reset, amended or rebased away)
/// are not kept: git counts them discarded and expires them itself.
fn branch_fate(git_dir: &std::path::Path, branch: &str, preview: bool) -> BranchFate {
    let commits = |count: usize| match count {
        1 => "1 commit".to_string(),
        n => format!("{n} commits"),
    };
    let fate = || -> Result<BranchFate> {
        let full = format!("refs/heads/{branch}");
        if meta_core::git::lookup(git_dir, ["rev-parse", "--verify", "--quiet", &full])?.is_none() {
            return Ok(BranchFate::Gone);
        }
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
                commits(unique.len()),
                escape(first)
            )));
        }
        if preview {
            return Ok(BranchFate::Deleted);
        }
        // git refuses a branch checked out in another worktree
        meta_core::git::run(git_dir, ["branch", "-D", "--", branch])?;
        Ok(BranchFate::Deleted)
    };
    fate().unwrap_or_else(|e| {
        BranchFate::Kept(meta_plugin::display::escape_lines(&format!("{e:#}")).into_owned())
    })
}
