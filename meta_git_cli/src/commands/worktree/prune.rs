use anyhow::Result;
use chrono::Utc;
use colored::*;
use meta_core::display::{escape, escape_path};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use meta_cli::worktree::discover_worktree_members;
use meta_git_lib::worktree::git_ops::git_worktree_remove;
use meta_git_lib::worktree::helpers::{resolve_worktree_root, Configs};
use meta_git_lib::worktree::hooks::fire_post_prune;
use meta_git_lib::worktree::lease;
use meta_git_lib::worktree::record::{self, SetRecord};
use meta_git_lib::worktree::safety::{check_worktree_removal, delete_checked_set, operation_at};
use meta_git_lib::worktree::types::*;
use meta_git_lib::worktree::workspace::Workspace;

use meta_core::ExecutionMode;

/// Helper to create a PruneEntry with consistent structure.
fn create_prune_entry(
    name: String,
    path: String,
    reason: impl Into<String>,
    age_seconds: Option<u64>,
) -> PruneEntry {
    PruneEntry {
        name,
        path,
        reason: reason.into(),
        age_seconds,
    }
}

/// Only a path that is really gone counts as missing. A dangling link or a
/// path prune can't inspect (permission denied) exists, so it gets checked
/// and kept, never forgotten.
fn is_missing(path: &Path) -> bool {
    matches!(path.symlink_metadata(), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// Every record naming each set path, with the admin directory it is in.
type Records = BTreeMap<PathBuf, Vec<(PathBuf, SetRecord)>>;

/// The sets meta created that have a record in a repository of
/// `workspace`. There is no global list, and the worktree directory may be
/// shared with other workspaces, so sets are found only through these
/// records. Records that can't be used are returned as kept entries, named
/// after their set when it is under `worktree_root`.
fn find_sets(workspace: &Workspace, worktree_root: &Path) -> (Records, Vec<PruneEntry>) {
    let mut found: Vec<(PathBuf, SetRecord)> = Vec::new();
    let mut unusable: Vec<PruneEntry> = Vec::new();
    // Records hold resolved paths
    let worktree_root = worktree_root
        .canonicalize()
        .unwrap_or_else(|_| worktree_root.to_path_buf());
    let mut not_used = |path: &Path, e: &anyhow::Error| {
        let path = set_of_admin(path, &worktree_root).unwrap_or_else(|| path.to_path_buf());
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        unusable.push(create_prune_entry(
            name.unwrap_or_default(),
            path.display().to_string(),
            format!("meta's records there can't be read: {e:#}"),
            None,
        ))
    };
    // Only some repositories can be searched: that is reported
    for (path, e) in &workspace.unreadable {
        not_used(path, e);
    }
    for git_dir in &workspace.repos {
        for (admin, record) in record::in_repo(git_dir) {
            match record {
                // A workspace sharing this repository made it: its set, not ours
                Ok(record) if !workspace.made_here(&record.project) => {}
                Ok(record) => found.push((admin, record)),
                Err(e) => not_used(&admin, &e),
            }
        }
    }
    let mut sets = Records::new();
    let mut seen = BTreeSet::new();
    for (admin, record) in found {
        if seen.insert(admin.clone()) {
            sets.entry(record.set.clone())
                .or_default()
                .push((admin, record));
        }
    }
    (sets, unusable)
}

/// The set under `worktree_root` holding the worktree whose admin directory
/// is `admin`, if git's link back to the worktree leads there.
fn set_of_admin(admin: &Path, worktree_root: &Path) -> Option<PathBuf> {
    let gitdir = std::fs::read_to_string(admin.join("gitdir")).ok()?;
    // git may write it relative to `admin` (worktree.useRelativePaths)
    let checkout = admin.join(gitdir.trim());
    let checkout = checkout.canonicalize().unwrap_or(checkout);
    let name = checkout
        .strip_prefix(worktree_root)
        .ok()?
        .components()
        .next()?;
    Some(worktree_root.join(name))
}

/// Why an existing set with `record` is due for pruning, if it is: the
/// reason and, for an expired TTL, its age. An ephemeral set lives only as
/// long as the run that made it holds its lease (see `lease`); prune never
/// asks about one whose lease is held. (A set whose repositories were
/// removed from the project, or whose project is gone, is no longer this
/// workspace's: prune leaves it.)
fn eligible(record: &SetRecord, now: i64) -> Option<(String, Option<u64>)> {
    if record.ephemeral {
        return Some(("ephemeral".to_string(), None));
    }
    // Check TTL expiration
    let remaining = record.ttl_remaining(now).filter(|r| *r <= 0)?;
    // Total age = configured TTL + seconds past expiry
    let age = record
        .ttl_seconds
        .unwrap_or(0)
        .saturating_add(remaining.unsigned_abs());
    Some(("ttl_expired".to_string(), Some(age)))
}

pub(crate) fn handle_prune(mode: ExecutionMode, json: bool, strict: bool) -> Result<()> {
    let dry_run = mode.is_preview();

    // Hooks come from the owner: inside a set being pruned, its own config
    // goes with it
    let cwd = std::env::current_dir()?;
    let configs = Configs::find(&cwd)?;
    let workspace = Workspace::of(&configs.here);
    let worktree_root = resolve_worktree_root(Some(&configs.owner), &cwd)?;
    let (sets, mut kept) = find_sets(&workspace, &worktree_root);
    // A set whose lease is held is being created, or is an ephemeral set
    // whose run goes on: prune leaves it. One whose creator is gone without
    // finishing it is pruned, recorded or not.
    let mut held = BTreeSet::new();
    let mut abandoned = BTreeMap::new();
    for found in lease::all(&workspace.repos) {
        match &found.lease {
            Some(lease) if !workspace.made_here(&lease.project) => {}
            Some(lease) if found.live => {
                held.insert(lease.set.clone());
            }
            Some(lease) => {
                abandoned.insert(lease.set.clone(), found);
            }
            // being written, or unreadable: which set it names is unknown
            None => {}
        }
    }
    // a lease left behind by a set that is gone too names nothing
    abandoned.retain(|set, found| {
        let stale =
            is_missing(set) && set.parent().is_some_and(is_plain_dir) && !sets.contains_key(set);
        if stale && !dry_run {
            let _ = found.delete();
        }
        !stale
    });
    if sets.is_empty() && kept.is_empty() && abandoned.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&PruneOutput {
                    removed: vec![],
                    kept: vec![],
                    dry_run,
                })?
            );
        } else {
            println!("No worktree sets of this workspace were found. Nothing to prune.");
        }
        return Ok(());
    }

    let now = Utc::now().timestamp();
    let mut to_remove: Vec<(PruneEntry, Option<SetRecord>)> = Vec::new();
    let mut gone_members = BTreeMap::new();
    for (set, found) in &abandoned {
        if is_missing(set) {
            continue;
        }
        let name = set
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        to_remove.push((
            create_prune_entry(
                name,
                set.display().to_string(),
                match found.lease.as_ref().is_some_and(|l| l.ephemeral) {
                    true => format!("ephemeral, the run that made it ({}) ended", found.holder()),
                    false => format!("abandoned while {} was creating it", found.holder()),
                },
                None,
            ),
            None,
        ));
    }

    for (set, records) in &sets {
        if held.contains(set) || (abandoned.contains_key(set) && !is_missing(set)) {
            continue;
        }
        let path_key = set.display().to_string();
        let Some((_, found)) = records.first() else {
            continue;
        };
        let name = found.name();

        // Check if path exists (orphaned detection). A set with a worktree
        // still checked out (moved elsewhere) is left alone, and so is one
        // whose worktrees folder is gone too (a drive that isn't mounted?)
        if is_missing(set) {
            if !set.parent().is_some_and(is_plain_dir) {
                continue;
            }
            if !records
                .iter()
                .all(|(admin, _)| record::checkout_gone(admin))
            {
                continue;
            }
            to_remove.push((
                create_prune_entry(name, path_key, "orphaned (missing directory)", None),
                Some(found.clone()),
            ));
            continue;
        }

        // A worktree of it whose folder was deleted by hand: git's
        // registration goes, so its name and branch can be used again. (Not
        // through a link, which may lead to a volume that isn't mounted.)
        for (admin, record) in records.iter().filter(|_| is_plain_dir(set)) {
            if let Some(checkout) = record::checkout(admin).filter(|_| record::checkout_gone(admin))
            {
                to_remove.push((
                    create_prune_entry(
                        name.clone(),
                        checkout.display().to_string(),
                        "a worktree of it whose folder was deleted",
                        None,
                    ),
                    Some(record.clone()),
                ));
                gone_members.insert(checkout, (admin.clone(), record.clone()));
            }
        }

        // The record the set's own worktrees hold decides, not an older one
        // left in a repository; the removal check must see this one again
        let members = discover_worktree_members(set).unwrap_or_default();
        let record = match record::find(set, members.iter().map(|m| &m.path)) {
            Some(record) => record,
            // None of its worktrees holds a record (or prune can't read
            // them). Records left by worktrees that are gone are git's to
            // prune; any other stands, and the check decides, as it does
            // for a path that now leads through a link.
            None if is_plain_dir(set)
                && records
                    .iter()
                    .all(|(admin, _)| record::checkout_gone(admin)) =>
            {
                continue
            }
            None => found.clone(),
        };

        if let Some((reason, age)) = eligible(&record, now) {
            to_remove.push((
                create_prune_entry(name, path_key, reason, age),
                Some(record),
            ));
        }
    }

    // Prune never discards work: a candidate whose directory holds anything
    // that deleting it would lose is kept, with the reason.
    let mut candidates = Vec::new();
    for (entry, record) in to_remove {
        let wt_path = Path::new(&entry.path);
        if is_missing(wt_path) {
            candidates.push((entry, None));
            continue;
        }
        if let Some(why) = path_moved(wt_path) {
            kept.push(create_prune_entry(
                entry.name,
                entry.path,
                format!("{}, but {why}", entry.reason),
                entry.age_seconds,
            ));
            continue;
        }
        // A set holding another set is blocked by the check, and one holding
        // another workspace's worktrees is that workspace's to prune.
        // In an abandoned set, worktrees not recorded yet are the creator's
        let is_abandoned = record.is_none();
        let checked = check_worktree_removal(wt_path).map(|check| {
            let problems: Vec<String> = check
                .blockers_but(is_abandoned)
                .chain(&check.unsaved)
                .cloned()
                .collect();
            (workspace.foreign_members(wt_path, &check), problems, check)
        });
        let problems = match checked {
            Ok((foreign, _, _)) if !foreign.is_empty() => foreign.join("; "),
            Ok((_, problems, check))
                if problems.is_empty() && (is_abandoned || check.record() == record.as_ref()) =>
            {
                candidates.push((entry, Some(check)));
                continue;
            }
            Ok((_, problems, check)) if problems.is_empty() && check.record().is_some() => {
                "its record changed after prune read it".to_string()
            }
            Ok((_, problems, _)) if problems.is_empty() => {
                "meta's record of it is gone".to_string()
            }
            Ok((_, problems, _)) => problems.join("; "),
            Err(e) => format!("cannot inspect it: {e:#}"),
        };
        kept.push(create_prune_entry(
            entry.name,
            entry.path,
            format!("{}, but {problems}", entry.reason),
            entry.age_seconds,
        ));
    }

    once_each(&mut kept);
    if dry_run || candidates.is_empty() {
        let would_remove: Vec<PruneEntry> = candidates.into_iter().map(|(e, _)| e).collect();
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&PruneOutput {
                    removed: would_remove,
                    kept: kept.clone(),
                    dry_run,
                })?
            );
        } else {
            if would_remove.is_empty() {
                println!("Nothing to prune.");
            } else {
                println!("[DRY RUN] Would prune {} worktree(s):", would_remove.len());
                for entry in &would_remove {
                    println!(
                        "  {} ({}) — {}",
                        escape(&entry.name),
                        escape(&entry.reason),
                        escape(&entry.path)
                    );
                }
            }
            print_kept(&kept);
        }
        warn_kept(&kept, strict)?;
        return Ok(());
    }

    // Actually remove: each checked set, or the records of a missing one.
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for (prune_entry, check) in candidates {
        let wt_path = Path::new(&prune_entry.path);
        let removal = match &check {
            // Checked again: a directory above may have become a link since
            Some(check) => match path_moved(wt_path) {
                None => delete_checked_set(wt_path, check, false, false).map(|()| None),
                Some(why) => {
                    kept.push(create_prune_entry(
                        prune_entry.name,
                        prune_entry.path,
                        format!("{}, but {why}", prune_entry.reason),
                        prune_entry.age_seconds,
                    ));
                    continue;
                }
            },
            // A missing directory only loses its records. One that has
            // reappeared since was never checked: keep it, nothing failed.
            None if !is_missing(wt_path) => {
                kept.push(create_prune_entry(
                    prune_entry.name,
                    prune_entry.path,
                    format!("{}, but it reappeared", prune_entry.reason),
                    prune_entry.age_seconds,
                ));
                continue;
            }
            None => match gone_members.get(wt_path) {
                Some((admin, record)) => drop_registration(admin, record),
                None => forget(wt_path, &sets),
            },
        };
        let removal = match removal {
            Ok(Some(why)) => {
                kept.push(create_prune_entry(
                    prune_entry.name,
                    prune_entry.path,
                    format!("{}, but {why}", prune_entry.reason),
                    prune_entry.age_seconds,
                ));
                continue;
            }
            Ok(None) => Ok(()),
            Err(e) => Err(e),
        };
        // its lease goes with it
        let removal = removal.and_then(|()| match abandoned.get(wt_path) {
            Some(found) => found.delete(),
            None => Ok(()),
        });
        match removal {
            Ok(()) => removed.push(prune_entry),
            Err(e) => {
                failed.push(prune_entry.name.clone());
                kept.push(create_prune_entry(
                    prune_entry.name,
                    prune_entry.path,
                    format!("{}, but removing it failed: {e:#}", prune_entry.reason),
                    prune_entry.age_seconds,
                ));
            }
        }
    }

    // Fire post-prune hook
    fire_post_prune(&removed, Some(&configs.owner));
    once_each(&mut kept);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&PruneOutput {
                removed,
                kept: kept.clone(),
                dry_run: false,
            })?
        );
    } else {
        println!("{} Pruned {} worktree(s):", "✓".green(), removed.len());
        for entry in &removed {
            println!(
                "  {} ({}) — {}",
                escape(&entry.name),
                escape(&entry.reason),
                escape(&entry.path)
            );
        }
        print_kept(&kept);
    }

    if !failed.is_empty() {
        anyhow::bail!("Failed to prune: {}", escape(&failed.join(", ")));
    }
    warn_kept(&kept, strict)
}

