use anyhow::Result;
use chrono::Utc;
use colored::*;
use meta_core::display::escape;
use meta_core::out;
use meta_core::outln;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use meta_cli::worktree::discover_worktree_members;
use meta_core::set_record::{self as record, SetRecord};
use meta_git_lib::worktree::helpers::{resolve_worktree_root, Configs};
use meta_git_lib::worktree::hooks::fire_post_prune;
use meta_git_lib::worktree::lease;
use meta_git_lib::worktree::safety::{check_worktree_removal, delete_checked_set};
use meta_git_lib::worktree::ttl;
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
/// reason and, for an expired TTL, its age. (An ephemeral set is pruned once
/// the run that made it is gone and left its lease beside it: see the
/// leases in `handle_prune`. No lease is no proof the run ended: `git clean`
/// may have deleted it. A set whose repositories were removed from the
/// project, or whose project is gone, is no longer this workspace's: prune
/// leaves it.)
///
/// A set whose TTL can't be read isn't due: an error.
fn eligible(record: &SetRecord, now: i64) -> Result<Option<(String, Option<u64>)>> {
    // Check TTL expiration
    let Some(remaining) = ttl::remaining(record, now)?.filter(|r| *r <= 0) else {
        return Ok(None);
    };
    // Total age = configured TTL + seconds past expiry
    let age = record
        .ttl_seconds
        .unwrap_or(0)
        .saturating_add(remaining.unsigned_abs());
    Ok(Some(("ttl_expired".to_string(), Some(age))))
}

