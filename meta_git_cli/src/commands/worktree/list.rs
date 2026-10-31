use anyhow::Result;
use chrono::Utc;
use colored::*;
use meta_core::display::{escape, escape_path};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;

use meta_cli::worktree::{
    detect_worktree_location, discover_worktree_members, discover_worktree_members_recursive,
    discover_worktree_repos, discover_worktree_repos_recursive, WorktreeMemberInfo,
};
use meta_core::duration::format_duration;
use meta_git_lib::worktree::git_ops::repo_status;
use meta_git_lib::worktree::helpers::*;
use meta_git_lib::worktree::record::{self, SetRecord};
use meta_git_lib::worktree::types::ListRepoEntry;

use super::cli_types::ListArgs;

#[derive(Serialize)]
struct ListWarning {
    name: String,
    root: String,
    message: String,
}

#[derive(Serialize)]
struct ListOutput {
    worktrees: Vec<ListEntry>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<ListWarning>,
}

#[derive(Serialize)]
struct ListEntry {
    name: String,
    root: String,
    has_meta_root: bool,
    repos: Vec<RepoEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ephemeral: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttl_remaining_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    custom: Option<HashMap<String, String>>,
}

/// Keep the existing observed JSON shape; metadata-only deliberately omits Git facts.
#[derive(Serialize)]
#[serde(untagged)]
enum RepoEntry {
    Observed(ListRepoEntry),
    Metadata(WorktreeMemberInfo),
}

/// The set's repos, and meta's record of creating it if it has one.
fn repo_entries(
    path: &std::path::Path,
    args: &ListArgs,
    selected: bool,
) -> Result<(Vec<RepoEntry>, Option<SetRecord>)> {
    if args.metadata_only {
        let members = if args.recursive {
            discover_worktree_members_recursive(path)?
        } else {
            discover_worktree_members(path)?
        };
        let record = record::find(path, members.iter().map(|m| &m.path));
        return Ok((
            members.into_iter().map(RepoEntry::Metadata).collect(),
            record,
        ));
    }
    let repos = if args.recursive {
        discover_worktree_repos_recursive(path)?
    } else if selected {
        discover_worktree_repos(path)?
    } else {
        discover_worktree_repos(path).unwrap_or_default()
    };
    let record = record::find(path, repos.iter().map(|r| &r.path));
    let entries = repos
        .par_iter()
        .map(|repo| {
            let status = repo_status(&repo.path);
            let dirty = if args.recursive || selected {
                status?.is_dirty()
            } else {
                status.is_ok_and(|s| s.is_dirty())
            };
            Ok(RepoEntry::Observed(ListRepoEntry {
                alias: repo.alias.clone(),
                branch: repo.branch.clone(),
                dirty,
            }))
        })
        .collect::<Result<_>>()?;
    Ok((entries, record))
}

