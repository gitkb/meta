use anyhow::Result;
use colored::*;
use meta_core::display::escape;
use rayon::prelude::*;

use meta_git_lib::worktree::git_ops::repo_status;
use meta_git_lib::worktree::helpers::resolve_existing_worktree;
use meta_git_lib::worktree::types::*;

use super::cli_types::StatusArgs;

/// Render a named set, preserving legacy defaults unless recursive is requested.
pub(crate) fn handle_status(args: StatusArgs, json: bool) -> Result<()> {
    let name = &args.name;
    let ctx = resolve_existing_worktree(name)?;
    // Membership without git; then one status per repo
    let members = if args.recursive {
        meta_cli::worktree::discover_worktree_members_recursive(&ctx.wt_dir)?
    } else {
        meta_cli::worktree::discover_worktree_members(&ctx.wt_dir)?
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
            let status = if args.recursive {
                observed?
            } else {
                observed.unwrap_or_default()
            };
            let (ahead, behind) = status.ahead_behind.unwrap_or_default();
            Ok(StatusRepoEntry {
                alias: r.alias.clone(),
                path: r.path.display().to_string(),
                branch: status.branch.clone().unwrap_or_else(|| "HEAD".to_string()),
                dirty: status.is_dirty(),
                modified_count: status.changed_paths().count(),
                untracked_count: status.untracked_count(),
                ahead,
                behind,
                modified_files: status.changed_paths().map(str::to_string).collect(),
            })
        })
        .collect::<Result<Vec<_>>>()?;

    if json {
        let output = StatusOutput {
            name: name.to_string(),
            repos: statuses,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{}:", name.bold());
        for s in &statuses {
            let status_icon = if s.dirty {
                "●".yellow().to_string()
            } else {
                "✓".green().to_string()
            };
            let mut details = Vec::new();
            if s.modified_count > 0 {
                details.push(format!("{} modified", s.modified_count));
            }
            if s.untracked_count > 0 {
                details.push(format!("{} untracked", s.untracked_count));
            }
            if s.ahead > 0 {
                details.push(format!("↑{}", s.ahead));
            }
            if s.behind > 0 {
                details.push(format!("↓{}", s.behind));
            }
            let detail_str = if details.is_empty() {
                "clean".to_string()
            } else {
                details.join(", ")
            };
            println!(
                "  {} {:12} {:20} {}",
                status_icon,
                escape(&s.alias),
                escape(&s.branch),
                detail_str
            );
        }
    }

    Ok(())
}
