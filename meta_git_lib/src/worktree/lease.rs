//! A lease names a worktree set while meta creates it, and an ephemeral set
//! for as long as the run it was made for lasts.
//!
//! The creator takes the lease before it claims the set's directory, in the
//! git directory of the first repository the set takes a worktree of
//! (`<git dir>/meta-worktree-creating/`): it locks `<name>.lock` (an OS
//! lock), then writes `<name>.json` saying which set, and holds the lock
//! until it is done; then it deletes both. Whoever can take the lock knows
//! the creator is gone, however it ended (SIGKILL included), whatever its
//! process id now names, on any machine sharing the files. (The lock is on a
//! file of its own: Windows enforces locks, so a locked file can't be read.)
//! A lease left behind marks a set that was never finished: its worktrees
//! are meta's even where meta hadn't recorded them yet, and it may be
//! removed.
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

    /// What its holder is doing with the set, for messages: creating it, or
    /// running a command in it (an ephemeral set).
    pub fn doing(&self) -> String {
        match &self.lease {
            Some(lease) if lease.ephemeral => {
                format!("in use by the ephemeral run of {}", self.holder())
            }
            _ => format!("being created by {}", self.holder()),
        }
    }

    /// Delete a lease whose creator is gone. Taking the lock first keeps it
    /// from deleting one a new creator has just taken.
    pub fn delete(&self) -> Result<()> {
        let Some(_lock) = lock(&self.path)? else {
            anyhow::bail!("{} is in use again", escape_path(&self.path));
        };
        remove(&self.path)?;
        remove(&lock_path(&self.path))
    }
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

/// Whether `file` is still the file at `path`. Windows doesn't delete a file
/// that is open (meta opens it to lock it) until the last handle closes.
#[cfg(not(unix))]
fn still_at(_file: &File, path: &Path) -> bool {
    path.symlink_metadata().is_ok()
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
    let lease = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    Found { path, lease, live }
}

/// Every lease in the repositories whose git directories are `git_dirs`.
pub fn all(git_dirs: &[PathBuf]) -> Vec<Found> {
    git_dirs
        .iter()
        .filter_map(|git_dir| std::fs::read_dir(git_dir.join(LEASE_DIR)).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .map(read)
        .collect()
}

/// The lease naming the set at `set` (resolved), if any of `git_dirs` holds
/// one. A lease that can't be read whose file is named for this set counts.
pub fn find(git_dirs: &[PathBuf], set: &Path) -> Option<Found> {
    git_dirs
        .iter()
        .map(|git_dir| path_for(git_dir, set))
        .find(|path| path.symlink_metadata().is_ok())
        .map(read)
        .filter(|found| found.lease.as_ref().is_none_or(|lease| lease.set == set))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(set: &Path) -> Lease {
        Lease {
            set: set.to_path_buf(),
            project: PathBuf::from("/p"),
            pid: std::process::id(),
            ephemeral: false,
        }
    }

    #[test]
    fn a_held_lease_is_live_until_dropped_then_gone() {
        let temp = tempfile::tempdir().unwrap();
        let git_dir = temp.path().join("git");
        let set = temp.path().join("wt/a");
        let held = take(&git_dir, &lease(&set)).unwrap();
        let found = find(std::slice::from_ref(&git_dir), &set).unwrap();
        assert!(found.live && found.is_mine());
        assert_eq!(found.lease, Some(lease(&set)));
        // another creator is refused while it is held
        let refused = take(&git_dir, &lease(&set)).unwrap_err().to_string();
        assert!(refused.contains("is being created by pid"), "{refused}");
        drop(held);
        assert!(find(std::slice::from_ref(&git_dir), &set).is_none());
        assert!(all(std::slice::from_ref(&git_dir)).is_empty());
    }

    #[test]
    fn a_lease_left_behind_is_dead_and_blocks_its_set_only_while_the_set_exists() {
        let temp = tempfile::tempdir().unwrap();
        let git_dir = temp.path().join("git");
        let set = temp.path().join("wt/a");
        std::fs::create_dir_all(&set).unwrap();
        // what a creator killed outright leaves: the file, no lock
        let path = path_for(&git_dir, &set);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string(&lease(&set)).unwrap()).unwrap();
        let found = find(std::slice::from_ref(&git_dir), &set).unwrap();
        // it names this process, which doesn't hold it: a pid says nothing
        assert!(!found.live && !found.is_mine());
        let refused = take(&git_dir, &lease(&set)).unwrap_err().to_string();
        assert!(refused.contains("left half-made"), "{refused}");
        // once the set is gone, a new creator takes it over
        std::fs::remove_dir(&set).unwrap();
        let held = take(&git_dir, &lease(&set)).unwrap();
        assert!(find(std::slice::from_ref(&git_dir), &set).unwrap().live);
        drop(held);
    }

    #[test]
    fn deleting_a_dead_lease_leaves_a_live_one() {
        let temp = tempfile::tempdir().unwrap();
        let git_dir = temp.path().join("git");
        let set = temp.path().join("wt/a");
        let held = take(&git_dir, &lease(&set)).unwrap();
        let found = find(std::slice::from_ref(&git_dir), &set).unwrap();
        assert!(found.delete().is_err());
        assert!(found.path.exists());
        drop(held);
    }
}
