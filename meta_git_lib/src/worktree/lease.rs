//! A lease names a worktree set while meta creates it, and an ephemeral set
//! for as long as the run it was made for lasts.
//!
//! A set's lease lives beside the set's folder, in the worktrees folder:
//! `.<set>.meta-lease`, written once an OS lock is held on
//! `.<set>.meta-lease.lock`. Its place depends only on the set's path, so
//! every command finds the same one whatever repositories the workspace has.
//! The creator takes it before it claims the set's folder and holds it until
//! it is done; then it deletes both. `remove` and `prune` hold the same lock
//! while they delete a set ([`hold`]), so no create of it starts meanwhile.
//! Whoever can take the lock knows the creator is gone, however it ended
//! (SIGKILL included), whatever its process id now names, on any machine
//! sharing the files. (The lock is on a file of its own: Windows enforces
//! locks, so a locked file can't be read.)
//!
//! A lease left behind marks a set that was never finished: its worktrees
//! are meta's even where meta hadn't recorded them yet, and it may be
//! removed, after the same checks as any set. Unlike a record (see
//! [`super::record`]), a lease isn't in a git directory: forging one takes
//! write access to the worktrees folder, which already lets anyone delete
//! the set (owner decision, 2026-10-31).

use anyhow::{Context, Result};
use meta_core::display::escape_path;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::{Path, PathBuf};

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

/// A lease this process holds. Dropping it deletes the lease, then lets go
/// of the lock (see [`Held::leave`]).
#[derive(Debug)]
pub struct Held {
    path: PathBuf,
    _lock: File,
    /// Let go of the lock but leave the lease, as a creator that is gone does.
    leave: bool,
}

impl Held {
    /// Let go of the lock and leave the lease where it is: the set is the
    /// next `prune`'s to remove (`create --ephemeral`).
    pub fn leave(mut self) {
        self.leave = true;
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if !self.leave {
            let _ = std::fs::remove_file(&self.path);
        }
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
pub fn hold(set: &Path) -> Result<(Option<Lease>, Claim)> {
    let path = path_for(set)?;
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
    let mut lock = path.as_os_str().to_owned();
    lock.push(".lock");
    PathBuf::from(lock)
}
/// The lock of the lease at `path`, taken (its file made if missing), or
/// `None` if another process holds it. A lock taken on a file that was
/// deleted meanwhile (its holder let go) is let go, and taken again on the
/// file now there.
fn lock(path: &Path) -> Result<Option<File>> {
    let lock = lock_path(path);
    loop {
        let file = match std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock)
        {
            Ok(file) => file,
            // Windows: its holder is deleting it, and still has it open
            Err(e) if cfg!(windows) && e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Ok(None)
            }
            Err(e) => return Err(e).with_context(|| format!("Cannot open {}", escape_path(&lock))),
        };
        // A reader tests the lock by taking it for a moment: try a few times
        // before taking it for held
        let mut locked = false;
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if taken(file.try_lock())
                .with_context(|| format!("Cannot lock {}", escape_path(&lock)))?
            {
                locked = true;
                break;
            }
        }
        if !locked {
            return Ok(None);
        }
        if still_at(&file, &lock) {
            return Ok(Some(file));
        }
    }
}

/// Whether a try at a lock took it: `false` when another process holds it.
/// Any other failure (no lock support on the filesystem, say) is an error,
/// never taken for a free or a held lease.
fn taken(attempt: Result<(), std::fs::TryLockError>) -> std::io::Result<bool> {
    match attempt {
        Ok(()) => Ok(true),
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
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

/// The suffix of a lease's file name.
const SUFFIX: &str = ".meta-lease";

/// The lease file of the set at `set`: `.<set>.meta-lease` beside it.
fn path_for(set: &Path) -> Result<PathBuf> {
    match (set.parent(), set.file_name()) {
        (Some(dir), Some(name)) => {
            let mut file = std::ffi::OsString::from(".");
            file.push(name);
            file.push(SUFFIX);
            Ok(dir.join(file))
        }
        _ => anyhow::bail!("{} has no folder to keep its lease in", escape_path(set)),
    }
}

/// Take the lease for creating `lease.set`. Fails if another creator holds
/// it; a lease left by a creator that is gone is taken over only if its set
/// is gone too (otherwise that set is a half-made one, for `remove`).
pub fn take(lease: &Lease) -> Result<Held> {
    let path = path_for(&lease.set)?;
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
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp.{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let written = serde_json::to_vec_pretty(lease)
        .map_err(std::io::Error::other)
        .and_then(|json| std::fs::write(&tmp, json))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.with_context(|| format!("Cannot write {}", escape_path(&path)))?;
    held().push(path.clone());
    Ok(Held {
        path,
        _lock: lock,
        leave: false,
    })
}

/// The lease at `path`: whether its lock is held, and what it says.
fn read(path: PathBuf) -> Found {
    let live = match File::open(lock_path(&path)) {
        // a lock that can't be tested counts as held
        Ok(file) => !matches!(taken(file.try_lock()), Ok(true)),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    };
    let lease = read_lease(&path);
    Found { path, lease, live }
}

/// Every lease in the worktrees folder `dir`.
pub fn all(dir: &Path) -> Vec<Found> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.') && name.ends_with(SUFFIX))
        })
        .map(read)
        .collect()
}

