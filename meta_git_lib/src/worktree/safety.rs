//! Checks that must pass before meta deletes a worktree set.
//!
//! Removal deletes the whole set directory, so every repository inside it is
//! inspected, nested ones included, and so is anything outside a repository.

use anyhow::{Context, Result};
use meta_core::display::{escape, escape_path};
use std::path::{Path, PathBuf};

use super::git_ops::{git_command, remove_worktree_repos};
use super::record::{self, SetRecord};

/// A linked worktree checkout inside a set, the git directory of the
/// repository it belongs to (where `git worktree remove` runs), and meta's
/// record of creating it, if it has one.
#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub path: PathBuf,
    pub repo_git_dir: PathBuf,
    pub record: Option<SetRecord>,
}

/// What deleting a worktree set would destroy.
#[derive(Debug, Default, PartialEq)]
pub struct RemovalCheck {
    /// Every linked worktree in the set, nested and hidden ones included.
    pub members: Vec<Member>,
    /// Repositories that aren't linked worktrees, checkouts git can't
    /// inspect, and worktrees meta recorded for another set. No flag
    /// overrides these.
    pub blockers: Vec<String>,
    /// Work that deleting the set would lose. `--force` may discard it.
    pub unsaved: Vec<String>,
    /// Files outside every repository (part of `unsaved`).
    pub loose: Vec<PathBuf>,
    /// Records that can't be used (part of `blockers`). They block removal
    /// as a proof of ownership; a create rolling back the set it just
    /// claimed needs no such proof.
    pub unusable_records: Vec<String>,
    /// Worktrees without a record in a set whose other worktrees have one
    /// (part of `blockers`): meta didn't make them, so it doesn't delete
    /// them. A create rolling back may not have recorded every worktree yet.
    pub missing_records: Vec<String>,
    /// The members whose record can't be used.
    unusable_members: Vec<PathBuf>,
    /// Each checkout's complete git status, stranded commit ids, and the size
    /// and modification time of every file `unsaved` lists, so a re-check
    /// sees changes the abbreviated lines would hide, rewrites included.
    exact: Vec<String>,
}

impl RemovalCheck {
    pub fn is_clear(&self) -> bool {
        self.blockers.is_empty() && self.unsaved.is_empty()
    }

    /// Meta's record of creating the set, if a worktree in it has one. A
    /// record naming another set is a blocker, so without blockers every
    /// record found names this set.
    pub fn record(&self) -> Option<&SetRecord> {
        self.members.iter().find_map(|m| m.record.as_ref())
    }
}

/// Inspect a worktree set before deleting it.
///
/// Blocks on repositories that aren't linked worktrees (a `.git` directory or
/// link, a bare repository, or a `.git` file that doesn't name a linked
/// worktree of its repository) and on checkouts git cannot inspect. Reports
/// as unsaved: uncommitted or untracked changes, commits that no branch, tag
/// or remote ref holds, and files that no repository holds. Ignored files count as disposable, as
/// they do for `git worktree remove`. Blocks on a worktree whose record
/// (see [`record`]) names another set, such as a set nested in this one,
/// whose record can't be read, or that has no record while another worktree
/// of the set has one.
pub fn check_worktree_removal(wt_dir: &Path) -> Result<RemovalCheck> {
    let mut check = RemovalCheck::default();
    if wt_dir.symlink_metadata().is_ok_and(|m| m.is_symlink()) {
        check.blockers.push(format!(
            "{} is a symbolic link; meta doesn't delete through links",
            escape_path(wt_dir)
        ));
        return Ok(check);
    }
    let set = wt_dir
        .canonicalize()
        .with_context(|| format!("Cannot resolve {}", escape_path(wt_dir)))?;
    let root_owned = match git_entry(wt_dir)? {
        GitEntry::Worktree(dirs) => {
            add_member(&mut check, &set, wt_dir, dirs);
            true
        }
        GitEntry::Repository(why) => {
            check
                .blockers
                .push(format!("{}: {why}", escape_path(wt_dir)));
            return Ok(check);
        }
        GitEntry::None => false,
    };
    let mut loose = Vec::new();
    scan(&set, wt_dir, root_owned, &mut loose, &mut check)?;
    // A stable order, so two checks of an unchanged set compare equal.
    check.members.sort_by(|a, b| a.path.cmp(&b.path));
    loose.sort();

    // Another set's records are blockers already
    let mut records = check
        .members
        .iter()
        .filter_map(|m| m.record.as_ref().filter(|r| r.set == set));
    if let Some(first) = records.next() {
        if records.any(|r| r != first) {
            check.blockers.push(format!(
                "{}: its worktrees hold different records of meta creating it",
                escape_path(wt_dir)
            ));
        }
        let missing: Vec<String> = check
            .members
            .iter()
            .filter(|m| m.record.is_none() && !check.unusable_members.contains(&m.path))
            .map(|m| {
                format!(
                    "{}: meta has no record of creating it (made by hand, or its record was \
                     deleted); run 'git worktree remove' on it first",
                    relative_label(wt_dir, &m.path)
                )
            })
            .collect();
        check.blockers.extend(missing.iter().cloned());
        check.missing_records = missing;
    }
    check.blockers.sort();

    let paths: Vec<PathBuf> = check.members.iter().map(|m| m.path.clone()).collect();
    for member in &paths {
        let label = relative_label(wt_dir, member);
        match unsaved_work(member, &paths) {
            Ok((found, exact)) => {
                check
                    .unsaved
                    .extend(found.into_iter().map(|what| format!("{label}: {what}")));
                check.exact.push(exact);
            }
            // What meta can't inspect it doesn't delete, even with --force.
            Err(e) => check
                .blockers
                .push(format!("{label}: cannot verify ({e:#})")),
        }
    }
    if !loose.is_empty() {
        check.unsaved.push(format!(
            "{} not inside any repository: {}",
            plural(loose.len(), "file"),
            sample(&loose, |p| relative_label(wt_dir, p))
        ));
    }
    check
        .exact
        .extend(loose.iter().map(|path| file_fingerprint(path)));
    check.loose = loose;
    Ok(check)
}

