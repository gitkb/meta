use anyhow::Result;
use colored::*;
use meta_core::display::escape;
use rayon::prelude::*;

use meta_cli::worktree::WorktreeMemberInfo;
use meta_git_lib::worktree::git_ops::repo_status;
use meta_git_lib::worktree::helpers::resolve_existing_worktree;
use meta_git_lib::worktree::types::*;

use super::cli_types::StatusArgs;

/// Render a named set, preserving legacy defaults unless recursive is requested.
pub(crate) fn handle_status(args: StatusArgs, json: bool) -> Result<()> {
    let name = &args.name;
    let wt_dir = resolve_existing_worktree(name)?;
    // Membership without git; then one status per repo
    let members = if args.recursive {
        meta_cli::worktree::discover_worktree_members_recursive(&wt_dir)?
    } else {
        meta_cli::worktree::discover_worktree_members(&wt_dir)?
    };
    anyhow::ensure!(
        !members.is_empty(),
        "No repos found in worktree '{}'",
        escape(name)
    );

    let statuses: Vec<StatusRepoEntry> = members
        .par_iter()
        .map(|r| {
            let observed = repo_status(&r.path);
            // what git can't read is unknown; a work tree it refuses still
            // has the branch and upstream git names
            let (status, refs) = match observed {
                Ok(status) => (Some(status.clone()), Some(status)),
                Err(e) if args.recursive => return Err(e),
                Err(_) => (
                    None,
                    meta_core::git::RepoStatus::refs(&r.path).ok().flatten(),
                ),
            };
            let counts = refs.as_ref().map(|r| r.ahead_behind.unwrap_or_default());
            Ok(StatusRepoEntry {
                alias: r.alias.clone(),
                path: r.path.display().to_string(),
                branch: refs.map(|r| r.branch.unwrap_or_else(|| "HEAD".to_string())),
                dirty: status.as_ref().map(|s| s.is_dirty()),
                modified_count: status.as_ref().map(|s| s.changed_paths().count()),
                untracked_count: status.as_ref().map(|s| s.untracked_count()),
                ahead: counts.map(|(ahead, _)| ahead),
                behind: counts.map(|(_, behind)| behind),
                modified_files: status
                    .iter()
                    .flat_map(|s| s.changed_paths().map(str::to_string))
                    .collect(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let missing = missing_members(&wt_dir, &members);

    if json {
        let output = StatusOutput {
            name: name.to_string(),
            repos: statuses,
            missing: missing.clone(),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{}:", name.bold());
        for s in &statuses {
            let status_icon = match s.dirty {
                Some(true) => "●".yellow().to_string(),
                Some(false) => "✓".green().to_string(),
                None => "?".red().to_string(),
            };
            let mut details = Vec::new();
            if let Some(n) = s.modified_count.filter(|&n| n > 0) {
                details.push(format!("{n} modified"));
            }
            if let Some(n) = s.untracked_count.filter(|&n| n > 0) {
                details.push(format!("{n} untracked"));
            }
            if let Some(n) = s.ahead.filter(|&n| n > 0) {
                details.push(format!("↑{n}"));
            }
            if let Some(n) = s.behind.filter(|&n| n > 0) {
                details.push(format!("↓{n}"));
            }
            let detail_str = match s.dirty {
                None => "unknown: git can't read its work tree".to_string(),
                Some(_) if details.is_empty() => "clean".to_string(),
                Some(_) => details.join(", "),
            };
            println!(
                "  {} {:12} {:20} {}",
                status_icon,
                escape(&s.alias),
                escape(s.branch.as_deref().unwrap_or("-")),
                detail_str
            );
        }
        for alias in &missing {
            println!(
                "  {} {:12} {:20} missing: its checkout is gone from the set",
                "✗".red(),
                escape(alias),
                "-"
            );
        }
    }
    anyhow::ensure!(
        missing.is_empty(),
        "worktree '{}' is missing the checkouts of {}",
        escape(name),
        escape(&missing.join(", "))
    );

    Ok(())
}

/// The projects whose repository still records a checkout in the set at
/// `wt_dir` that is gone (deleted by hand, say): from meta's record of
/// creating the set, the workspace it was made from, and each project's
/// worktree records.
fn missing_members(wt_dir: &std::path::Path, members: &[WorktreeMemberInfo]) -> Vec<String> {
    use meta_core::gitdir::Checkout;
    use meta_git_lib::worktree::record;
    let Some(made) = record::find(wt_dir, members.iter().map(|m| &m.path)) else {
        return Vec::new();
    };
    let Ok(Some(workspace)) = meta_core::config::Workspace::in_dir(&made.project) else {
        return Vec::new();
    };
    let projects = std::iter::once((".".to_string(), made.project.clone())).chain(
        workspace
            .projects
            .iter()
            .map(|p| (p.name.clone(), made.project.join(&p.path))),
    );
    projects
        .filter(|(alias, _)| !members.iter().any(|m| &m.alias == alias))
        .filter(|(_, source)| match Checkout::of(source) {
            Checkout::Repo { git_dir } => record::in_repo(&git_dir).iter().any(|(admin, found)| {
                found.as_ref().is_ok_and(|found| found.set == made.set)
                    && record::checkout_gone(admin)
            }),
            _ => false,
        })
        .map(|(alias, _)| alias)
        .collect()
}
