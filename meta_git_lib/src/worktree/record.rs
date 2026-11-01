//! What meta records about a worktree set it created.
//!
//! There is no global list. Each linked worktree meta creates gets a record
//! in git's own admin directory for that worktree
//! (`<repo git dir>/worktrees/<id>/meta-worktree.json`). Git ignores the
//! file and deletes it with the worktree (`git worktree remove` or `prune`).
//! Only something that can write the repository's git directory can make
//! one, so a record is meta's proof that it made the set the record names.

use anyhow::{Context, Result};
use meta_core::display::escape_path;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The record's file name inside a worktree's admin directory.
pub const RECORD_FILE: &str = meta_core::config::WORKTREE_RECORD_FILE;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetRecord {
    /// The set directory, resolved when it was created.
    pub set: PathBuf,
    /// The meta directory the set was created from.
    pub project: PathBuf,
    pub created_at: String,
    #[serde(default)]
    pub ephemeral: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub custom: HashMap<String, String>,
}

impl SetRecord {
    /// The set's name: its directory's name.
    pub fn name(&self) -> String {
        self.set
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Seconds until the TTL expires: `None` without a TTL, negative once
    /// expired. A malformed `created_at` warns and never expires.
    pub fn ttl_remaining(&self, now_epoch: i64) -> Option<i64> {
        self.ttl_seconds.map(|ttl| {
            let created = match chrono::DateTime::parse_from_rfc3339(&self.created_at) {
                Ok(dt) => dt.timestamp(),
                Err(e) => {
                    log::warn!(
                        "Malformed created_at '{}' in the record of '{}': {}",
                        meta_core::display::escape(&self.created_at),
                        escape_path(&self.set),
                        e
                    );
                    return i64::MAX;
                }
            };
            let ttl = i64::try_from(ttl).unwrap_or(i64::MAX);
            created.saturating_add(ttl).saturating_sub(now_epoch)
        })
    }
}

/// The record in a worktree's admin directory, if it has one.
pub fn read(admin: &Path) -> Result<Option<SetRecord>> {
    let path = admin.join(RECORD_FILE);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("Cannot read {}", escape_path(&path))),
    };
    serde_json::from_str(&content)
        .map(Some)
        .with_context(|| format!("Cannot parse {}", escape_path(&path)))
}

/// Record `record` in a worktree's admin directory, atomically (a temp file
/// renamed into place). The directory is git's: it must exist, and it
/// carries the repository's permissions, not those of meta's data dir.
pub fn write(admin: &Path, record: &SetRecord) -> Result<()> {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let path = admin.join(RECORD_FILE);
    let tmp = admin.join(format!(
        "{RECORD_FILE}.tmp.{}.{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let json = serde_json::to_string_pretty(record)?;
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut file| std::io::Write::write_all(&mut file, json.as_bytes()))
        .and_then(|()| std::fs::rename(&tmp, &path));
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.with_context(|| format!("Cannot write {}", escape_path(&path)))
}

/// Delete the record in a worktree's admin directory, if any.
pub fn delete(admin: &Path) -> Result<()> {
    let path = admin.join(RECORD_FILE);
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("Cannot delete {}", escape_path(&path)))
        }
        _ => Ok(()),
    }
}

/// Where git checked out the worktree whose admin directory is `admin`, if
/// git's link back to it can be read. (git may write it relative to `admin`.)
pub fn checkout(admin: &Path) -> Option<PathBuf> {
    let gitdir = std::fs::read_to_string(admin.join("gitdir")).ok()?;
    Some(admin.join(gitdir.trim()).parent()?.to_path_buf())
}