/// A file's path, size and modification time: enough to notice a rewrite.
fn file_fingerprint(path: &Path) -> String {
    match path.symlink_metadata() {
        Ok(meta) => format!(
            "{}\t{}\t{:?}",
            path.display(),
            meta.len(),
            meta.modified().ok()
        ),
        Err(e) => format!("{}\t{}", path.display(), e.kind()),
    }
}

/// Delete a set that [`check_worktree_removal`] inspected, and nothing else.
///
/// Checks the set again first and refuses if anything changed since `check`
/// (prune checks many sets before deleting any). Then deletes the files
/// outside every repository only if `force`, unregisters the checked
/// worktrees (stopping at the first failure), and removes the directories
/// left empty.
/// Anything outside the checked worktrees that is still there is reported
/// and left in place. meta doesn't lock out other processes, so a write into
/// the set during the deletion itself can still be lost.
pub fn delete_checked_set(
    wt_dir: &Path,
    check: &RemovalCheck,
    force: bool,
    verbose: bool,
) -> Result<()> {
    if check_worktree_removal(wt_dir)? != *check {
        anyhow::bail!(
            "{} changed after it was checked; left in place, check it again",
            escape_path(wt_dir)
        );
    }
    // Loose files first: git deletes each worktree's record with it, so a
    // set that fails here is still meta's to remove on a retry.
    if force {
        for path in &check.loose {
            remove_entry(path).with_context(|| format!("Cannot delete {}", escape_path(path)))?;
        }
    }
    remove_worktree_repos(&check.members, force, verbose)?;
    remove_empty_dirs(wt_dir)?;
    if wt_dir.symlink_metadata().is_ok() {
        let left: Vec<PathBuf> = std::fs::read_dir(wt_dir)
            .map(|entries| entries.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        anyhow::bail!(
            "{} could not be deleted completely (left in place: {}); new files may have \
             appeared in it, or some couldn't be removed",
            escape_path(wt_dir),
            sample(&left, |p| relative_label(wt_dir, p))
        );
    }
    Ok(())
}

/// Delete a file or link. On Windows a link to a directory needs `remove_dir`.
fn remove_entry(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) if path.symlink_metadata().is_ok_and(|m| m.is_symlink()) => {
            std::fs::remove_dir(path).map_err(|_| e)
        }
        other => other,
    }
}

/// Remove `dir` and its subdirectories that hold nothing. Links aren't followed.
fn remove_empty_dirs(dir: &Path) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("Cannot read {}", escape_path(dir))),
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("Cannot read {}", escape_path(dir)))?;
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            remove_empty_dirs(&entry.path())?;
        }
    }
    // Fails harmlessly when something is left; the caller reports it.
    let _ = std::fs::remove_dir(dir);
    Ok(())
}

/// Add the linked worktree at `path`, with its admin directory and its
/// repository's git directory, to the members of the set resolved to `set`.
fn add_member(
    check: &mut RemovalCheck,
    set: &Path,
    path: &Path,
    (admin, repo_git_dir): (PathBuf, PathBuf),
) {
    let record = match trusted_record(&admin, &repo_git_dir) {
        Ok(record) => record,
        Err(e) => {
            let why = format!(
                "{}: meta's record of it can't be used ({e:#})",
                escape_path(path)
            );
            check.unusable_records.push(why.clone());
            check.blockers.push(why);
            check.unusable_members.push(path.to_path_buf());
            None
        }
    };
    if let Some(other) = record.as_ref().filter(|r| r.set != set) {
        check
            .blockers
            .push(if other.set.symlink_metadata().is_ok() {
                format!(
                    "{}: meta created it for another worktree set, {}; remove that one first",
                    escape_path(path),
                    escape_path(&other.set)
                )
            } else {
                format!(
                "{}: meta created it for a worktree set at {}, which is gone; was the set moved?",
                escape_path(path),
                escape_path(&other.set)
            )
            });
    }
    // git refuses to remove a locked worktree, which would stop a removal
    // halfway
    if admin.join("locked").symlink_metadata().is_ok() {
        check.blockers.push(format!(
            "{}: git has it locked; run 'git worktree unlock' on it first",
            escape_path(path)
        ));
    }
    check.members.push(Member {
        path: path.to_path_buf(),
        repo_git_dir,
        record,
    });
}

