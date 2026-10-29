//! Atomic JSON store read/write.
//!
//! Provides generic utilities for reading and writing JSON files with
//! atomic write semantics (write to a temporary file, then rename).

use crate::display::escape_path;
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::Path;

/// Read a JSON file, returning `T::default()` if the file doesn't exist.
///
/// Returns an error if the file exists but can't be read or parsed: only a
/// missing file means an empty store, so a failed read never lets an update
/// replace every entry.
pub fn read<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    let content = match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        result => {
            result.with_context(|| format!("Failed to read store file: {}", escape_path(path)))?
        }
    };

    if content.trim().is_empty() {
        return Ok(T::default());
    }

    serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse store file: {}", escape_path(path)))
}

/// Write data to a JSON file atomically.
///
/// Writes to a temporary file beside it, then renames it to the target path.
/// This ensures readers never see a partially-written file. A store that is
/// a symlink is followed: its target is replaced (or created), not the link.
pub fn write_atomic<T: Serialize>(path: &Path, data: &T) -> Result<()> {
    let path = &follow_links(path)?;
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            crate::data_dir::create_private_dir(parent)?;
        }
    }

    // One temp file per writer (a fixed name lets two writers interleave),
    // created fresh, readable and writable by you alone
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let tmp_path = path.with_extension(format!(
        "tmp.{}.{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));

    let json =
        serde_json::to_string_pretty(data).with_context(|| "Failed to serialize store data")?;

    let _ = std::fs::remove_file(&tmp_path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(&tmp_path)
        .with_context(|| format!("Failed to create temp file: {}", escape_path(&tmp_path)))?;
    std::io::Write::write_all(&mut file, json.as_bytes())
        .with_context(|| format!("Failed to write temp file: {}", escape_path(&tmp_path)))?;
    drop(file);

    let renamed = std::fs::rename(&tmp_path, path)
        .with_context(|| format!("Failed to rename temp file to: {}", escape_path(path)));
    if renamed.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    renamed
}

/// Where `path` leads through its symlinks, even to a target that does not
/// exist yet. More than 40 links (a loop) is an error, as opening it would be.
fn follow_links(path: &Path) -> Result<std::path::PathBuf> {
    let mut path = path.to_path_buf();
    for _ in 0..=40 {
        match std::fs::read_link(&path) {
            Ok(target) => path = path.parent().unwrap_or(Path::new("")).join(target),
            Err(_) => return Ok(path),
        }
    }
    anyhow::bail!("Too many levels of symbolic links: {}", escape_path(&path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::collections::HashMap;

    #[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
    struct TestStore {
        items: HashMap<String, String>,
    }

    #[test]
    fn test_read_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonexistent.json");

        let store: TestStore = read(&path).unwrap();
        assert_eq!(store, TestStore::default());
    }

    #[test]
    fn test_write_atomic_and_read() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.json");

        let mut store = TestStore::default();
        store.items.insert("key".to_string(), "value".to_string());

        write_atomic(&path, &store).unwrap();

        let loaded: TestStore = read(&path).unwrap();
        assert_eq!(loaded.items.get("key").unwrap(), "value");

        // Ensure tmp file was cleaned up
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn test_write_atomic_creates_parent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("dir").join("test.json");

        let store = TestStore::default();
        write_atomic(&path, &store).unwrap();
        assert!(path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_store_files_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("new").join("store.json");
        write_atomic(&path, &TestStore::default()).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn test_symlinked_store_is_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target.json");
        let link = tmp.path().join("store.json");
        write_atomic(&target, &TestStore::default()).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let mut store = TestStore::default();
        store.items.insert("key".to_string(), "value".to_string());
        write_atomic(&link, &store).unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(read::<TestStore>(&target).unwrap(), store);

        // A link to a store not written yet creates it, as `echo > link` would
        let dangling = tmp.path().join("dangling.json");
        std::os::unix::fs::symlink("new.json", &dangling).unwrap();
        write_atomic(&dangling, &store).unwrap();
        assert!(std::fs::symlink_metadata(&dangling)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            read::<TestStore>(&tmp.path().join("new.json")).unwrap(),
            store
        );

        // A link loop fails and is left as it is
        let looped = tmp.path().join("loop.json");
        std::os::unix::fs::symlink("loop.json", &looped).unwrap();
        assert!(write_atomic(&looped, &store).is_err());
        assert_eq!(std::fs::read_link(&looped).unwrap(), Path::new("loop.json"));
    }

    #[test]
    fn test_read_empty_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("empty.json");
        std::fs::write(&path, "").unwrap();

        let store: TestStore = read(&path).unwrap();
        assert_eq!(store, TestStore::default());
    }
}
