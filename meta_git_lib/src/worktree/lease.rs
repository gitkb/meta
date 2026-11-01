//! A lease names a worktree set while meta creates it, and an ephemeral set
//! for as long as the run it was made for lasts.
//!
//! Every set's lease lives in one place per workspace, the git directory of
//! its root repository (see [`home`]), in
//! `meta-worktree-creating/`: `<name>.lock`, which an OS lock is taken on,
//! and `<name>.json`, written once the lock is held, saying which set. The
//! creator takes it before it claims the set's directory and holds it until
//! it is done; then it deletes both. `remove` and `prune` hold the same lock
//! while they delete a set ([`hold`]), so no create of it starts meanwhile.
//! Whoever can take the lock knows the creator is gone, however it ended
//! (SIGKILL included), whatever its process id now names, on any machine
//! sharing the files. (The lock is on a file of its own: Windows enforces
//! locks, so a locked file can't be read.) A lease left behind marks a set
//! that was never finished: its worktrees are meta's even where meta hadn't
//! recorded them yet, and it may be removed.
//!
//! Like a record (see [`super::record`]), a lease is only in a repository's
//! git directory, so only something that can write there can make one.

use anyhow::{Context, Result};
use meta_core::display::escape_path;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::{Path, PathBuf};

/// The directory of leases in a repository's git directory.
pub const LEASE_DIR: &str = "meta-worktree-creating";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lease {
    /// The set directory, resolved.
    pub set: PathBuf,
    /// The meta directory the set is created from.
    pub project: PathBuf,
    /// The creating process, for messages only: whether it lives is the lock's
    /// to say.
    pub pid: u32,
    /// The set is ephemeral: its creator holds the lease for the whole run.
    #[serde(default)]
    pub ephemeral: bool,
    /// This lease and no other (two creators can have one process id, in
    /// separate pid namespaces).
    #[serde(default)]
    pub token: String,
}

impl Lease {
    /// A lease for this process creating `set` from `project`.
    pub fn new(set: PathBuf, project: PathBuf, ephemeral: bool) -> Lease {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        Lease {
            set,
            project,
            pid: std::process::id(),
            ephemeral,
            token: format!("{}-{nanos}", std::process::id()),
        }
    }
}

/// Where the workspace keeps its leases: the git directory of its root, or
/// where the root isn't a repository, the first of its repositories' (by
/// path, so every meta command picks the same).
pub fn home(workspace: &super::workspace::Workspace) -> Result<PathBuf> {
    if let Some(root) = meta_core::gitdir::common_dir(&workspace.root)? {
        return Ok(root);
    }
    workspace
        .repos
        .iter()
        .min()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("The workspace has no git repository to keep its leases in"))
}

/// A lease this process holds. Dropping it deletes the lease, then lets go
/// of the lock.
#[derive(Debug)]
pub struct Held {
    path: PathBuf,
    _lock: File,
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(lock_path(&self.path));
        held().retain(|path| *path != self.path);
    }
}

/// The leases this process holds. A process id doesn't say: another machine,
/// or another pid namespace, can have the same one.
fn held() -> std::sync::MutexGuard<'static, Vec<PathBuf>> {
    static HELD: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
    HELD.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A lease found on disk.
#[derive(Debug)]
pub struct Found {
    pub path: PathBuf,
    /// What it says; `None` if it can't be read.
    pub lease: Option<Lease>,
    /// Whether its creator holds it still. One whose lock can't be tested
    /// counts as held.
    pub live: bool,
}

impl Found {
    /// Whether this process holds it.
    pub fn is_mine(&self) -> bool {
        held().contains(&self.path)
    }

    /// Who holds it, for messages.
    pub fn holder(&self) -> String {
        match &self.lease {
            Some(lease) => format!("pid {}", lease.pid),
            None => "another meta".to_string(),
        }
    }

    /// What its holder is doing with the set, for messages.
    pub fn doing(&self) -> String {
        match &self.lease {
            Some(lease) if lease.ephemeral => {
                format!("in use by the ephemeral run of {}", self.holder())
            }
            Some(_) => format!("being created by {}", self.holder()),
            None => "in use by another meta".to_string(),
        }
    }

    /// Delete a lease whose creator is gone, if it is still the one found.
    pub fn delete(&self) -> Result<()> {
        let Some(lock) = lock(&self.path)? else {
            anyhow::bail!("{} is in use again", escape_path(&self.path));
        };
        let claim = Claim {
            path: self.path.clone(),
            _lock: lock,
        };
        match read_lease(&self.path) == self.lease {
            true => claim.delete(),
            false => Ok(()),
        }
    }
}

