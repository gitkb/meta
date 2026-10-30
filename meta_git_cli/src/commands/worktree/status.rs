use anyhow::Result;
use colored::*;
use meta_core::display::escape;
use rayon::prelude::*;

use meta_git_lib::worktree::git_ops::*;
use meta_git_lib::worktree::helpers::{
    discover_and_validate_worktree, resolve_existing_worktree, validate_worktree_name,
};
use meta_git_lib::worktree::types::*;

use super::cli_types::StatusArgs;

/// Render a named set, preserving legacy defaults unless recursive is requested.
pub(crate) fn handle_status(args: StatusArgs, _verbose: bool, json: bool) -> Result<()> {
    let name = &args.name;

    let repos = if args.recursive {
        validate_worktree_name(name)?;
        let ctx = resolve_existing_worktree(name)?;
        let repos = meta_cli::worktree::discover_worktree_repos_recursive(&ctx.wt_dir)?;
        anyhow::ensure!(!repos.is_empty(), "No repos found in worktree '{name}'");
        repos
    } else {
        discover_and_validate_worktree(name)?
    };

    let statuses: Vec<StatusRepoEntry> = repos
        .par_iter()
        .map(|r| {
            let observed = git_status_summary(&r.path);
            let summary = if args.recursive {
                observed?
            } else {
                observed.unwrap_or(GitStatusSummary {
                    dirty: false,
                    modified_files: vec![],
                    untracked_count: 0,
                })
            };
            let (ahead, behind) = git_ahead_behind(&r.path).unwrap_or((0, 0));

            Ok(StatusRepoEntry {
                alias: r.alias.clone(),
                path: r.path.display().to_string(),
                branch: r.branch.clone(),
                dirty: summary.dirty,
                modified_count: summary.modified_files.len(),
                untracked_count: summary.untracked_count,
                ahead,
                behind,
                modified_files: summary.modified_files,
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
