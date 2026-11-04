use anyhow::{Context, Result};
use chrono::Utc;
use colored::*;
use meta_plugin::display::{escape, escape_path};
use meta_plugin::outln;
use meta_plugin::{Ctx, ProjectResult, Request, Response};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;

use meta_core::duration::format_duration;
use meta_core::set_record::{self as record, SetRecord};
use meta_core::worktree::{
    detect_worktree_location, discover_worktree_members, discover_worktree_members_strict,
    discover_worktree_repos, discover_worktree_repos_strict, WorktreeMemberInfo,
};
use meta_git_lib::worktree::git_ops::repo_status;
use meta_git_lib::worktree::ttl;
use meta_git_lib::worktree::types::ListRepoEntry;

use super::cli_types::ListArgs;

/// One set, as listed: the record of each set (`meta --json git worktree
/// list` gives one row per set).
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
        let members = if args.strict {
            discover_worktree_members_strict(path)?
        } else {
            discover_worktree_members(path)?
        };
        let record = record::find(path, members.iter().map(|m| &m.path));
        return Ok((
            members.into_iter().map(RepoEntry::Metadata).collect(),
            record,
        ));
    }
    let repos = if args.strict {
        discover_worktree_repos_strict(path)?
    } else {
        discover_worktree_repos(path)?
    };
    let record = record::find(path, repos.iter().map(|r| &r.path));
    let entries = repos
        .par_iter()
        .map(|repo| {
            let status = repo_status(&repo.path);
            let status = if args.strict || selected {
                Some(status?)
            } else {
                status.ok()
            };
            Ok(RepoEntry::Observed(ListRepoEntry {
                alias: repo.alias.clone(),
                branch: status.as_ref().map(|_| repo.branch.clone()),
                dirty: status.map(|s| s.is_dirty()),
            }))
        })
        .collect::<Result<_>>()?;
    Ok((entries, record))
}

/// A selected set fails closed. All-set listing retains healthy sets and warnings.
pub(crate) fn handle_list(args: ListArgs, ctx: &Ctx, request: &Request) -> Result<Response> {
    let json = request.meta.json;
    let cwd = request.cwd.canonicalize()?;
    let location = detect_worktree_location(&cwd)?;
    let worktree_root = match (&location, std::env::var_os("META_WORKTREES")) {
        (Some((_, root)), None) => root
            .parent()
            .context("The worktree set has no parent folder")?
            .to_path_buf(),
        _ => {
            let configs = super::configs(request)?;
            let root = meta_core::worktree::resolve_worktree_root(
                configs.as_ref().map(|c| &c.owner),
                &cwd,
            )?;
            if root.is_absolute() {
                root
            } else {
                cwd.join(root)
            }
        }
    };
    let selected = match args.name.as_ref() {
        Some(name) => {
            meta_core::worktree::validate_name(name)?;
            let path = worktree_root.join(name);
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
        meta_core::worktree::validate_name(name)?;
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
                    Err(error) if args.strict || args.metadata_only => return Err(error.into()),
                    Err(_) => continue,
                };
                match entry.file_type() {
                    // a set folder that is a link counts as the folder it leads to
                    Ok(kind) if kind.is_dir() || kind.is_symlink() && entry.path().is_dir() => sets
                        .push((
                            entry.file_name().to_string_lossy().into_owned(),
                            entry.path(),
                        )),
                    Err(error) if args.strict || args.metadata_only => return Err(error.into()),
                    _ => {}
                }
            }
        }
        sets
    };
    let now = Utc::now().timestamp();
    // (with a warning found on the way, said once all are read)
    type Listed = Option<(ListEntry, Option<String>)>;
    let results: Vec<Result<Listed>> = sets
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
            let mut warning = None;
            let (ephemeral, ttl_remaining_seconds, custom) = match record {
                Some(record) => (
                    Some(record.ephemeral),
                    ttl::remaining(&record, now).unwrap_or_else(|e| {
                        warning = Some(format!("{e:#}"));
                        None
                    }),
                    (!record.custom.is_empty()).then_some(record.custom),
                ),
                None => (None, None, None),
            };
            let entry = ListEntry {
                name: name.clone(),
                root: path.display().to_string(),
                has_meta_root,
                repos,
                ephemeral,
                ttl_remaining_seconds,
                custom,
            };
            Ok(Some((entry, warning)))
        })
        .collect();
    let mut worktrees = Vec::new();
    // (name, message)
    let mut warnings = Vec::new();
    for ((name, _), result) in sets.iter().zip(results) {
        match result {
            Ok(Some((entry, warning))) => {
                if let Some(warning) = warning {
                    ctx.warn(warning);
                }
                worktrees.push(entry);
            }
            Ok(None) => {}
            // a set that can't be read is named with why, the others listed
            Err(error) if !is_selected => warnings.push((name.clone(), format!("{error:#}"))),
            Err(error) => return Err(error),
        }
    }
    worktrees.sort_by(|a, b| a.name.cmp(&b.name));
    warnings.sort();
    for (name, message) in &warnings {
        ctx.warn(format!("{}: {}", escape(name), escape(message)));
    }
    let records = |worktrees: &[ListEntry]| -> Vec<ProjectResult> {
        worktrees
            .iter()
            .map(|entry| {
                ProjectResult::about_workspace(entry).at(std::path::Path::new(&entry.root))
            })
            .collect()
    };
    if json {
        return Ok(Response::printed(records(&worktrees)));
    }
    let listed = records(&worktrees);
    if worktrees.is_empty() {
        outln!(
            "{}",
            if warnings.is_empty() {
                "No worktrees found."
            } else {
                "No readable worktree sets found."
            }
        )?;
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
        outln!("{header}")?;
        for repo in entry.repos {
            match repo {
                RepoEntry::Metadata(repo) => {
                    outln!("  {}  {}", escape(&repo.alias), escape_path(&repo.path))?
                }
                RepoEntry::Observed(repo) => {
                    let status = match repo.dirty {
                        Some(true) => "modified".yellow().to_string(),
                        Some(false) => "clean".green().to_string(),
                        None => "unknown: git can't read it".red().to_string(),
                    };
                    outln!(
                        "  {:12} -> {:20} ({})",
                        escape(&repo.alias),
                        escape(repo.branch.as_deref().unwrap_or("-")),
                        status
                    )?;
                }
            }
        }
        outln!()?;
    }
    Ok(Response::printed(listed))
}
