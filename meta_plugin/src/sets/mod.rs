//! Worktree sets, for any provider: a set is a folder in the workspace's
//! worktrees folder (`Request::workspace.worktree.dir`) holding checkouts of
//! some of the workspace's projects, each at the project's path. meta finds
//! the set a command runs in and its members (the projects whose folder is
//! in the set) and sends them as `Request::set`; a provider makes and
//! deletes sets with its own tool.
//!
//! What a set has beside it, in the worktrees folder (so nothing is written
//! into a checkout, where it would be an untracked file):
//! - `.meta-sets/<set>.json`, its [`Owner`]: which provider made it, and from
//!   which workspace. [`create`] writes it. meta refuses a command on a named
//!   set from a provider that isn't its owner. A set without the file is
//!   meta-git's (every set made before the file existed is a git set). The
//!   folder's name starts with `.`, as no set's can: a provider listing the
//!   sets in the worktrees folder passes over names that start with `.`.
//! - `.<set>.meta-lease` and its lock: the [`lease`] its creator or its
//!   ephemeral run holds.
//!
//! [`fire`] runs the workspace's `worktree.hooks` for a provider's set
//! events, the same for every provider.

pub mod lease;

use crate::display::{escape, escape_path};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use crate::LEASE_VAR;

/// The provider that owns a set without an owner file.
pub const DEFAULT_PROVIDER: &str = "git";

/// The folder beside the sets that holds their owner files.
pub const OWNERS_DIR: &str = ".meta-sets";

/// Who made a set, as `.meta-sets/<set>.json` beside it says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    /// The provider (plugin) that made it: only its commands act on it by name
    pub provider: String,
    /// The folder of the workspace config the set was made from
    #[serde(with = "crate::wire_path")]
    pub workspace: PathBuf,
    /// The set's own folder, resolved, when it was made: a file beside
    /// another folder (a copy of the workspace) is not that set's
    #[serde(with = "crate::wire_path")]
    pub set: PathBuf,
}

/// A set name is one folder name of letters, digits, `-` and `_`.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("Worktree name cannot be empty");
    }
    let shown = escape(name);
    if name.starts_with('.') {
        anyhow::bail!("Invalid worktree name '{shown}': cannot start with '.'");
    }
    if name.contains('/') || name.contains('\\') {
        anyhow::bail!("Invalid worktree name '{shown}': cannot contain path separators");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!("Invalid worktree name '{shown}': only ASCII alphanumeric characters, hyphens, and underscores allowed");
    }
    Ok(())
}

/// The file beside `set` named `.<set><suffix>`.
pub(crate) fn beside(set: &Path, suffix: &str) -> Result<PathBuf> {
    match (set.parent(), set.file_name()) {
        (Some(dir), Some(name)) => {
            let mut file = std::ffi::OsString::from(".");
            file.push(name);
            file.push(suffix);
            Ok(dir.join(file))
        }
        _ => anyhow::bail!("{} has no folder to keep its files in", escape_path(set)),
    }
}

/// The owner file of the set at `set`: `.meta-sets/<set>.json` beside it.
fn owner_path(set: &Path) -> Result<PathBuf> {
    match (set.parent(), set.file_name()) {
        (Some(dir), Some(name)) => {
            let mut file = name.to_owned();
            file.push(".json");
            Ok(dir.join(OWNERS_DIR).join(file))
        }
        _ => anyhow::bail!("{} has no folder to keep its files in", escape_path(set)),
    }
}

/// Record that `provider` made the set at `set` from the workspace whose
/// config is in `workspace`. Written whole, then put in place.
pub fn create(set: &Path, provider: &str, workspace: &Path) -> Result<()> {
    let path = owner_path(set)?;
    let owner = Owner {
        provider: provider.to_string(),
        workspace: workspace.to_path_buf(),
        set: set
            .canonicalize()
            .with_context(|| format!("Cannot resolve {}", escape_path(set)))?,
    };
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp.{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let json = serde_json::to_vec_pretty(&owner)?;
    let write = || {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&tmp, &json).and_then(|()| std::fs::rename(&tmp, &path))
    };
    // the folder goes with the last owner file another command forgets:
    // made again once if it went in between
    let written = match write() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => write(),
        written => written,
    };
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.with_context(|| format!("Cannot write {}", escape_path(&path)))
}