/// Meta's record of the linked worktree checked out at `checkout`, with its
/// admin directory: `None` if it isn't a linked worktree or has no record,
/// an error if it has a record that can't be trusted.
pub fn member_record(checkout: &Path) -> Result<Option<(PathBuf, SetRecord)>> {
    let Ok((admin, repo_git_dir)) = linked_worktree_dirs(checkout) else {
        return Ok(None);
    };
    Ok(trusted_record(&admin, &repo_git_dir)?.map(|record| (admin, record)))
}

/// The record in `admin`, the admin directory of a worktree of the
/// repository whose git directory is `repo_git_dir`. Git keeps every
/// worktree's admin directory in its repository's `worktrees` directory; a
/// record anywhere else is no proof of anything, and an error.
fn trusted_record(admin: &Path, repo_git_dir: &Path) -> Result<Option<SetRecord>> {
    let Some(record) = record::read(admin)? else {
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

enum GitEntry {
    /// A linked worktree, with its admin directory and its repository's git
    /// directory.
    Worktree((PathBuf, PathBuf)),
    /// A repository of its own, or a checkout meta can't attribute to one.
    Repository(String),
    None,
}

fn git_entry(dir: &Path) -> Result<GitEntry> {
    let git = dir.join(".git");
    match git.symlink_metadata() {
        Ok(meta) if meta.is_file() => Ok(match linked_worktree_dirs(dir) {
            Ok(dirs) => GitEntry::Worktree(dirs),
            Err(e) => GitEntry::Repository(format!("not a linked worktree ({e:#})")),
        }),
        Ok(_) => Ok(GitEntry::Repository(
            "a git repository, not a linked worktree".to_string(),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let bare = dir.join("HEAD").is_file()
                && dir.join("objects").is_dir()
                && dir.join("refs").is_dir();
            Ok(if bare {
                GitEntry::Repository("a bare git repository".to_string())
            } else {
                GitEntry::None
            })
        }
        Err(e) => Err(e).with_context(|| format!("Cannot inspect {}", escape_path(&git))),
    }
}

/// The admin directory of the linked worktree checked out at `checkout`, in
/// its repository's git directory (see [`linked_worktree_dirs`]).
pub fn linked_worktree_admin_dir(checkout: &Path) -> Result<PathBuf> {
    linked_worktree_dirs(checkout).map(|(admin, _)| admin)
}

/// Follow a checkout's `.git` file to its admin directory and its
/// repository's git directory, requiring the linked-worktree layout: an
/// admin directory with a `commondir` and a `gitdir` that points back at
/// this checkout. A `--separate-git-dir` checkout or a submodule fails this.
fn linked_worktree_dirs(checkout: &Path) -> Result<(PathBuf, PathBuf)> {
    let git_file = checkout.join(".git");
    let content = std::fs::read_to_string(&git_file).context("cannot read .git")?;
    let admin = content
        .trim()
        .strip_prefix("gitdir:")
        .map(|target| checkout.join(target.trim()))
        .context(".git has no gitdir line")?;
    anyhow::ensure!(
        admin.exists(),
        "its git metadata {} is missing; was the repository moved or deleted?",
        escape_path(&admin)
    );
    let common = std::fs::read_to_string(admin.join("commondir")).with_context(|| {
        if admin.components().any(|c| c.as_os_str() == "modules") {
            format!(
                "{} looks like a submodule; meta can't remove sets that contain submodules",
                escape_path(&admin)
            )
        } else {
            format!("{} has no commondir", escape_path(&admin))
        }
    })?;
    let backlink = std::fs::read_to_string(admin.join("gitdir"))
        .with_context(|| format!("{} has no gitdir", escape_path(&admin)))?;
    let points_back = admin
        .join(backlink.trim())
        .canonicalize()
        .ok()
        .is_some_and(|target| git_file.canonicalize().ok() == Some(target));
    anyhow::ensure!(
        points_back,
        "{} belongs to another checkout (if this set was moved, run \
         'git worktree repair' in its repository)",
        escape_path(&admin)
    );
    let common = admin.join(common.trim());
    Ok((admin, common))
}

/// Walk the set without following links. `owned` means a repository above
/// `dir` accounts for its files.
fn scan(
    set: &Path,
    dir: &Path,
    owned: bool,
    loose: &mut Vec<PathBuf>,
    check: &mut RemovalCheck,
) -> Result<()> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("Cannot read {}", escape_path(dir)))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("Cannot read {}", escape_path(dir)))?;
        let path = entry.path();
        if entry.file_name() == ".git" {
            continue;
        }
        let file_type = entry
            .file_type()
            .with_context(|| format!("Cannot inspect {}", escape_path(&path)))?;
        if !file_type.is_dir() {
            if !owned {
                loose.push(path);
            }
            continue;
        }
        match git_entry(&path)? {
            GitEntry::Repository(why) => check
                .blockers
                .push(format!("{}: {why}", escape_path(&path))),
            GitEntry::Worktree(dirs) => {
                add_member(check, set, &path, dirs);
                scan(set, &path, true, loose, check)?;
            }
            GitEntry::None => scan(set, &path, owned, loose, check)?,
        }
    }
    Ok(())
}