/// A set's lease, held by a command deleting the set (see [`hold`]): no
/// create of the set can start until it is dropped or deleted.
#[derive(Debug)]
pub struct Claim {
    path: PathBuf,
    _lock: File,
}

impl Claim {
    /// Delete the lease, if one was left.
    pub fn delete(self) -> Result<()> {
        remove(&self.path)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(lock_path(&self.path));
    }
}

/// Hold the lease of the set at `set` (resolved) while it is deleted. Fails
/// if its creator or ephemeral run still holds it. The lease its creator
/// left, if one was (the set is half-made, or its ephemeral run ended).
pub fn hold(home: &Path, set: &Path) -> Result<(Option<Lease>, Claim)> {
    let path = path_for(home, set);
    let dir = home.join(LEASE_DIR);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Cannot create {}", escape_path(&dir)))?;
    let Some(lock) = lock(&path)? else {
        anyhow::bail!("it is {}", read(path).doing());
    };
    let left = read_lease(&path);
    Ok((left, Claim { path, _lock: lock }))
}

fn read_lease(path: &Path) -> Option<Lease> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn remove(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("Cannot delete {}", escape_path(path)))
        }
        _ => Ok(()),
    }
}

/// The lock file of the lease at `path`.
fn lock_path(path: &Path) -> PathBuf {
    path.with_extension("lock")
}
/// The lock of the lease at `path`, taken (its file made if missing), or
/// `None` if another process holds it. A lock taken on a file that was
/// deleted meanwhile (its holder let go) is let go, and taken again on the
/// file now there.
fn lock(path: &Path) -> Result<Option<File>> {
    let lock = lock_path(path);
    loop {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock)
            .with_context(|| format!("Cannot open {}", escape_path(&lock)))?;
        // A reader tests the lock by taking it for a moment: try a few times
        // before taking it for held
        let locked = (0..3).any(|attempt| {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            file.try_lock().is_ok()
        });
        if !locked {
            return Ok(None);
        }
        if still_at(&file, &lock) {
            return Ok(Some(file));
        }
    }
}

/// Whether `file` is still the file at `path`.
#[cfg(unix)]
fn still_at(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (file.metadata(), path.symlink_metadata()) {
        (Ok(open), Ok(there)) => (open.dev(), open.ino()) == (there.dev(), there.ino()),
        _ => false,
    }
}

/// Whether `file` is still the file at `path` (by volume and file index).
#[cfg(not(unix))]
fn still_at(file: &File, path: &Path) -> bool {
    let open = file.try_clone().and_then(same_file::Handle::from_file);
    match (open, same_file::Handle::from_path(path)) {
        (Ok(open), Ok(there)) => open == there,
        _ => false,
    }
}

/// The lease file for `set` in the repository whose git directory is
/// `git_dir`: named after the set, plus a hash of its full path, as two
/// workspaces sharing a repository may each have a set of that name.
fn path_for(git_dir: &Path, set: &Path) -> PathBuf {
    // FNV-1a: stable across builds and platforms, unlike std's hasher
    let hash = set
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    let name = set
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    git_dir
        .join(LEASE_DIR)
        .join(format!("{name}-{hash:016x}.json"))
}

/// Take the lease for creating `lease.set`, in the repository whose git
/// directory is `git_dir`. Fails if another creator holds it; a lease left
/// by a creator that is gone is taken over only if its set is gone too
/// (otherwise that set is a half-made one, for `remove`).
pub fn take(git_dir: &Path, lease: &Lease) -> Result<Held> {
    let path = path_for(git_dir, &lease.set);
    let dir = git_dir.join(LEASE_DIR);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Cannot create {}", escape_path(&dir)))?;
    let Some(lock) = lock(&path)? else {
        anyhow::bail!(
            "Worktree set {} is {}",
            escape_path(&lease.set),
            read(path).doing()
        );
    };
    if path.symlink_metadata().is_ok() {
        // left by a creator that is gone
        if lease.set.symlink_metadata().is_ok() {
            anyhow::bail!(
                "Worktree set {} was left half-made by {}, which is gone; remove it with \
                 'meta git worktree remove'",
                escape_path(&lease.set),
                read(path).holder()
            );
        }
        remove(&path)?;
    }
    // Written whole, then put in place: a reader sees all of it or none
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    let written = serde_json::to_vec_pretty(lease)
        .map_err(std::io::Error::other)
        .and_then(|json| std::fs::write(&tmp, json))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.with_context(|| format!("Cannot write {}", escape_path(&path)))?;
    held().push(path.clone());
    Ok(Held { path, _lock: lock })
}