/// Drop the worktrees of a set whose directory is gone (see
/// [`drop_registration`]); why any are left registered.
fn forget(set: &Path, sets: &Records) -> Result<Option<String>> {
    let mut left = Vec::new();
    for (admin, record) in sets.get(set).into_iter().flatten() {
        left.extend(drop_registration(admin, record)?);
    }
    Ok((!left.is_empty()).then(|| left.join("; ")))
}

/// Have git drop its registration of the worktree whose admin directory is
/// `admin`, if its checkout is still gone and it still holds the record
/// prune read: git removes that one registration, and the record with it,
/// so the name and branch can be used again. Nothing else in the repository
/// is pruned. Left registered, and why: one with a git operation under way,
/// a locked one (on a drive that isn't mounted, say), and one whose
/// detached HEAD is a commit no branch or tag holds.
fn drop_registration(admin: &Path, record: &SetRecord) -> Result<Option<String>> {
    if !record::checkout_gone(admin) || record::read(admin)?.as_ref() != Some(record) {
        return Ok(None);
    }
    let (Some(checkout), Some(repo)) = (
        record::checkout(admin),
        admin.parent().and_then(Path::parent),
    ) else {
        anyhow::bail!("Cannot read git's entry {}", escape_path(admin));
    };
    if let Some(operation) = operation_at(admin) {
        return Ok(Some(format!(
            "{}: a {operation} is under way there, and git keeps its state (an autostash, say) \
             with the entry",
            escape_path(&checkout)
        )));
    }
    if admin.join("locked").symlink_metadata().is_ok() {
        return Ok(Some(format!(
            "git has {} locked (on a drive that isn't mounted?)",
            escape_path(&checkout)
        )));
    }
    let head = std::fs::read_to_string(admin.join("HEAD")).unwrap_or_default();
    let head = head.trim();
    if !head.is_empty() && !head.starts_with("ref:") {
        let holders = meta_core::git::run(
            repo,
            [
                "for-each-ref",
                "--contains",
                head,
                "--count=1",
                "--format=%(refname)",
            ],
        )?;
        if holders.trim().is_empty() {
            return Ok(Some(format!(
                "{}: its detached HEAD {} is on no branch or tag (if it was moved, run 'git \
                 worktree repair')",
                escape_path(&checkout),
                escape(head)
            )));
        }
    }
    git_worktree_remove(repo, &checkout, false).map(|()| None)
}