/// Describe the work in one checkout that deleting it would lose, and return
/// git's complete output alongside for exact comparison.
fn unsaved_work(repo: &Path, members: &[PathBuf]) -> Result<(Vec<String>, String)> {
    let output = git_command(repo)
        .args(["status", "--porcelain", "-z", "--untracked-files=all"])
        .output()
        .context("cannot run git status")?;
    anyhow::ensure!(
        output.status.success(),
        "git status failed: {}",
        escape(String::from_utf8_lossy(&output.stderr).trim())
    );
    // Exact, for the re-check: raw bytes, not a lossy decoding.
    let text = format!("{:?}", output.stdout);
    // Paths stay raw bytes, so a file with any name is found again on disk.
    let mut modified: Vec<(String, PathBuf)> = Vec::new();
    let mut untracked: Vec<(String, PathBuf)> = Vec::new();
    let mut entries = output.stdout.split(|b| *b == 0).filter(|e| !e.is_empty());
    while let Some(entry) = entries.next() {
        let (Some(code), Some(path)) = (entry.get(..2), entry.get(3..)) else {
            continue;
        };
        if code.contains(&b'R') || code.contains(&b'C') {
            entries.next(); // the rename source
        }
        let shown = String::from_utf8_lossy(path).into_owned();
        let full = repo.join(path_from_bytes(path.strip_suffix(b"/").unwrap_or(path))?);
        if code != b"??" {
            modified.push((shown, full));
        } else if !members.contains(&full) {
            // Nested checkouts show up as untracked; they are checked separately.
            untracked.push((shown, full));
        }
    }
    let hidden = hidden_files(repo)?;

    let mut found = Vec::new();
    for (files, what) in [
        (&modified, "with uncommitted changes"),
        (&untracked, "untracked"),
        (
            &hidden,
            "git is told not to check (assume-unchanged or skip-worktree)",
        ),
    ] {
        if !files.is_empty() {
            found.push(format!(
                "{} {what}: {}",
                plural(files.len(), "file"),
                sample(files, |(shown, _)| escape(shown).into_owned())
            ));
        }
    }
    let operation = operation_in_progress(repo)?;
    if let Some(op) = operation {
        found.push(format!(
            "a {op} is in progress (its state, such as an autostash, would be lost)"
        ));
    }

    let output = git_command(repo)
        .args([
            "rev-list",
            "HEAD",
            "--not",
            "--branches",
            "--tags",
            "--remotes",
        ])
        .output()
        .context("cannot run git rev-list")?;
    anyhow::ensure!(
        output.status.success(),
        "git rev-list failed: {}",
        escape(String::from_utf8_lossy(&output.stderr).trim())
    );
    let commits = String::from_utf8_lossy(&output.stdout).into_owned();
    let stranded = commits.lines().count();
    if stranded > 0 {
        found.push(format!(
            "{} not on any branch, tag or remote",
            plural(stranded, "commit")
        ));
    }
    // Staged content, which the status codes alone don't show.
    let staged = git_command(repo)
        .args(["diff", "--cached", "--raw", "-z", "--no-renames"])
        .output()
        .context("cannot run git diff --cached")?;
    anyhow::ensure!(
        staged.status.success(),
        "git diff --cached failed: {}",
        escape(String::from_utf8_lossy(&staged.stderr).trim())
    );
    let staged = String::from_utf8_lossy(&staged.stdout).into_owned();
    let files: Vec<String> = modified
        .iter()
        .chain(&untracked)
        .chain(&hidden)
        .map(|(_, path)| file_fingerprint(path))
        .collect();
    Ok((
        found,
        format!(
            "{}\0{text}\0{commits}\0{staged}\0{}\0{operation:?}",
            repo.display(),
            files.join("\n")
        ),
    ))
}