/// A selected set fails closed. All-set listing retains healthy sets and warnings.
pub(crate) fn handle_list(args: ListArgs, json: bool, cwd: &std::path::Path) -> Result<()> {
    let cwd = cwd.canonicalize()?;
    let location = detect_worktree_location(&cwd);
    let worktree_root = match (&location, std::env::var_os("META_WORKTREES")) {
        (Some((_, root)), None) => root
            .parent()
            .expect("worktree location has a parent")
            .to_path_buf(),
        _ => {
            let configs = Configs::find_optional(&cwd)?;
            let root = resolve_worktree_root(configs.as_ref().map(|c| &c.owner), &cwd)?;
            if root.is_absolute() {
                root
            } else {
                cwd.join(root)
            }
        }
    };
    let selected = match args.name.as_ref() {
        Some(name) => {
            validate_worktree_name(name)?;
            let path = worktree_set_dir(&worktree_root, name)?;
            anyhow::ensure!(
                path.is_dir(),
                "Worktree '{name}' not found at {}",
                escape_path(&path)
            );
            Some((name.clone(), path))
        }
        None => location,
    };
    if let Some((name, _)) = &selected {
        validate_worktree_name(name)?;
    }
    let is_selected = selected.is_some();
    let sets = if let Some(selected) = selected {
        vec![selected]
    } else {
        let root = worktree_root;
        let mut sets = Vec::new();
        if root.exists() {
            for entry in std::fs::read_dir(root)? {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) if args.recursive || args.metadata_only => return Err(error.into()),
                    Err(_) => continue,
                };
                match entry.file_type() {
                    Ok(kind) if kind.is_dir() => sets.push((
                        entry.file_name().to_string_lossy().into_owned(),
                        entry.path(),
                    )),
                    Err(error) if args.recursive || args.metadata_only => return Err(error.into()),
                    _ => {}
                }
            }
        }
        sets
    };
    let now = Utc::now().timestamp();
    let results: Vec<Result<Option<ListEntry>>> = sets
        .par_iter()
        .map(|(name, path)| {
            let (repos, record) = repo_entries(path, &args, is_selected)?;
            if repos.is_empty() {
                anyhow::ensure!(!is_selected, "No repos found in worktree '{name}'");
                return Ok(None);
            }
            let has_meta_root = repos.iter().any(|repo| match repo {
                RepoEntry::Observed(repo) => repo.alias == ".",
                RepoEntry::Metadata(repo) => repo.alias == ".",
            });
            let (ephemeral, ttl_remaining_seconds, custom) = match record {
                Some(record) => (
                    Some(record.ephemeral),
                    record.ttl_remaining(now),
                    (!record.custom.is_empty()).then_some(record.custom),
                ),
                None => (None, None, None),
            };
            Ok(Some(ListEntry {
                name: name.clone(),
                root: path.display().to_string(),
                has_meta_root,
                repos,
                ephemeral,
                ttl_remaining_seconds,
                custom,
            }))
        })
        .collect();
    let mut worktrees = Vec::new();
    let mut warnings = Vec::new();
    for ((name, path), result) in sets.iter().zip(results) {
        match result {
            Ok(Some(entry)) => worktrees.push(entry),
            Ok(None) => {}
            Err(error) if !is_selected && (args.recursive || args.metadata_only) => {
                warnings.push(ListWarning {
                    name: name.clone(),
                    root: path.display().to_string(),
                    message: format!("{error:#}"),
                })
            }
            Err(error) => return Err(error),
        }
    }
    worktrees.sort_by(|a, b| a.name.cmp(&b.name));
    warnings.sort_by(|a, b| a.name.cmp(&b.name));
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ListOutput {
                worktrees,
                warnings
            })?
        );
        return Ok(());
    }
    if worktrees.is_empty() {
        println!(
            "{}",
            if warnings.is_empty() {
                "No worktrees found."
            } else {
                "No readable worktree sets found."
            }
        );
    }
    for entry in worktrees {
        let mut header = escape(&entry.name).bold().to_string();
        if entry.ephemeral == Some(true) {
            header.push_str(&format!(" {}", "[ephemeral]".dimmed()));
        }
        if let Some(ttl) = entry.ttl_remaining_seconds {
            header.push_str(&format!(
                " {}",
                if ttl > 0 {
                    format!("[TTL: {}]", format_duration(ttl))
                        .dimmed()
                        .to_string()
                } else {
                    "[expired]".red().to_string()
                }
            ));
        }
        println!("{header}");
        for repo in entry.repos {
            match repo {
                RepoEntry::Metadata(repo) => {
                    println!("  {}  {}", escape(&repo.alias), escape_path(&repo.path))
                }
                RepoEntry::Observed(repo) => {
                    let status = if repo.dirty {
                        "modified".yellow().to_string()
                    } else {
                        "clean".green().to_string()
                    };
                    println!(
                        "  {:12} -> {:20} ({})",
                        escape(&repo.alias),
                        escape(&repo.branch),
                        status
                    );
                }
            }
        }
        println!();
    }
    for warning in warnings {
        eprintln!(
            "Warning: {}: {}",
            escape(&warning.name),
            escape(&warning.message)
        );
    }
    Ok(())
}
