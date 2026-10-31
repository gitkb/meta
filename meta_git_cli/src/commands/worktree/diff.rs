use anyhow::Result;
use colored::*;
use meta_core::display::escape;
use rayon::prelude::*;

use meta_git_lib::worktree::git_ops::git_diff_stat;
use meta_git_lib::worktree::helpers::discover_and_validate_worktree;
use meta_git_lib::worktree::types::*;

use super::cli_types::DiffArgs;

pub(crate) fn handle_diff(args: DiffArgs, json: bool) -> Result<()> {
    let name = &args.name;
    let base_ref = &args.base;

    let repos = discover_and_validate_worktree(name)?;

    let diff_entries: Vec<DiffRepoEntry> = repos
        .par_iter()
        .map(|r| {
            let (files_changed, insertions, deletions, files) =
                git_diff_stat(&r.path, base_ref).unwrap_or((0, 0, 0, vec![]));
            DiffRepoEntry {
                alias: r.alias.clone(),
                base_ref: base_ref.to_string(),
                files_changed,
                insertions,
                deletions,
                files,
            }
        })
        .collect();

    let mut total_repos_changed = 0;
    let mut total_files = 0;
    let mut total_insertions = 0;
    let mut total_deletions = 0;
    for d in &diff_entries {
        if d.files_changed > 0 {
            total_repos_changed += 1;
            total_files += d.files_changed;
            total_insertions += d.insertions;
            total_deletions += d.deletions;
        }
    }

    if json {
        let output = DiffOutput {
            name: name.to_string(),
            base: base_ref.to_string(),
            repos: diff_entries,
            totals: DiffTotals {
                repos_changed: total_repos_changed,
                files_changed: total_files,
                insertions: total_insertions,
                deletions: total_deletions,
            },
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        // Human mode: always show stat summary
        println!("{} vs {}:", name.bold(), escape(base_ref));
        for d in &diff_entries {
            if d.files_changed > 0 {
                let insertions = d.insertions;
                let deletions = d.deletions;
                println!(
                    "  {:12} {} {} ({} files)",
                    escape(&d.alias),
                    format!("+{insertions}").green(),
                    format!("-{deletions}").red(),
                    d.files_changed,
                );
            }
        }
        if total_repos_changed > 0 {
            println!("  {}", "─".repeat(40));
            println!(
                "  {:12} {} {} ({} files, {} repos)",
                "Total",
                format!("+{total_insertions}").green(),
                format!("-{total_deletions}").red(),
                total_files,
                total_repos_changed,
            );
        } else {
            println!("  No changes vs {}", escape(base_ref));
        }
    }

    Ok(())
}