/// Whether `path` is a directory reached without following any link.
fn is_plain_dir(path: &Path) -> bool {
    path.symlink_metadata().is_ok_and(|m| m.is_dir()) && path_moved(path).is_none()
}

/// Why the recorded path no longer resolves to itself, if it doesn't: a
/// directory above it became a symlink, which could lead into another set,
/// or it can't be resolved at all.
/// Each record holds its set's resolved path from when it was created. A link at
/// the set directory itself is followed by the removal check; the set it leads
/// to is deleted only when its own record is the one prune read.
fn path_moved(path: &Path) -> Option<String> {
    match path.symlink_metadata() {
        Ok(meta) if !meta.file_type().is_symlink() => match path.canonicalize() {
            Ok(resolved) if resolved == path => None,
            Ok(_) => Some("a directory above it now leads elsewhere through a symlink".to_string()),
            Err(e) => Some(format!("its path cannot be resolved ({e})")),
        },
        _ => None,
    }
}

/// Keep the first entry for each path: a set kept for several reasons (an
/// unusable record in each of its worktrees, say) is listed once.
fn once_each(kept: &mut Vec<PruneEntry>) {
    let mut paths = BTreeSet::new();
    kept.retain(|e| paths.insert(e.path.clone()));
}

fn print_kept(kept: &[PruneEntry]) {
    if kept.is_empty() {
        return;
    }
    println!("Kept {} worktree(s):", kept.len());
    for entry in kept {
        println!(
            "  {} ({}) — {}",
            escape(&entry.name),
            escape(&entry.reason),
            escape(&entry.path)
        );
    }
}