/// The lease of the set at `set` (resolved), if there is one. One that
/// can't be read counts.
pub fn find(set: &Path) -> Option<Found> {
    let path = path_for(set).ok()?;
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

    /// What a creator killed outright leaves: the lease, no lock held.
    fn left_behind(lease: &Lease) -> PathBuf {
        let path = path_for(&lease.set).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string(lease).unwrap()).unwrap();
        path
    }

    #[test]
    fn a_lease_sits_beside_its_set() {
        let set = Path::new("worktrees").join("a");
        let path = path_for(&set).unwrap();
        assert_eq!(path.parent(), set.parent());
        assert_eq!(path.file_name().unwrap(), ".a.meta-lease");
        assert_eq!(lock_path(&path).file_name().unwrap(), ".a.meta-lease.lock");
    }

    #[test]
    fn a_held_lease_is_live_until_dropped_then_gone() {
        let temp = tempfile::tempdir().unwrap();
        let set = temp.path().join("wt/a");
        std::fs::create_dir_all(set.parent().unwrap()).unwrap();
        let mine = lease(&set);
        let held = take(&mine).unwrap();
        let found = find(&set).unwrap();
        assert!(found.live && found.is_mine());
        assert_eq!(found.lease, Some(mine));
        // another creator, or a command deleting the set, is refused
        let refused = take(&lease(&set)).unwrap_err().to_string();
        assert!(refused.contains("is being created by pid"), "{refused}");
        let refused = hold(&set).unwrap_err().to_string();
        assert!(refused.contains("being created by pid"), "{refused}");
        drop(held);
        assert!(find(&set).is_none());
        assert!(all(set.parent().unwrap()).is_empty());
        assert_eq!(std::fs::read_dir(set.parent().unwrap()).unwrap().count(), 0);
    }

    #[test]
    fn a_lease_left_behind_is_dead_and_blocks_its_set_only_while_the_set_exists() {
        let temp = tempfile::tempdir().unwrap();
        let set = temp.path().join("wt/a");
        std::fs::create_dir_all(&set).unwrap();
        left_behind(&lease(&set));
        let found = find(&set).unwrap();
        // it names this process, which doesn't hold it: a pid says nothing
        assert!(!found.live && !found.is_mine());
        assert_eq!(all(set.parent().unwrap()).len(), 1);
        let refused = take(&lease(&set)).unwrap_err().to_string();
        assert!(refused.contains("left half-made"), "{refused}");
        // once the set is gone, a new creator takes it over
        std::fs::remove_dir(&set).unwrap();
        let held = take(&lease(&set)).unwrap();
        assert!(find(&set).unwrap().live);
        drop(held);
    }

    #[test]
    fn holding_a_set_for_deletion_keeps_creators_out_and_says_what_was_left() {
        let temp = tempfile::tempdir().unwrap();
        let set = temp.path().join("wt/a");
        std::fs::create_dir_all(set.parent().unwrap()).unwrap();
        // no lease: only the lock is held
        let (left, claim) = hold(&set).unwrap();
        assert!(left.is_none());
        let refused = take(&lease(&set)).unwrap_err().to_string();
        assert!(refused.contains("in use by another meta"), "{refused}");
        drop(claim);
        assert_eq!(std::fs::read_dir(set.parent().unwrap()).unwrap().count(), 0);
        // a lease left behind is handed over, and deleted with the set
        let old = lease(&set);
        let path = left_behind(&old);
        let (left, claim) = hold(&set).unwrap();
        assert_eq!(left, Some(old));
        claim.delete().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn a_lease_made_again_since_it_was_found_is_not_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let set = temp.path().join("wt/a");
        let path = left_behind(&lease(&set));
        let found = find(&set).unwrap();
        // another create of the set (same pid, another namespace) left its own
        left_behind(&lease(&set));
        found.delete().unwrap();
        assert!(path.exists());
        let found = find(&set).unwrap();
        found.delete().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn a_lease_left_on_purpose_is_dead_and_stays() {
        let temp = tempfile::tempdir().unwrap();
        let set = temp.path().join("wt/a");
        std::fs::create_dir_all(set.parent().unwrap()).unwrap();
        take(&lease(&set)).unwrap().leave();
        let found = find(&set).unwrap();
        assert!(!found.live && !found.is_mine());
        assert!(found.lease.is_some());
    }

    #[test]
    fn only_a_lock_another_process_holds_reads_as_held() {
        assert!(taken(Ok(())).unwrap());
        assert!(!taken(Err(std::fs::TryLockError::WouldBlock)).unwrap());
        let failed = std::io::Error::other("no locks on this filesystem");
        assert!(taken(Err(std::fs::TryLockError::Error(failed))).is_err());
    }
}