/// Whether the checkout of the worktree whose admin directory is `admin`
/// is gone (git calls such a worktree prunable). Anything that can't be
/// read counts as still there.
pub fn checkout_gone(admin: &Path) -> bool {
    let Ok(gitdir) = std::fs::read_to_string(admin.join("gitdir")) else {
        return false;
    };
    matches!(
        admin.join(gitdir.trim()).symlink_metadata(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
    )
}

/// Meta's record of creating the set at `set`, from the first of its
/// worktrees `checkouts` with a trusted record naming it (see
/// [`super::safety::member_record`]). Worktrees whose record can't be used
/// are passed over: the removal check blocks on them.
pub fn find<P: AsRef<Path>>(
    set: &Path,
    checkouts: impl IntoIterator<Item = P>,
) -> Option<SetRecord> {
    let set = set.canonicalize().ok()?;
    checkouts.into_iter().find_map(|checkout| {
        super::safety::member_record(checkout.as_ref())
            .ok()
            .flatten()
            .map(|(_, record)| record)
            .filter(|record| record.set == set)
    })
}

/// Every record in the worktree admin directories of the repository whose
/// git directory is `git_dir`, by admin directory. A record, or a
/// directory, that can't be read is there as its error: a set it would
/// have shown must not be taken as absent.
pub fn in_repo(git_dir: &Path) -> Vec<(PathBuf, Result<SetRecord>)> {
    let worktrees = git_dir.join("worktrees");
    let entries = match std::fs::read_dir(&worktrees) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            let e =
                anyhow::Error::from(e).context(format!("Cannot read {}", escape_path(&worktrees)));
            return vec![(worktrees, Err(e))];
        }
    };
    entries
        .filter_map(|entry| match entry {
            Ok(entry) => {
                let admin = entry.path();
                read(&admin).transpose().map(|record| (admin, record))
            }
            Err(e) => Some((
                worktrees.clone(),
                Err(anyhow::Error::from(e)
                    .context(format!("Cannot read {}", escape_path(&worktrees)))),
            )),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(created_at: &str, ttl_seconds: Option<u64>) -> SetRecord {
        SetRecord {
            set: PathBuf::from("/p/.worktrees/test-wt"),
            project: PathBuf::from("/p"),
            created_at: created_at.to_string(),
            ephemeral: false,
            ttl_seconds,
            custom: HashMap::new(),
        }
    }

    #[test]
    fn name_is_the_set_directory_name() {
        assert_eq!(record("", None).name(), "test-wt");
    }

    #[test]
    fn ttl_remaining() {
        // 2025-01-01T00:00:00Z is epoch 1735689600
        let created = 1_735_689_600i64;
        let r = record("2025-01-01T00:00:00Z", Some(3600));
        assert_eq!(r.ttl_remaining(created + 1800), Some(1800));
        assert_eq!(r.ttl_remaining(created + 3600), Some(0));
        assert_eq!(r.ttl_remaining(created + 7200), Some(-3600));
        assert_eq!(record("2025-01-01T00:00:00Z", None).ttl_remaining(0), None);
        let huge = record("2025-01-01T00:00:00Z", Some(u64::MAX));
        assert_eq!(
            huge.ttl_remaining(1_800_000_000),
            Some(i64::MAX - 1_800_000_000)
        );
        let malformed = record("not-a-date", Some(3600));
        assert_eq!(malformed.ttl_remaining(1_700_000_000), Some(i64::MAX));
    }

    #[test]
    fn write_read_delete_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let admin = temp.path().join("worktrees/wt");
        std::fs::create_dir_all(&admin).unwrap();
        assert_eq!(read(&admin).unwrap(), None);

        let mut r = record("2025-01-01T00:00:00Z", Some(60));
        r.custom.insert("k".into(), "v".into());
        write(&admin, &r).unwrap();
        assert_eq!(read(&admin).unwrap(), Some(r.clone()));
        let found = in_repo(temp.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, admin);
        assert_eq!(found[0].1.as_ref().unwrap(), &r);

        delete(&admin).unwrap();
        delete(&admin).unwrap();
        assert_eq!(read(&admin).unwrap(), None);
        assert!(in_repo(temp.path()).is_empty());
    }

    #[test]
    fn write_needs_the_admin_dir_to_exist() {
        let temp = tempfile::tempdir().unwrap();
        let admin = temp.path().join("gone");
        assert!(write(&admin, &record("", None)).is_err());
        assert!(!admin.exists());
    }

    #[test]
    fn a_malformed_record_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join(RECORD_FILE), "not json").unwrap();
        assert!(read(temp.path()).is_err());
    }
}