/// Kept worktrees are a warning, and an error in strict mode.
fn warn_kept(kept: &[PruneEntry], strict: bool) -> Result<()> {
    if kept.is_empty() {
        return Ok(());
    }
    let names: Vec<_> = kept.iter().map(|e| escape(&e.name)).collect();
    super::warn_or_bail(strict, format!("Kept worktrees: {}", names.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_gitdir_finds_its_set() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap().join("wt");
        let checkout = root.join("broken/a");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::write(checkout.join(".git"), "").unwrap();
        let admin = tmp.path().join("repo/.git/worktrees/a");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("gitdir"), "../../../../wt/broken/a/.git\n").unwrap();
        assert_eq!(set_of_admin(&admin, &root), Some(root.join("broken")));
    }

    #[test]
    fn kept_names_are_escaped_on_one_line() {
        let kept = [create_prune_entry(
            "bad\nFORGED\u{1b}[2J".to_string(),
            String::new(),
            "",
            None,
        )];
        let message = warn_kept(&kept, true).unwrap_err().to_string();
        assert!(
            !message.contains('\n') && !message.contains('\u{1b}'),
            "{message}"
        );
    }

    // ── create_prune_entry ──────────────────────────────

    #[test]
    fn create_prune_entry_with_age() {
        let entry = create_prune_entry(
            "test-wt".to_string(),
            "/path/to/wt".to_string(),
            "ttl_expired",
            Some(3600),
        );
        assert_eq!(entry.name, "test-wt");
        assert_eq!(entry.path, "/path/to/wt");
        assert_eq!(entry.reason, "ttl_expired");
        assert_eq!(entry.age_seconds, Some(3600));
    }

    #[test]
    fn create_prune_entry_without_age() {
        let entry = create_prune_entry(
            "test-wt".to_string(),
            "/path/to/wt".to_string(),
            "orphaned (missing directory)",
            None,
        );
        assert_eq!(entry.name, "test-wt");
        assert_eq!(entry.reason, "orphaned (missing directory)");
        assert_eq!(entry.age_seconds, None);
    }

    #[test]
    fn create_prune_entry_converts_into_string() {
        let entry = create_prune_entry(
            "test-wt".to_string(),
            "/path/to/wt".to_string(),
            String::from("test reason"),
            None,
        );
        assert_eq!(entry.reason, "test reason");
    }
}
