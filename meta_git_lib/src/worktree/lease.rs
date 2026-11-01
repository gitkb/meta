//! A lease names a worktree set while meta creates it, and an ephemeral set
//! for as long as the run it was made for lasts.
//!
//! The creator writes the lease before it claims the set's directory, in the
//! git directory of the first repository the set takes a worktree of
//! (`<git dir>/meta-worktree-creating/`), and holds an OS lock on it until
//! it is done; then it deletes the lease. Whoever can take the lock knows the
//! creator is gone, however it ended (SIGKILL included), whatever its process
//! id now names, on any machine sharing the files. A lease that is left
//! behind marks a set that was never finished: its worktrees are meta's
//! even where meta hadn't recorded them yet, and it may be removed.
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
    _file: File,
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A lease found on disk.
#[derive(Debug)]
pub struct Found {
    pub path: PathBuf,
    /// What it says; `None` if it can't be read (still being written, say).
    pub lease: Option<Lease>,
    /// Whether its creator holds it still. One whose lock can't be tested
    /// counts as held.
    pub live: bool,
}

impl Found {
    /// Whether this process holds it.
    pub fn is_mine(&self) -> bool {
        self.lease
            .as_ref()
            .is_some_and(|lease| lease.pid == std::process::id())
    }

    /// Who holds it, for messages.
    pub fn holder(&self) -> String {
        match &self.lease {
            Some(lease) => format!("pid {}", lease.pid),
            None => "another meta".to_string(),
        }
    }

    /// Delete a lease whose creator is gone. Taking the lock first keeps it
    /// from deleting one a new creator has just taken.
    pub fn delete(&self) -> Result<()> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(e).with_context(|| format!("Cannot open {}", escape_path(&self.path)))
            }
        };
        if file.try_lock().is_err() {
            anyhow::bail!("{} is in use again", escape_path(&self.path));
        }
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).with_context(|| format!("Cannot delete {}", escape_path(&self.path)))
            }
            _ => Ok(()),
        }
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
    loop {
        match File::create_new(&path) {
            Ok(file) => {
                file.try_lock()
                    .with_context(|| format!("Cannot lock {}", escape_path(&path)))?;
                let held = Held { path, _file: file };
                let json = serde_json::to_string_pretty(lease)?;
                std::io::Write::write_all(&mut &held._file, json.as_bytes())
                    .with_context(|| format!("Cannot write {}", escape_path(&held.path)))?;
                return Ok(held);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let found = read(path.clone());
                if found.live {
                    anyhow::bail!(
                        "Worktree set {} is being created by {}",
                        escape_path(&lease.set),
                        found.holder()
                    );
                }
                if lease.set.symlink_metadata().is_ok() {
                    anyhow::bail!(
                        "Worktree set {} was left half-made by {}, which is gone; remove it \
                         with 'meta git worktree remove'",
                        escape_path(&lease.set),
                        found.holder()
                    );
                }
                // left by a creator that is gone, of a set that is gone
                found.delete()?;
            }
            Err(e) => {
                return Err(e).with_context(|| format!("Cannot create {}", escape_path(&path)))
            }
        }
    }
}

/// The lease at `path`, read after its lock is tested: a creator writes it
/// only once it holds the lock.
fn read(path: PathBuf) -> Found {
    let live = match File::open(&path) {
        Ok(file) => !matches!(file.try_lock(), Ok(())),
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
        assert!(!found.live);
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
