use anyhow::Result;
use colored::*;
use meta_core::display::{escape, escape_lines};
use meta_core::outln;
use rayon::prelude::*;

use meta_git_lib::worktree::git_ops::{git_diff_patch, git_diff_stat};
use meta_git_lib::worktree::helpers::discover_and_validate_worktree;
use meta_git_lib::worktree::types::*;

use super::cli_types::DiffArgs;

pub(crate) fn handle_diff(args: DiffArgs, json: bool) -> Result<()> {
    let name = &args.name;
    let base_ref = &args.base;

    let (_, repos) = discover_and_validate_worktree(name)?;

    let diff_entries: Vec<DiffRepoEntry> = repos
        .par_iter()
        .map(|r| {
            let mut entry = DiffRepoEntry {
                alias: r.alias.clone(),
                base_ref: base_ref.to_string(),
                files_changed: None,
                insertions: None,
                deletions: None,
                files: Vec::new(),
                error: None,
            };
            match git_diff_stat(&r.path, base_ref) {
                Ok((files_changed, insertions, deletions, files)) => {
                    entry.files_changed = Some(files_changed);
                    entry.insertions = Some(insertions);
                    entry.deletions = Some(deletions);
                    entry.files = files;
                }
                Err(error) => entry.error = Some(format!("{error:#}")),
            }
            entry
        })
        .collect();

    let mut total_repos_changed = 0;
    let mut total_files = 0;
    let mut total_insertions = 0;
    let mut total_deletions = 0;
    let failed = diff_entries.iter().filter(|d| d.error.is_some()).count();
    for d in &diff_entries {
        if let (Some(files), Some(insertions), Some(deletions)) =
            (d.files_changed, d.insertions, d.deletions)
        {
            if files == 0 {
                continue;
            }
            total_repos_changed += 1;
            total_files += files;
            total_insertions += insertions;
            total_deletions += deletions;
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
        outln!("{}", serde_json::to_string_pretty(&output)?)?;
    } else {
        outln!("{} vs {}:", name.bold(), escape(base_ref))?;
        for d in &diff_entries {
            if let Some(error) = &d.error {
                outln!("  {:12} error: {}", escape(&d.alias), escape(error))?;
            } else if let (Some(files), Some(insertions), Some(deletions)) =
                (d.files_changed, d.insertions, d.deletions)
            {
                if files == 0 {
                    continue;
                }
                outln!(
                    "  {:12} {} {} ({} files)",
                    escape(&d.alias),
                    format!("+{insertions}").green(),
                    format!("-{deletions}").red(),
                    files,
                )?;
            }
        }
        if total_repos_changed > 0 {
            outln!("  {}", "─".repeat(40))?;
            outln!(
                "  {:12} {} {} ({} files, {} repos)",
                "Total",
                format!("+{total_insertions}").green(),
                format!("-{total_deletions}").red(),
                total_files,
                total_repos_changed,
            )?;
        } else if failed == 0 {
            outln!("  No changes vs {}", escape(base_ref))?;
        }
        // the patch follows the summary, unless the summary is all that was asked for
        if !args.stat {
            for (d, r) in diff_entries.iter().zip(&repos) {
                if d.files_changed.unwrap_or(0) == 0 {
                    continue;
                }
                outln!("\n{}", format!("── {} ──", escape(&d.alias)).bold())?;
                // file contents: escaped like any other text from the repo
                outln!("{}", escape_lines(&git_diff_patch(&r.path, base_ref)?))?;
            }
        }
    }

    anyhow::ensure!(
        failed == 0,
        "Failed to diff {failed} repos against {}",
        escape(base_ref)
    );
    Ok(())
}