/// Tracked files git is told not to look at (assume-unchanged or
/// skip-worktree) that are on disk: `git status` can't see changes to them,
/// so meta counts them as unsaved instead of guessing. Files absent from
/// disk (a sparse checkout) hold nothing.
fn hidden_files(repo: &Path) -> Result<Vec<(String, PathBuf)>> {
    let output = git_command(repo)
        .args(["ls-files", "-v", "-z"])
        .output()
        .context("cannot run git ls-files")?;
    anyhow::ensure!(
        output.status.success(),
        "git ls-files failed: {}",
        escape(String::from_utf8_lossy(&output.stderr).trim())
    );
    // Entries are "<tag> <path>"; a lowercase tag means assume-unchanged,
    // "S" skip-worktree.
    let mut hidden = Vec::new();
    for entry in output.stdout.split(|b| *b == 0) {
        let Some((tag, rest)) = entry.split_first() else {
            continue;
        };
        let Some(path) = rest.strip_prefix(b" ") else {
            continue;
        };
        if tag.is_ascii_lowercase() || *tag == b'S' {
            let full = repo.join(path_from_bytes(path)?);
            if full.symlink_metadata().is_ok() {
                hidden.push((String::from_utf8_lossy(path).into_owned(), full));
            }
        }
    }
    Ok(hidden)
}

/// A path git printed as raw bytes. Outside Unix, git writes UTF-8.
fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes)
            .map(PathBuf::from)
            .context("git printed a path that isn't UTF-8")
    }
}

/// The operation in progress in a checkout, if any. Its state (a rebase's
/// autostash, say) lives in the worktree's git directory, which removal
/// deletes.
fn operation_in_progress(repo: &Path) -> Result<Option<&'static str>> {
    let output = git_command(repo)
        .args(["rev-parse", "--git-dir"])
        .output()
        .context("cannot run git rev-parse")?;
    anyhow::ensure!(
        output.status.success(),
        "git rev-parse --git-dir failed: {}",
        escape(String::from_utf8_lossy(&output.stderr).trim())
    );
    let git_dir = repo.join(String::from_utf8_lossy(&output.stdout).trim_end_matches(['\n', '\r']));
    let markers = [
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase or am"),
        ("MERGE_HEAD", "merge"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
        ("sequencer", "cherry-pick or revert"),
        ("BISECT_START", "bisect"),
    ];
    Ok(markers
        .into_iter()
        .find(|(marker, _)| git_dir.join(marker).symlink_metadata().is_ok())
        .map(|(_, name)| name))
}

pub(super) fn relative_label(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => escape_path(rel).to_string(),
        Err(_) => escape_path(path).to_string(),
    }
}

fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// The first few items, so a huge list stays readable.
fn sample<T>(items: &[T], show: impl Fn(&T) -> String) -> String {
    const SHOWN: usize = 3;
    let mut text = items
        .iter()
        .take(SHOWN)
        .map(show)
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > SHOWN {
        text.push_str(&format!(", and {} more", items.len() - SHOWN));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A source repo with one commit and a linked worktree of it at `wt`.
    fn repo_with_worktree(root: &Path, name: &str, wt: &Path) {
        let source = root.join(name);
        std::fs::create_dir_all(&source).unwrap();
        git(&source, &["init", "-q"]);
        git(&source, &["commit", "-q", "--allow-empty", "-m", "init"]);
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        git(
            &source,
            &["worktree", "add", "-q", "-b", name, wt.to_str().unwrap()],
        );
    }

    #[test]
    fn clean_nested_set_is_clear() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        repo_with_worktree(tmp.path(), "root", &set);
        repo_with_worktree(tmp.path(), "leaf", &set.join("sub/leaf"));
        repo_with_worktree(tmp.path(), "hidden", &set.join(".scratch/leaf"));

        let check = check_worktree_removal(&set).unwrap();
        assert!(check.is_clear(), "{check:?}");
        let mut members: Vec<_> = check.members.iter().map(|m| m.path.clone()).collect();
        members.sort();
        assert_eq!(
            members,
            [
                set.clone(),
                set.join(".scratch").join("leaf"),
                set.join("sub").join("leaf")
            ]
        );
    }

    fn set_record(set: &Path) -> SetRecord {
        SetRecord {
            set: set.canonicalize().unwrap(),
            project: set.to_path_buf(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            ephemeral: false,
            ttl_seconds: None,
            custom: Default::default(),
        }
    }

    fn mark(checkout: &Path, record: &SetRecord) {
        record::write(&linked_worktree_admin_dir(checkout).unwrap(), record).unwrap();
    }

    #[test]
    fn records_name_their_set_and_block_any_other() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        repo_with_worktree(tmp.path(), "root", &set);
        repo_with_worktree(tmp.path(), "leaf", &set.join("leaf"));
        // A worktree git made by hand carries no record
        let check = check_worktree_removal(&set).unwrap();
        assert!(check.is_clear() && check.record().is_none(), "{check:?}");

        let record = set_record(&set);
        mark(&set, &record);
        // In a set meta made, a worktree without a record is not meta's
        let check = check_worktree_removal(&set).unwrap();
        assert_eq!(check.missing_records.len(), 1, "{check:?}");
        assert!(check.missing_records[0].starts_with("leaf: meta has no record"));
        assert_eq!(check.blockers, check.missing_records);
        mark(&set.join("leaf"), &record);
        let check = check_worktree_removal(&set).unwrap();
        assert!(check.is_clear(), "{check:?}");
        assert_eq!(check.record(), Some(&record));

        // A set nested in this one is another set's
        let inner = set.join("inner");
        repo_with_worktree(tmp.path(), "inner", &inner);
        mark(&inner, &set_record(&inner));
        let check = check_worktree_removal(&set).unwrap();
        assert_eq!(check.blockers.len(), 1, "{check:?}");
        assert!(
            check.blockers[0].contains("another worktree set"),
            "{check:?}"
        );
        // and the inner set is clear on its own
        assert!(check_worktree_removal(&inner).unwrap().is_clear());
    }

    #[test]
    fn records_that_differ_or_sit_outside_git_layout_block() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        repo_with_worktree(tmp.path(), "root", &set);
        repo_with_worktree(tmp.path(), "leaf", &set.join("leaf"));
        let record = set_record(&set);
        mark(&set, &record);
        let mut later = record.clone();
        later.ttl_seconds = Some(0);
        mark(&set.join("leaf"), &later);
        let check = check_worktree_removal(&set).unwrap();
        assert!(
            check
                .blockers
                .iter()
                .any(|b| b.contains("different records")),
            "{check:?}"
        );

        // A checkout pointing at a copied admin directory with a record in
        // it: the backlink holds, but git doesn't keep it there
        let fake = tmp.path().join("fake");
        repo_with_worktree(tmp.path(), "real", &fake.join("real"));
        let admin = linked_worktree_admin_dir(&fake.join("real")).unwrap();
        let copy = fake.join("admin");
        std::fs::create_dir(&copy).unwrap();
        for file in ["HEAD", "commondir", "gitdir"] {
            std::fs::copy(admin.join(file), copy.join(file)).unwrap();
        }
        let common = std::fs::read_to_string(admin.join("commondir")).unwrap();
        std::fs::write(
            copy.join("commondir"),
            admin.join(common.trim()).to_str().unwrap(),
        )
        .unwrap();
        std::fs::write(
            fake.join("real/.git"),
            format!("gitdir: {}", copy.display()),
        )
        .unwrap();
        mark(&fake.join("real"), &set_record(&fake));
        let check = check_worktree_removal(&fake).unwrap();
        assert!(check.record().is_none(), "{check:?}");
        assert!(
            check
                .blockers
                .iter()
                .any(|b| b.contains("not where git keeps")),
            "{check:?}"
        );
        // nor does `add` find it to copy into the worktrees it adds
        assert!(member_record(&fake.join("real")).is_err());
        assert_eq!(record::find(&fake, [fake.join("real")]), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_record_is_trusted_when_the_worktrees_dir_is_a_link() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("root");
        std::fs::create_dir_all(&source).unwrap();
        git(&source, &["init", "-q"]);
        git(&source, &["commit", "-q", "--allow-empty", "-m", "init"]);
        std::fs::create_dir(source.join(".git/real-worktrees")).unwrap();
        std::os::unix::fs::symlink("real-worktrees", source.join(".git/worktrees")).unwrap();
        let set = tmp.path().join("set");
        git(
            &source,
            &["worktree", "add", "-q", "-b", "s", set.to_str().unwrap()],
        );
        let record = set_record(&set);
        mark(&set, &record);
        let check = check_worktree_removal(&set).unwrap();
        assert!(check.is_clear(), "{check:?}");
        assert_eq!(check.record(), Some(&record));
    }

    #[test]
    fn a_locked_worktree_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        repo_with_worktree(tmp.path(), "root", &set);
        git(
            &tmp.path().join("root"),
            &["worktree", "lock", set.to_str().unwrap()],
        );
        let check = check_worktree_removal(&set).unwrap();
        assert!(
            check.blockers.iter().any(|b| b.contains("locked")),
            "{check:?}"
        );
    }

    #[test]
    fn reports_dirty_nested_repo_loose_files_and_primary_checkouts() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        repo_with_worktree(tmp.path(), "leaf", &set.join("sub/leaf"));
        std::fs::write(set.join("sub/leaf/new.txt"), "work").unwrap();
        std::fs::write(set.join("notes.txt"), "work").unwrap();
        std::fs::create_dir_all(set.join("clone")).unwrap();
        git(&set.join("clone"), &["init", "-q"]);
        git(&set, &["init", "-q", "--bare", "mirror.git"]);

        let check = check_worktree_removal(&set).unwrap();
        let blockers = check.blockers.join("\n");
        assert_eq!(check.blockers.len(), 2, "{blockers}");
        assert!(blockers.contains("clone") && blockers.contains("mirror.git"));
        let unsaved = check.unsaved.join("\n").replace('\\', "/");
        assert!(
            unsaved.contains("sub/leaf: 1 file untracked: new.txt"),
            "{unsaved}"
        );
        assert!(
            unsaved.contains("not inside any repository: notes.txt"),
            "{unsaved}"
        );
    }

    #[test]
    fn reports_commits_only_a_detached_head_holds() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        repo_with_worktree(tmp.path(), "root", &set);
        git(&set, &["checkout", "-q", "--detach"]);
        git(
            &set,
            &["commit", "-q", "--allow-empty", "-m", "detached work"],
        );

        let check = check_worktree_removal(&set).unwrap();
        assert_eq!(
            check.unsaved,
            [".: 1 commit not on any branch, tag or remote"]
        );
    }

    #[test]
    fn delete_checked_set_refuses_files_that_appeared_after_the_check() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        repo_with_worktree(tmp.path(), "leaf", &set.join("sub/leaf"));
        std::fs::write(set.join("notes.txt"), "work").unwrap();
        let check = check_worktree_removal(&set).unwrap();
        std::fs::write(set.join("sub/late.txt"), "written after the check").unwrap();

        let err = delete_checked_set(&set, &check, true, false).unwrap_err();
        assert!(
            err.to_string().contains("changed after it was checked"),
            "{err}"
        );
        for kept in ["sub/late.txt", "notes.txt", "sub/leaf/.git"] {
            assert!(set.join(kept).exists(), "{kept}");
        }

        std::fs::remove_file(set.join("sub/late.txt")).unwrap();
        let check = check_worktree_removal(&set).unwrap();
        assert_eq!(check.loose, [set.join("notes.txt")]);
        delete_checked_set(&set, &check, true, false).unwrap();
        assert!(!set.exists(), "--force discards the checked loose file");
    }

    #[test]
    fn recheck_sees_changes_the_summary_hides() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        repo_with_worktree(tmp.path(), "root", &set);
        for name in ["a", "b", "c", "d"] {
            std::fs::write(set.join(name), "x").unwrap();
        }
        let before = check_worktree_removal(&set).unwrap();
        std::fs::rename(set.join("d"), set.join("e")).unwrap();
        let after = check_worktree_removal(&set).unwrap();
        assert_eq!(before.unsaved, after.unsaved, "the summary looks the same");
        assert_ne!(before, after);
    }

    #[test]
    fn a_worktree_of_a_bare_repository_is_clear() {
        let tmp = tempfile::tempdir().unwrap();
        let seed = tmp.path().join("seed");
        std::fs::create_dir(&seed).unwrap();
        git(&seed, &["init", "-q"]);
        git(&seed, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let bare = tmp.path().join("bare.git");
        git(
            &seed,
            &["clone", "-q", "--bare", ".", bare.to_str().unwrap()],
        );
        let set = tmp.path().join("set");
        std::fs::create_dir(&set).unwrap();
        git(
            &bare,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "t",
                set.join("a").to_str().unwrap(),
            ],
        );

        let check = check_worktree_removal(&set).unwrap();
        assert!(check.is_clear(), "{check:?}");
        delete_checked_set(&set, &check, false, false).unwrap();
        assert!(!set.exists());
    }

    #[test]
    fn reports_files_hidden_by_index_flags_and_operations_in_progress() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        repo_with_worktree(tmp.path(), "root", &set);
        std::fs::write(set.join("config"), "original").unwrap();
        git(&set, &["add", "config"]);
        git(&set, &["commit", "-q", "-m", "config"]);
        git(&set, &["update-index", "--assume-unchanged", "config"]);
        let check = check_worktree_removal(&set).unwrap();
        assert_eq!(
            check.unsaved,
            [".: 1 file git is told not to check (assume-unchanged or skip-worktree): config"]
        );

        git(&set, &["update-index", "--no-assume-unchanged", "config"]);
        let admin = tmp.path().join("root/.git/worktrees/set");
        std::fs::create_dir(admin.join("rebase-merge")).unwrap();
        let check = check_worktree_removal(&set).unwrap();
        assert!(
            check.unsaved[0].contains("a rebase is in progress"),
            "{check:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn files_with_non_utf8_names_are_tracked_exactly() {
        use std::os::unix::ffi::OsStrExt;

        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        repo_with_worktree(tmp.path(), "root", &set);
        let name = std::ffi::OsStr::from_bytes(b"caf\xff");
        std::fs::write(set.join(name), "draft").unwrap();
        let before = check_worktree_removal(&set).unwrap();
        std::fs::write(set.join(name), "new work, longer than the draft").unwrap();
        let err = delete_checked_set(&set, &before, true, false).unwrap_err();
        assert!(
            err.to_string().contains("changed after it was checked"),
            "{err}"
        );
        assert!(set.join(name).exists());
    }

    #[test]
    fn a_checkout_git_cannot_inspect_is_a_blocker() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        repo_with_worktree(tmp.path(), "root", &set);
        std::fs::write(set.join("file"), "x").unwrap();
        git(&set, &["add", "file"]);
        let index = tmp.path().join("root/.git/worktrees/set/index");
        std::fs::write(&index, "not an index").unwrap();

        let check = check_worktree_removal(&set).unwrap();
        assert!(check.unsaved.is_empty(), "{check:?}");
        assert!(check.blockers[0].contains("cannot verify"), "{check:?}");
    }

    #[test]
    fn recheck_sees_a_rewritten_file() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        repo_with_worktree(tmp.path(), "leaf", &set.join("leaf"));
        std::fs::write(set.join("leaf/notes.txt"), "old draft").unwrap();
        std::fs::write(set.join("loose.txt"), "old").unwrap();
        let before = check_worktree_removal(&set).unwrap();

        std::fs::write(
            set.join("leaf/notes.txt"),
            "new work, longer than the draft",
        )
        .unwrap();
        let after = check_worktree_removal(&set).unwrap();
        assert_eq!(before.unsaved, after.unsaved, "the summary looks the same");
        assert_ne!(before, after);
        let err = delete_checked_set(&set, &before, true, false).unwrap_err();
        assert!(
            err.to_string().contains("changed after it was checked"),
            "{err}"
        );

        std::fs::write(set.join("loose.txt"), "newer").unwrap();
        assert_ne!(after, check_worktree_removal(&set).unwrap());
    }

    #[test]
    fn recheck_sees_a_change_only_in_the_index() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        repo_with_worktree(tmp.path(), "root", &set);
        std::fs::write(set.join("f"), "staged").unwrap();
        git(&set, &["add", "f"]);
        std::fs::write(set.join("f"), "working copy").unwrap();
        let before = check_worktree_removal(&set).unwrap();

        // Replace the staged blob only; status stays the same, the file too.
        std::fs::write(tmp.path().join("other"), "other staged").unwrap();
        let blob = Command::new("git")
            .args(["hash-object", "-w"])
            .arg(tmp.path().join("other"))
            .current_dir(&set)
            .output()
            .unwrap();
        let blob = String::from_utf8_lossy(&blob.stdout).trim().to_string();
        git(
            &set,
            &["update-index", "--cacheinfo", &format!("100644,{blob},f")],
        );

        let after = check_worktree_removal(&set).unwrap();
        assert_eq!(before.unsaved, after.unsaved);
        assert_ne!(before, after);
    }

    #[test]
    fn delete_checked_set_refuses_a_set_that_changed_inside_a_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        repo_with_worktree(tmp.path(), "root", &set);
        std::fs::write(set.join(".gitignore"), "ignored/\n").unwrap();
        git(&set, &["add", ".gitignore"]);
        git(&set, &["commit", "-q", "-m", "ignore"]);
        let check = check_worktree_removal(&set).unwrap();
        assert!(check.is_clear(), "{check:?}");
        // A repository appears in an ignored directory after the check.
        std::fs::create_dir_all(set.join("ignored/repo")).unwrap();
        git(&set.join("ignored/repo"), &["init", "-q"]);

        let err = delete_checked_set(&set, &check, true, false).unwrap_err();
        assert!(
            err.to_string().contains("changed after it was checked"),
            "{err}"
        );
        assert!(set.join("ignored/repo/.git").exists());
    }

    #[test]
    fn symlinked_set_root_is_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target");
        repo_with_worktree(tmp.path(), "root", &target);
        let link = tmp.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        #[cfg(windows)]
        if std::os::windows::fs::symlink_dir(&target, &link).is_err() {
            return; // creating links needs a privilege Windows CI may not have
        }
        let check = check_worktree_removal(&link).unwrap();
        assert!(check.blockers[0].contains("symbolic link"), "{check:?}");
    }

    #[test]
    fn separate_git_dir_checkout_is_not_a_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        std::fs::create_dir_all(set.join("repo")).unwrap();
        let git_dir = tmp.path().join("repo.git");
        git(
            &set.join("repo"),
            &[
                "init",
                "-q",
                "--separate-git-dir",
                git_dir.to_str().unwrap(),
            ],
        );

        let check = check_worktree_removal(&set).unwrap();
        assert!(check.members.is_empty());
        assert_eq!(check.blockers.len(), 1, "{check:?}");
        assert!(
            check.blockers[0].contains("not a linked worktree"),
            "{check:?}"
        );
    }

    #[test]
    fn set_root_that_is_a_primary_checkout_is_blocked() {
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "-q"]);
        let check = check_worktree_removal(tmp.path()).unwrap();
        assert_eq!(check.blockers.len(), 1);
    }
}
