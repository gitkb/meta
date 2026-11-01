//! What meta records about a worktree set it created.
//!
//! There is no global list. Each linked worktree meta creates gets a record
//! in git's own admin directory for that worktree
//! (`<repo git dir>/worktrees/<id>/meta-worktree.json`). Git ignores the
//! file and deletes it with the worktree (`git worktree remove` or `prune`).
//! Only something that can write the repository's git directory can make
//! one, so a record is meta's proof that it made the set the record names.

use crate::display::escape_path;
use crate::gitdir;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The record's file name inside a worktree's admin directory.
pub const RECORD_FILE: &str = "meta-worktree.json";

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
/// worktrees `checkouts` with a trusted record naming it (see [`member`]). Worktrees whose record can't be used
/// are passed over: the removal check blocks on them.
pub fn find<P: AsRef<Path>>(
    set: &Path,
    checkouts: impl IntoIterator<Item = P>,
) -> Option<SetRecord> {
    let set = set.canonicalize().ok()?;
    checkouts.into_iter().find_map(|checkout| {
        member(checkout.as_ref())
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

/// Meta's record of the linked worktree checked out at `checkout`, with its
/// admin directory: `None` if it isn't a linked worktree or has no record,
/// an error if it has a record that can't be read or trusted.
pub fn member(checkout: &Path) -> Result<Option<(PathBuf, SetRecord)>> {
    let Ok(gitdir::Linked {
        admin,
        common: repo_git_dir,
    }) = gitdir::linked(checkout)
    else {
        return Ok(None);
    };
    Ok(trusted(&admin, &repo_git_dir)?.map(|record| (admin, record)))
}

/// The record in `admin`, the admin directory of a worktree of the
/// repository whose git directory is `repo_git_dir`. Git keeps every
/// worktree's admin directory in its repository's `worktrees` directory; a
/// record anywhere else is no proof of anything, and an error.
pub fn trusted(admin: &Path, repo_git_dir: &Path) -> Result<Option<SetRecord>> {
    let Some(record) = read(admin)? else {
        return Ok(None);
    };
    let registered = || -> Option<bool> {
        Some(
            admin.canonicalize().ok()?.parent()?
                == repo_git_dir.join("worktrees").canonicalize().ok()?,
        )
    };
    anyhow::ensure!(
        registered() == Some(true),
        "its git metadata {} is not where git keeps a worktree's",
        escape_path(admin)
    );
    Ok(Some(record))
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