pub(crate) fn handle_prune(mode: ExecutionMode, json: bool, strict: bool) -> Result<()> {
    let dry_run = mode.is_preview();

    // Hooks come from the owner: inside a set being pruned, its own config
    // goes with it
    let cwd = std::env::current_dir()?;
    let configs = Configs::find(&cwd)?;
    let workspace = Workspace::of(&configs.here)?;
    let worktree_root = resolve_worktree_root(Some(&configs.owner), &cwd)?;
    let (sets, mut kept) = find_sets(&workspace, &worktree_root);
    // A set whose lease is held is being created, or is an ephemeral set
    // whose run goes on: prune leaves it. One whose creator is gone without
    // finishing it is pruned, recorded or not.
    let mut held = BTreeSet::new();
    let mut abandoned = BTreeMap::new();
    // Leases sit beside their sets: in the worktrees folder, and beside each
    // set a record names
    let folders: BTreeSet<PathBuf> = std::iter::once(
        worktree_root
            .canonicalize()
            .unwrap_or_else(|_| worktree_root.clone()),
    )
    .chain(
        sets.keys()
            .filter_map(|set| set.parent().map(Path::to_path_buf)),
    )
    .collect();
    for found in folders.iter().flat_map(|folder| lease::all(folder)) {
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
    let now = Utc::now().timestamp();
    let mut to_remove: Vec<(Candidate, Option<SetRecord>)> = Vec::new();
    for (set, found) in &abandoned {
        if is_missing(set) {
            continue;
        }
        let name = set
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let reason = match found.lease.as_ref().is_some_and(|l| l.ephemeral) {
            true => format!("ephemeral, the run that made it ({}) ended", found.holder()),
            false => format!("abandoned while {} was creating it", found.holder()),
        };
        to_remove.push((Candidate::new(set, name, reason, None), None));
    }

    for (set, records) in &sets {
        if held.contains(set) || (abandoned.contains_key(set) && !is_missing(set)) {
            continue;
        }
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
            let orphan = Candidate::new(set, name, "orphaned (missing directory)", None);
            to_remove.push((orphan, Some(found.clone())));
            continue;
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

        match eligible(&record, now) {
            Ok(Some((reason, age))) => {
                to_remove.push((Candidate::new(set, name, reason, age), Some(record)));
            }
            Ok(None) => {}
            Err(e) => out::warn_or_bail(strict, format!("{e:#}"))?,
        }
    }

    // Prune never discards work: a candidate whose directory holds anything
    // that deleting it would lose is kept, with the reason.
    let mut candidates = Vec::new();
    for (candidate, record) in to_remove {
        let wt_path = candidate.set.as_path();
        if is_missing(wt_path) {
            candidates.push((candidate, None));
            continue;
        }
        if let Some(why) = path_moved(wt_path) {
            kept.push(candidate.kept(why));
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
                candidates.push((candidate, Some(check)));
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
        kept.push(candidate.kept(problems));
    }

    // Remove each checked set, or the records of a missing one; a preview
    // only says which
    let attempted = !dry_run && !candidates.is_empty();
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for (candidate, check) in candidates {
        if dry_run {
            removed.push(candidate.entry());
            continue;
        }
        let wt_path = candidate.set.as_path();
        let removal = match &check {
            // Checked again: a directory above may have become a link since
            Some(check) => match path_moved(wt_path) {
                // The set's lease, held through the deletion so no create of
                // it starts meanwhile. Refused if one is creating it or an
                // ephemeral run uses it; an abandoned set whose lease is
                // another's now was made again since.
                None => {
                    let held = lease::hold(wt_path);
                    let token = |lease: Option<&lease::Lease>| lease.map(|l| l.token.clone());
                    let why = match &held {
                        Err(e) => Some(format!("{e:#}")),
                        Ok((left, _)) => abandoned
                            .get(wt_path)
                            .filter(|found| token(found.lease.as_ref()) != token(left.as_ref()))
                            .map(|_| "it was made again since".to_string()),
                    };
                    match (why, held) {
                        (None, Ok((_, claim))) => delete_checked_set(wt_path, check, false, false)
                            .and_then(|()| claim.delete()),
                        (why, _) => {
                            kept.push(candidate.kept(why.unwrap_or_default()));
                            continue;
                        }
                    }
                }
                Some(why) => {
                    kept.push(candidate.kept(why));
                    continue;
                }
            },
            // A missing directory only loses its records. One that has
            // reappeared since was never checked: keep it, nothing failed.
            None if !is_missing(wt_path) => {
                kept.push(candidate.kept("it reappeared"));
                continue;
            }
            // its lease, if one was left, goes with it
            None => forget(wt_path, &sets).and_then(|()| {
                abandoned
                    .get(wt_path)
                    .map_or(Ok(()), |found| found.delete())
            }),
        };
        match removal {
            Ok(()) => removed.push(candidate.entry()),
            Err(e) => {
                failed.push(candidate.name.clone());
                kept.push(candidate.kept(format!("removing it failed: {e:#}")));
            }
        }
    }

    once_each(&mut kept);
    report(&removed, &kept, dry_run, json)?;
    // What was done is reported first: a failing hook doesn't undo it
    let hook = match attempted {
        true => fire_post_prune(&removed, Some(&configs.owner)),
        false => Ok(()),
    };
    // a removal that failed is the error; a failing hook then only warns
    if !failed.is_empty() {
        if let Err(e) = hook {
            out::warn(format!("{e:#}"));
        }
        anyhow::bail!("Failed to prune: {}", escape(&failed.join(", ")));
    }
    hook.or_else(|e| out::warn_or_bail(strict, format!("{e:#}")))?;
    warn_kept(&kept, strict)
}

/// A set prune may remove: where it is, and why.
struct Candidate {
    name: String,
    set: PathBuf,
    reason: String,
    age: Option<u64>,
}

impl Candidate {
    fn new(set: &Path, name: String, reason: impl Into<String>, age: Option<u64>) -> Self {
        Candidate {
            name,
            set: set.to_path_buf(),
            reason: reason.into(),
            age,
        }
    }

    /// How the report shows it.
    fn entry(&self) -> PruneEntry {
        create_prune_entry(
            self.name.clone(),
            self.set.display().to_string(),
            self.reason.clone(),
            self.age,
        )
    }

    /// How the report shows it kept, and why.
    fn kept(self, why: impl std::fmt::Display) -> PruneEntry {
        let reason = format!("{}, but {why}", self.reason);
        create_prune_entry(self.name, self.set.display().to_string(), reason, self.age)
    }
}

/// Print what prune removed (or, previewing, would remove) and what it kept.
fn report(removed: &[PruneEntry], kept: &[PruneEntry], dry_run: bool, json: bool) -> Result<()> {
    if json {
        let output = PruneOutput {
            removed: removed.to_vec(),
            kept: kept.to_vec(),
            dry_run,
        };
        outln!("{}", serde_json::to_string_pretty(&output)?)?;
        return Ok(());
    }
    match (removed.len(), dry_run) {
        (0, _) => outln!("Nothing to prune.")?,
        (n, true) => outln!("[DRY RUN] Would prune {n} worktree(s):")?,
        (n, false) => outln!("{} Pruned {n} worktree(s):", "✓".green())?,
    }
    for entry in removed {
        outln!(
            "  {} ({}) — {}",
            escape(&entry.name),
            escape(&entry.reason),
            escape(&entry.path)
        )?;
    }
    print_kept(kept)?;
    Ok(())
}

/// Delete the records of a set whose directory is gone, each only if its
/// worktree is still gone and it is still the record prune read. Git's own
/// entries for its worktrees stay: one can hold work its folder didn't, and
/// a moved worktree can still be repaired (`create` and `add` at that path
/// say how to drop one).
fn forget(set: &Path, sets: &Records) -> Result<()> {
    for (admin, record) in sets.get(set).into_iter().flatten() {
        if record::checkout_gone(admin) && record::read(admin)?.as_ref() == Some(record) {
            record::delete(admin)?;
        }
    }
    Ok(())
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

fn print_kept(kept: &[PruneEntry]) -> Result<()> {
    if kept.is_empty() {
        return Ok(());
    }
    outln!("Kept {} worktree(s):", kept.len())?;
    for entry in kept {
        outln!(
            "  {} ({}) — {}",
            escape(&entry.name),
            escape(&entry.reason),
            escape(&entry.path)
        )?;
    }
    Ok(())
}

/// Kept worktrees are a warning, and an error in strict mode.
fn warn_kept(kept: &[PruneEntry], strict: bool) -> Result<()> {
    if kept.is_empty() {
        return Ok(());
    }
    let names: Vec<_> = kept.iter().map(|e| escape(&e.name)).collect();
    out::warn_or_bail(strict, format!("Kept worktrees: {}", names.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use meta_test_support::Sandbox;

    #[test]
    fn a_relative_gitdir_finds_its_set() {
        let sandbox = Sandbox::new();
        let tmp = sandbox.work();
        let root = tmp.canonicalize().unwrap().join("wt");
        let checkout = root.join("broken/a");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::write(checkout.join(".git"), "").unwrap();
        let admin = tmp.join("repo/.git/worktrees/a");
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