/// The lease at `path`: whether its lock is held, and what it says.
fn read(path: PathBuf) -> Found {
    let live = match File::open(lock_path(&path)) {
        Ok(file) => file.try_lock().is_err(),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    };
    let lease = read_lease(&path);
    Found { path, lease, live }
}

/// Every lease kept in `home`.
pub fn all(home: &Path) -> Vec<Found> {
    std::fs::read_dir(home.join(LEASE_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .map(read)
        .collect()
}

/// The lease of the set at `set` (resolved) kept in `home`, if there is one.
/// One that can't be read whose file is named for this set counts.
pub fn find(home: &Path, set: &Path) -> Option<Found> {
    let path = path_for(home, set);
    path.symlink_metadata()
        .is_ok()
        .then(|| read(path))
        .filter(|found| found.lease.as_ref().is_none_or(|lease| lease.set == set))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(set: &Path) -> Lease {
        Lease::new(set.to_path_buf(), PathBuf::from("/p"), false)
    }

    /// What a creator killed outright leaves: the JSON, no lock held.
    fn left_behind(home: &Path, lease: &Lease) -> PathBuf {
        let path = path_for(home, &lease.set);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string(lease).unwrap()).unwrap();
        path
    }

    #[test]
    fn a_held_lease_is_live_until_dropped_then_gone() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("git");
        let set = temp.path().join("wt/a");
        let mine = lease(&set);
        let held = take(&home, &mine).unwrap();
        let found = find(&home, &set).unwrap();
        assert!(found.live && found.is_mine());
        assert_eq!(found.lease, Some(mine));
        // another creator, or a command deleting the set, is refused
        let refused = take(&home, &lease(&set)).unwrap_err().to_string();
        assert!(refused.contains("is being created by pid"), "{refused}");
        let refused = hold(&home, &set).unwrap_err().to_string();
        assert!(refused.contains("being created by pid"), "{refused}");
        drop(held);
        assert!(find(&home, &set).is_none());
        assert!(all(&home).is_empty());
    }

    #[test]
    fn a_lease_left_behind_is_dead_and_blocks_its_set_only_while_the_set_exists() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("git");
        let set = temp.path().join("wt/a");
        std::fs::create_dir_all(&set).unwrap();
        left_behind(&home, &lease(&set));
        let found = find(&home, &set).unwrap();
        // it names this process, which doesn't hold it: a pid says nothing
        assert!(!found.live && !found.is_mine());
        let refused = take(&home, &lease(&set)).unwrap_err().to_string();
        assert!(refused.contains("left half-made"), "{refused}");
        // once the set is gone, a new creator takes it over
        std::fs::remove_dir(&set).unwrap();
        let held = take(&home, &lease(&set)).unwrap();
        assert!(find(&home, &set).unwrap().live);
        drop(held);
    }

    #[test]
    fn holding_a_set_for_deletion_keeps_creators_out_and_says_what_was_left() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("git");
        let set = temp.path().join("wt/a");
        // no lease: only the lock is held
        let (left, claim) = hold(&home, &set).unwrap();
        assert!(left.is_none());
        let refused = take(&home, &lease(&set)).unwrap_err().to_string();
        assert!(refused.contains("in use by another meta"), "{refused}");
        drop(claim);
        assert!(all(&home).is_empty());
        // a lease left behind is handed over, and deleted with the set
        let old = lease(&set);
        let path = left_behind(&home, &old);
        let (left, claim) = hold(&home, &set).unwrap();
        assert_eq!(left, Some(old));
        claim.delete().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn a_lease_made_again_since_it_was_found_is_not_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("git");
        let set = temp.path().join("wt/a");
        let path = left_behind(&home, &lease(&set));
        let found = find(&home, &set).unwrap();
        // another create of the set (same pid, another namespace) left its own
        left_behind(&home, &lease(&set));
        found.delete().unwrap();
        assert!(path.exists());
        let found = find(&home, &set).unwrap();
        found.delete().unwrap();
        assert!(!path.exists());
    }
}