/// Who made the set at `set`; `None` when nothing says (a set made before
/// owner files, or one that isn't a set). One that can't be read is an
/// error: it says the set is someone's.
pub fn owner(set: &Path) -> Result<Option<Owner>> {
    let path = owner_path(set)?;
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .with_context(|| format!("Cannot read {}", escape_path(&path))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Cannot read {}", escape_path(&path))),
    }
}

/// The provider whose set `set` is: its owner file's, else meta-git's.
pub fn provider(set: &Path) -> Result<String> {
    Ok(owner(set)?.map_or_else(|| DEFAULT_PROVIDER.to_string(), |owner| owner.provider))
}

/// Delete the owner file of the set at `set`, once the set is gone. A set
/// whose folder is still there keeps it.
pub fn forget(set: &Path) -> Result<()> {
    if set.symlink_metadata().is_ok() {
        anyhow::bail!("{} is still there; its owner file stays", escape_path(set));
    }
    let path = owner_path(set)?;
    match std::fs::remove_file(&path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("Cannot delete {}", escape_path(&path)))
        }
        // the folder goes with the last owner file (one still in use stays)
        _ => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::remove_dir(dir);
            }
            Ok(())
        }
    }
}

/// Delete the set `set` that `provider` made, while `claim` holds its lease:
/// only its members' folders and the folders leading to them may be in it.
/// Anything else (a file a user put there, a folder of no member) is refused
/// before anything is deleted, naming it. For a provider whose tool leaves
/// plain folders (a jj workspace forgotten); meta-git deletes its sets with
/// its own checks.
pub fn remove_checked(set: &crate::Set, claim: &lease::Claim, provider: &str) -> Result<()> {
    if !claim.is_for(&set.dir) {
        anyhow::bail!("The lease held is not the one of {}", escape_path(&set.dir));
    }
    let owner = self::provider(&set.dir)?;
    if owner != provider {
        anyhow::bail!(
            "Worktree set {} is {}'s, not {}'s",
            escape(&set.name),
            escape(&owner),
            escape(provider)
        );
    }
    let members: Vec<&Path> = set.members.iter().map(|m| m.abs.as_path()).collect();
    let mut stray = Vec::new();
    strays(&set.dir, &members, &mut stray)?;
    if !stray.is_empty() {
        let shown: Vec<String> = stray.iter().take(5).map(|p| escape_path(p)).collect();
        anyhow::bail!(
            "Worktree set {} holds what is no member's ({}{}); left in place",
            escape(&set.name),
            shown.join(", "),
            if stray.len() > shown.len() {
                ", …"
            } else {
                ""
            }
        );
    }
    std::fs::remove_dir_all(&set.dir)
        .with_context(|| format!("Cannot delete {}", escape_path(&set.dir)))?;
    forget(&set.dir)
}

/// What in `dir` is neither a member's folder nor on the way to one.
fn strays(dir: &Path, members: &[&Path], found: &mut Vec<PathBuf>) -> Result<()> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("Cannot read {}", escape_path(dir)))?;
    for entry in entries {
        let path = entry
            .with_context(|| format!("Cannot read {}", escape_path(dir)))?
            .path();
        if members.contains(&path.as_path()) {
            continue;
        }
        let real_dir = path.symlink_metadata().is_ok_and(|m| m.is_dir());
        match real_dir && members.iter().any(|member| member.starts_with(&path)) {
            true => strays(&path, members, found)?,
            false => found.push(path),
        }
    }
    Ok(())
}

