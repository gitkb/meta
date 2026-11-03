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

    // Read before git deletes the worktrees (their notes and HEAD reflogs
    // with them): the branches meta created for the set, with what only their
    // reflogs hold; the others' branches, which are left, and named; and the
    // commits only a worktree's HEAD reflog holds, saved under a ref so they
    // stay reachable
    let mut made: Vec<(std::path::PathBuf, String, Result<Vec<String>>)> = Vec::new();
    let mut left: Vec<(std::path::PathBuf, String)> = Vec::new();
    let mut kept_refs: Vec<KeptRef> = Vec::new();
    for member in &check.members {
        let Ok(linked) = meta_core::gitdir::linked(&member.path) else {
            continue;
        };
        let git_dir = linked
            .common
            .canonicalize()
            .unwrap_or(linked.common.clone());
        let place = member.path.strip_prefix(&wt_dir).unwrap_or(&member.path);
        // a ref that can't be made stops the remove before anything is deleted
        let (tips, commits) = head_reflog_tips(&member.path, &git_dir).with_context(|| {
            format!(
                "Refusing to remove worktree '{name}': can't tell what the HEAD reflog of {} holds",
                escape_path(&member.path)
            )
        })?;
        for tip in &tips {
            let ref_name = kept_ref(name, place, tip);
            if !preview {
                meta_core::git::run(&git_dir, ["update-ref", ref_name.as_str(), tip])
                    .with_context(|| {
                        format!(
                            "Refusing to remove worktree '{name}': can't save the commits only \
                             the HEAD reflog of {} holds",
                            escape_path(&member.path)
                        )
                    })?;
            }
            kept_refs.push(KeptRef {
                repo: repo_of(&git_dir).display().to_string(),
                ref_name,
                commits,
            });
        }
        match record::created_branch(&linked.admin) {
            Some(branch) => {
                // what the refs just saved hold is held (in a preview too)
                let reflogs = only_in_branch_reflog(&git_dir, &branch, &tips);
                made.push((git_dir, branch, reflogs));
            }
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
        .filter_map(|(git_dir, branch, reflogs)| {
            let (action, reason) = match branch_fate(&git_dir, &branch, reflogs, preview) {
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

    if json {
        let output = DestroyOutput {
            name: name.to_string(),
            path: wt_dir.display().to_string(),
            repos_removed: check.members.len(),
            dry_run: preview,
            branches: std::mem::take(&mut branches),
            kept_refs: std::mem::take(&mut kept_refs),
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
        for kept in &kept_refs {
            let repo = escape_path(std::path::Path::new(&kept.repo));
            outln!(
                "  {} {} only the worktree's HEAD reflog held, in {repo}, as {} (see them: \
                 git -C {repo} log {}; delete the ref once they're kept elsewhere)",
                if preview { "would save" } else { "saved" }.yellow(),
                match kept.commits {
                    1 => "1 commit".to_string(),
                    n => format!("{n} commits"),
                },
                escape(&kept.ref_name),
                escape(&kept.ref_name)
            )?;
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
    if preview {
        return Ok(());
    }
    // What was done is reported first: a failing hook doesn't undo it
    fire_post_destroy(name, &wt_dir, force, Some(&configs.owner))
        .or_else(|e| out::warn_or_bail(strict, format!("{e:#}")))
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
    /// Deleted (or, in a preview, would be): nothing on it or in its
    /// reflogs is lost
    Deleted,
    /// Gone already
    Gone,
    /// Kept, and why
    Kept(String),
}

/// Where refs/meta/kept keeps a tip from the worktree at `place` in set
/// `set`: `refs/meta/kept/<set>/<place>/<commit>`, `root` for the set's root,
/// and no part starting with a dot, which a ref name can't have.
fn kept_ref(set: &str, place: &std::path::Path, tip: &str) -> String {
    let place: Vec<String> = place
        .components()
        .map(|c| c.as_os_str().to_string_lossy().replace('.', "_"))
        .filter(|c| !c.is_empty())
        .collect();
    let place = if place.is_empty() {
        "root".to_string()
    } else {
        place.join("/")
    };
    let short = tip.get(..12).unwrap_or(tip);
    format!("refs/meta/kept/{set}/{place}/{short}")
}

/// What `rev-list --not` excludes as held elsewhere: remotes, tags,
/// branches (but `branch`), stashes, and what meta already kept.
fn held_elsewhere(git_dir: &std::path::Path, branch: Option<&str>) -> Vec<String> {
    let mut not = vec![
        "--not".to_string(),
        "--remotes".to_string(),
        "--tags".to_string(),
    ];
    if let Some(branch) = branch {
        // (--branches matches names without refs/heads/; a branch name holds
        // no glob characters)
        not.push(format!("--exclude={branch}"));
    }
    not.push("--branches".to_string());
    not.push("--glob=refs/meta/kept/*".to_string());
    let stashes =
        meta_core::git::run(git_dir, ["rev-list", "-g", "refs/stash", "--"]).unwrap_or_default();
    not.extend(stashes.lines().map(str::to_string));
    not
}

/// The commits reachable from `revs` (reflog entries) and from nothing in
/// `not`, as `git log --format` prints them, in parts so the command line
/// stays short.
fn reachable_only(
    git_dir: &std::path::Path,
    revs: &[String],
    not: &[String],
    format: &str,
) -> Result<Vec<String>> {
    let mut found: Vec<String> = Vec::new();
    for part in revs.chunks(200) {
        let mut args: Vec<&str> = vec!["log", format];
        args.extend(part.iter().map(String::as_str));
        args.extend(not.iter().map(String::as_str));
        for line in meta_core::git::run(git_dir, &args)?.lines() {
            if !found.iter().any(|known| known == line) {
                found.push(line.to_string());
            }
        }
    }
    Ok(found)
}

/// The entries of the reflog `rev` (run in `dir`), each once.
fn reflog_entries(dir: &std::path::Path, rev: &str) -> Result<Vec<String>> {
    let mut entries: Vec<String> = Vec::new();
    for sha in meta_core::git::run(dir, ["rev-list", "-g", rev, "--"])?.lines() {
        if !entries.iter().any(|known| known == sha) {
            entries.push(sha.to_string());
        }
    }
    Ok(entries)
}

/// The commits only the reflog of `branch` (in the repository whose git
/// directory is `git_dir`) holds: work reset, amended or rebased away on it,
/// on no remote, tag or other branch. Deleting the branch deletes its reflog.
fn only_in_branch_reflog(
    git_dir: &std::path::Path,
    branch: &str,
    saved: &[String],
) -> Result<Vec<String>> {
    let full = format!("refs/heads/{branch}");
    if meta_core::git::lookup(git_dir, ["rev-parse", "--verify", "--quiet", &full])?.is_none() {
        return Ok(Vec::new());
    }
    let entries = reflog_entries(git_dir, &full)?;
    let mut not = held_elsewhere(git_dir, Some(branch));
    not.extend(saved.iter().cloned());
    reachable_only(git_dir, &entries, &not, "--oneline")
}

/// The tips of what only the HEAD reflog of the worktree `checkout` holds
/// (commits on no branch, tag, remote, stash or meta-kept ref; git deletes
/// that reflog with the worktree), and how many commits that is.
fn head_reflog_tips(
    checkout: &std::path::Path,
    git_dir: &std::path::Path,
) -> Result<(Vec<String>, usize)> {
    let entries = reflog_entries(checkout, "HEAD")?;
    let not = held_elsewhere(git_dir, None);
    let unique = reachable_only(git_dir, &entries, &not, "--format=%H")?;
    if unique.is_empty() {
        return Ok((Vec::new(), 0));
    }
    // the entries that hold them, less those another one holds too
    let tips: Vec<String> = entries.into_iter().filter(|e| unique.contains(e)).collect();
    let tips = match tips.len() {
        0 | 1 => tips,
        _ => {
            let mut args = vec!["merge-base", "--independent"];
            args.extend(tips.iter().map(String::as_str));
            meta_core::git::run(git_dir, &args)?
                .lines()
                .map(str::to_string)
                .collect()
        }
    };
    Ok((tips, unique.len()))
}

/// Delete `branch` from the repository whose git directory is `git_dir` if
/// no commit on it, or only in its reflogs (`reflogs`), would be lost;
/// `preview`: only say whether it would be. A git error keeps it.
fn branch_fate(
    git_dir: &std::path::Path,
    branch: &str,
    reflogs: Result<Vec<String>>,
    preview: bool,
) -> BranchFate {
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
        let reflogs = reflogs?;
        if let Some(first) = reflogs.first() {
            return Ok(BranchFate::Kept(format!(
                "{} only in its reflog (reset, amended or rebased away; on no remote, other \
                 branch or tag; first: {})",
                commits(reflogs.len()),
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
        BranchFate::Kept(meta_core::display::escape_lines(&format!("{e:#}")).into_owned())
    })
}