/// Run the hook the workspace's `worktree.hooks` configures for `event`
/// (`post-create`, `post-destroy`, `post-prune`), if any, with `payload` as
/// JSON on its stdin, from the folder of the config the hooks come from.
/// The hook's stderr is meta's (as git's hooks'); its stdout is dropped. A
/// hook that fails, or can't start or be sent its payload, is an error
/// naming its event (not its command, which may hold a secret); what it
/// followed is done either way. A hook that doesn't read its payload is no
/// failure.
#[cfg(feature = "process")]
pub fn fire(worktree: &crate::Worktree, event: &str, payload: &serde_json::Value) -> Result<()> {
    use std::process::Stdio;
    let Some(command) = worktree.hooks.get(event) else {
        return Ok(());
    };
    let hook = format!("The {} hook", escape(event));
    #[expect(
        clippy::disallowed_methods,
        reason = "the hook's shell; process::clean_env clears the repo's git env next"
    )]
    let mut sh = std::process::Command::new("sh");
    crate::process::clean_env(&mut sh);
    if let Some(home) = &worktree.home {
        sh.current_dir(home);
    }
    let status = crate::process::spawn(
        sh.args(["-c", command])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit()),
    )
    .and_then(|child| child.exchange(payload.to_string().as_bytes()))
    .with_context(|| format!("{hook} could not run, or be sent its payload"))?
    .status;
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        anyhow::bail!("{hook} was killed by signal {signal}");
    }
    match status.code() {
        Some(0) => Ok(()),
        Some(code) => anyhow::bail!("{hook} exited with code {code}"),
        None => anyhow::bail!("{hook} ended without an exit code ({status})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_set_names_its_owner_beside_it_and_without_one_is_gits() {
        let temp = tempfile::tempdir().unwrap();
        let set = temp.path().join("wt/a");
        std::fs::create_dir_all(&set).unwrap();
        assert_eq!(owner(&set).unwrap(), None);
        assert_eq!(provider(&set).unwrap(), "git");
        create(&set, "jj", Path::new("/w")).unwrap();
        assert_eq!(provider(&set).unwrap(), "jj");
        assert_eq!(owner(&set).unwrap().unwrap().workspace, Path::new("/w"));
        // nothing is written into the set itself, nor any file beside it:
        // one folder whose name no set can have
        assert_eq!(std::fs::read_dir(&set).unwrap().count(), 0);
        let beside: Vec<_> = std::fs::read_dir(set.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap())
            .collect();
        assert!(beside.iter().all(|e| e.path().is_dir()));
        assert!(validate_name(OWNERS_DIR).is_err());
        // a file that can't be read is someone's: an error, not "git"
        std::fs::write(owner_path(&set).unwrap(), "{").unwrap();
        assert!(provider(&set).is_err());
        assert!(forget(&set).is_err(), "kept while the set is there");
        std::fs::remove_dir(&set).unwrap();
        forget(&set).unwrap();
        assert_eq!(owner(&set).unwrap(), None);
        assert!(!set.parent().unwrap().join(OWNERS_DIR).exists());
    }

    #[test]
    fn a_set_is_removed_only_by_its_owner_and_only_with_nothing_but_its_members() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("wt/a");
        let member = |path: &str| crate::Project {
            name: path.to_string(),
            path: path.to_string(),
            abs: dir.join(path),
            present: true,
            ..crate::Project::default()
        };
        let set = crate::Set {
            name: "a".to_string(),
            dir: dir.clone(),
            members: vec![member("app"), member("libs/core")],
            provider: "jj".to_string(),
            leased: false,
        };
        for m in &set.members {
            std::fs::create_dir_all(m.abs.join("src")).unwrap();
            std::fs::write(m.abs.join("src/x"), "x").unwrap();
        }
        create(&dir, "jj", Path::new("/w")).unwrap();
        let (_, claim) = lease::hold(&dir).unwrap();
        let refused = remove_checked(&set, &claim, "git").unwrap_err().to_string();
        assert!(refused.contains("is jj's, not git's"), "{refused}");
        std::fs::write(dir.join("libs/notes.txt"), "mine").unwrap();
        let refused = remove_checked(&set, &claim, "jj").unwrap_err().to_string();
        assert!(refused.contains("notes.txt"), "{refused}");
        assert!(dir.join("app/src/x").exists());
        std::fs::remove_file(dir.join("libs/notes.txt")).unwrap();
        remove_checked(&set, &claim, "jj").unwrap();
        assert!(!dir.exists() && owner(&dir).unwrap().is_none());
        // the lease held must be this set's
        std::fs::create_dir_all(&dir).unwrap();
        let (_, other) = lease::hold(&temp.path().join("wt/b")).unwrap();
        assert!(remove_checked(&set, &other, "jj").is_err());
    }

    #[test]
    fn names_are_one_plain_folder_name() {
        for good in ["a", "feat-1", "x_y"] {
            assert!(validate_name(good).is_ok(), "{good}");
        }
        for bad in ["", ".a", "a/b", "a\\b", "a b", "é"] {
            assert!(validate_name(bad).is_err(), "{bad}");
        }
    }
}
