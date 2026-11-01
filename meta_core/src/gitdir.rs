//! A checkout's `.git`, read the way git reads it: a directory, or a file
//! `gitdir: <path>` naming the git directory elsewhere. A linked worktree's
//! names its admin directory (`<repo>/.git/worktrees/<id>`), whose
//! `commondir` names the repository's git directory and whose `gitdir`
//! names the `.git` file back.

use crate::display::escape_path;
use anyhow::Context;
use std::path::{Path, PathBuf};

/// What the `.git` file in `checkout` names, as written (relative to
/// `checkout` unless absolute); `None` when `.git` is a directory or
/// missing. A file that can't be read or doesn't say `gitdir: <path>` (git's
/// own format, one space) is an error.
pub fn read_gitfile(checkout: &Path) -> anyhow::Result<Option<PathBuf>> {
    let dot_git = checkout.join(".git");
    match std::fs::metadata(&dot_git) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("Cannot inspect {}", escape_path(&dot_git)))
        }
    }
    let content = std::fs::read_to_string(&dot_git)
        .with_context(|| format!("Cannot read {}", escape_path(&dot_git)))?;
    let target = content
        .strip_prefix("gitdir: ")
        .map(|target| target.trim_end_matches(['\n', '\r']))
        .filter(|target| !target.is_empty())
        .with_context(|| format!("Invalid .git file format at {}", escape_path(&dot_git)))?;
    Ok(Some(PathBuf::from(target)))
}

/// What the `.git` file in `checkout` names, joined onto `checkout` (see
/// [`read_gitfile`]).
pub fn gitfile_target(checkout: &Path) -> anyhow::Result<Option<PathBuf>> {
    Ok(read_gitfile(checkout)?.map(|target| checkout.join(target)))
}

/// Whether `dir` has a git directory's shape, whatever its `HEAD` holds:
/// a `HEAD` entry, and `objects` and `refs` directories. What guards
/// deletion asks this, not git: a repository whose `HEAD` was
/// damaged still holds objects and refs worth keeping.
pub fn looks_like_git_dir(dir: &Path) -> bool {
    dir.join("HEAD").symlink_metadata().is_ok()
        && dir.join("objects").is_dir()
        && dir.join("refs").is_dir()
}

/// [`looks_like_git_dir`], with a `HEAD` git can read without waiting: a
/// file or a link, never a pipe.
fn has_git_dir_shape(dir: &Path) -> bool {
    looks_like_git_dir(dir)
        && dir
            .join("HEAD")
            .symlink_metadata()
            .is_ok_and(|meta| meta.is_file() || meta.is_symlink())
}

/// The git directory the repository at `dir` keeps its objects, refs and
/// worktrees in, as git names it: `.git` itself; for a linked worktree or a
/// `--separate-git-dir` checkout, where its `.git` file leads; a bare
/// repository itself. Only `dir` is asked about, never the repository
/// around it (see [`crate::git::git`]). `None` when nothing at `dir` is
/// git's (no `.git`, no git directory); an error, with git's reason, when
/// something is but git can't use it (an empty `.git`, a `.git` file naming
/// what is gone, a repository git won't read).
pub fn common_dir(dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    let dot_git = dir.join(".git");
    match dot_git.symlink_metadata() {
        Ok(_) => {
            // where git reads HEAD: `.git` (a link followed), or the folder
            // a `.git` file names; git would wait forever on a pipe there
            let git_dir = match dot_git.metadata() {
                Ok(meta) if meta.is_dir() => Some(dot_git.clone()),
                Ok(meta) if meta.is_file() => gitfile_target(dir)?,
                _ => None,
            };
            if let Some(git_dir) = git_dir {
                let head = git_dir.join("HEAD").symlink_metadata();
                anyhow::ensure!(
                    !head.is_ok_and(|meta| !meta.is_file() && !meta.is_symlink()),
                    "its git directory's HEAD is not a file"
                );
                anyhow::ensure!(
                    git_dir != dot_git || has_git_dir_shape(&dot_git),
                    "its .git is not a git directory"
                );
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !has_git_dir_shape(dir) => {
            return Ok(None)
        }
        // a bare repository, or what git must be asked about
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(e).with_context(|| format!("Cannot inspect {}", escape_path(&dot_git)))
        }
    }
    let output = crate::git::git(dir)
        .args(["rev-parse", "--show-prefix", "--git-common-dir"])
        .output()
        .with_context(|| format!("Cannot run git in {}", escape_path(dir)))?;
    anyhow::ensure!(
        output.status.success(),
        "git can't use its .git: {}",
        crate::display::escape(
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .trim_start_matches("fatal: ")
        )
    );
    // git's answer is the folder's own repository only at its top: where
    // the ceiling couldn't be set, git may have found one around it
    let answer = String::from_utf8_lossy(&output.stdout);
    let mut lines = answer.lines();
    let (prefix, common) = (
        lines.next().unwrap_or_default(),
        lines.next().unwrap_or_default(),
    );
    anyhow::ensure!(
        prefix.is_empty() && !common.is_empty(),
        "git can't use its .git, and takes it for part of the repository around it"
    );
    Ok(Some(match common {
        "." => dir.to_path_buf(),
        common => dir.join(common),
    }))
}

/// What is at a project's folder: the one answer every command acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checkout {
    /// Nothing: not cloned yet.
    Missing,
    /// Something that is no repository of its own, and why.
    NotARepo(String),
    /// A repository, and the git directory it shares with its worktrees
    /// (see [`common_dir`]).
    Repo { git_dir: PathBuf },
}

impl Checkout {
    /// The state of the folder at `dir` (a link followed: one to nothing is
    /// missing). Never git's answer for a repository around `dir`.
    pub fn of(dir: &Path) -> Self {
        match std::fs::metadata(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::Missing,
            Err(e) => return Self::NotARepo(format!("cannot read it: {e}")),
            Ok(meta) if !meta.is_dir() => return Self::NotARepo("not a folder".into()),
            Ok(_) => {}
        }
        match common_dir(dir) {
            Ok(Some(git_dir)) => Self::Repo { git_dir },
            Ok(None) => Self::NotARepo("folder exists, no .git".into()),
            Err(e) => Self::NotARepo(format!("{e:#}")),
        }
    }

    /// What is wrong with it for a command that acts on the repository
    /// there, as a line to print after the project's name; `None` for a
    /// repository.
    pub fn problem(&self) -> Option<String> {
        match self {
            Self::Missing => Some("not cloned (run 'meta git update')".into()),
            Self::NotARepo(why) => Some(format!("not a git repository ({why})")),
            Self::Repo { .. } => None,
        }
    }
}

/// A linked worktree's admin directory and its repository's git directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Linked {
    pub admin: PathBuf,
    pub common: PathBuf,
}

/// Follow the `.git` file of `checkout` to its admin directory and its
/// repository's git directory, requiring the linked-worktree layout git
/// keeps: an admin directory with a `HEAD`, a `commondir` naming a git
/// directory, and a `gitdir` that points back at this checkout. A
/// `--separate-git-dir` checkout, a submodule or a moved worktree fails.
pub fn linked(checkout: &Path) -> anyhow::Result<Linked> {
    let admin = gitfile_target(checkout)?
        .with_context(|| format!("{} has no .git file", escape_path(checkout)))?;
    anyhow::ensure!(
        admin.join("HEAD").is_file(),
        "its git metadata {} is missing; was the repository moved or deleted?",
        escape_path(&admin)
    );
    let common = std::fs::read_to_string(admin.join("commondir")).with_context(|| {
        if admin.components().any(|c| c.as_os_str() == "modules") {
            format!(
                "{} looks like a submodule; meta can't handle sets that contain submodules",
                escape_path(&admin)
            )
        } else {
            format!("{} has no commondir", escape_path(&admin))
        }
    })?;
    let common = admin.join(common.trim_end_matches(['\n', '\r']));
    // the layout only, without starting git: membership is read without it
    anyhow::ensure!(
        has_git_dir_shape(&common),
        "{} is not a git directory",
        escape_path(&common)
    );
    let backlink = std::fs::read_to_string(admin.join("gitdir"))
        .with_context(|| format!("{} has no gitdir", escape_path(&admin)))?;
    let points_back = admin
        .join(backlink.trim_end_matches(['\n', '\r']))
        .canonicalize()
        .ok()
        .is_some_and(|target| checkout.join(".git").canonicalize().ok() == Some(target));
    anyhow::ensure!(
        points_back,
        "{} belongs to another checkout (if it was moved, run 'git worktree repair' in its \
         repository)",
        escape_path(&admin)
    );
    Ok(Linked { admin, common })
}

#[cfg(test)]
#[expect(
    clippy::disallowed_methods,
    reason = "tests start the programs they test with"
)]
mod tests {
    use super::*;
    use crate::git::run;

    /// A repository with one commit at `dir/repo` and a linked worktree of
    /// it at `dir/wt`.
    fn repo_and_worktree(dir: &Path) -> (PathBuf, PathBuf) {
        let repo = dir.join("repo");
        std::fs::create_dir(&repo).unwrap();
        run(&repo, ["init", "-q"]).unwrap();
        run(
            &repo,
            [
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "a",
            ],
        )
        .unwrap();
        let wt = dir.join("wt");
        run(
            &repo,
            [
                "worktree".as_ref(),
                "add".as_ref(),
                "-q".as_ref(),
                wt.as_os_str(),
            ],
        )
        .unwrap();
        (repo, wt)
    }

    #[test]
    fn finds_the_common_dir_of_each_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let (repo, wt) = repo_and_worktree(tmp.path());
        let canonical = |p: Option<PathBuf>| p.unwrap().canonicalize().unwrap();
        let git_dir = repo.join(".git").canonicalize().unwrap();
        assert_eq!(canonical(common_dir(&repo).unwrap()), git_dir);
        assert_eq!(canonical(common_dir(&wt).unwrap()), git_dir);
        assert_eq!(canonical(common_dir(&repo.join(".git")).unwrap()), git_dir);
        assert_eq!(common_dir(tmp.path()).unwrap(), None);
        assert_eq!(common_dir(&tmp.path().join("missing")).unwrap(), None);
        // a folder in a repository without its own is none: git isn't let
        // look above it
        let plain = repo.join("plain");
        std::fs::create_dir(&plain).unwrap();
        assert_eq!(common_dir(&plain).unwrap(), None);
        // a `.git` git can't use is no repository, and says why
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(empty.join(".git")).unwrap();
        let err = common_dir(&empty).unwrap_err().to_string();
        assert!(err.contains("not a git directory"), "{err}");
        let nowhere = tmp.path().join("nowhere");
        std::fs::create_dir(&nowhere).unwrap();
        std::fs::write(nowhere.join(".git"), "gitdir: /nonexistent/x\n").unwrap();
        assert!(common_dir(&nowhere).is_err());
        let head = repo.join(".git/HEAD");
        let saved = std::fs::read_to_string(&head).unwrap();
        std::fs::write(&head, "").unwrap();
        assert!(common_dir(&repo).is_err());
        std::fs::write(&head, saved).unwrap();

        // a pipe named HEAD beside objects and refs is never opened
        #[cfg(unix)]
        {
            let plain = tmp.path().join("fifo");
            for dir in ["objects", "refs"] {
                std::fs::create_dir_all(plain.join(dir)).unwrap();
            }
            let fifo = std::process::Command::new("mkfifo")
                .arg(plain.join("HEAD"))
                .status();
            if fifo.is_ok_and(|status| status.success()) {
                let (send, receive) = std::sync::mpsc::channel();
                let dir = plain.clone();
                std::thread::spawn(move || send.send(common_dir(&dir).ok().flatten()));
                let answer = receive.recv_timeout(std::time::Duration::from_secs(5));
                assert_eq!(answer, Ok(None));
            }
            // nor where a checkout's .git, a link to one or a .git file leads
            let real = tmp.path().join("fifo-git");
            for dir in ["objects", "refs"] {
                std::fs::create_dir_all(real.join(dir)).unwrap();
            }
            let made = std::process::Command::new("mkfifo")
                .arg(real.join("HEAD"))
                .status();
            if made.is_ok_and(|status| status.success()) {
                let checkouts = ["dir", "link", "file"].map(|kind| tmp.path().join(kind));
                for checkout in &checkouts {
                    std::fs::create_dir(checkout).unwrap();
                }
                let [in_dir, linked, filed] = &checkouts;
                std::fs::rename(&real, in_dir.join(".git")).unwrap();
                std::os::unix::fs::symlink(in_dir.join(".git"), linked.join(".git")).unwrap();
                let named = format!("gitdir: {}\n", in_dir.join(".git").display());
                std::fs::write(filed.join(".git"), named).unwrap();
                for checkout in checkouts {
                    let (send, receive) = std::sync::mpsc::channel();
                    std::thread::spawn(move || send.send(common_dir(&checkout).is_err()));
                    let answer = receive.recv_timeout(std::time::Duration::from_secs(5));
                    assert_eq!(answer, Ok(true));
                }
            }
        }

        let linked = linked(&wt).unwrap();
        assert_eq!(linked.common.canonicalize().unwrap(), git_dir);
        let admin = linked.admin.canonicalize().unwrap();
        assert!(admin.starts_with(git_dir.join("worktrees")), "{admin:?}");
        assert!(super::linked(&repo).is_err());
    }

    #[test]
    fn a_bare_repository_is_its_own_common_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = tmp.path().join("bare.git");
        std::fs::create_dir(&bare).unwrap();
        run(&bare, ["init", "-q", "--bare"]).unwrap();
        assert_eq!(common_dir(&bare).unwrap(), Some(bare.clone()));
    }

    #[test]
    fn reads_gitdir_as_git_does() {
        let tmp = tempfile::tempdir().unwrap();
        let file = |content: &str| std::fs::write(tmp.path().join(".git"), content).unwrap();
        file("gitdir: elsewhere\r\n");
        assert_eq!(
            gitfile_target(tmp.path()).unwrap(),
            Some(tmp.path().join("elsewhere"))
        );
        // git rejects these too
        for bad in [
            "gitdir:elsewhere",
            " gitdir: elsewhere",
            "gitdir: \n",
            "elsewhere",
        ] {
            file(bad);
            assert!(gitfile_target(tmp.path()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_moved_worktree_is_not_linked() {
        let tmp = tempfile::tempdir().unwrap();
        let (_, wt) = repo_and_worktree(tmp.path());
        let moved = tmp.path().join("moved");
        std::fs::rename(&wt, &moved).unwrap();
        let err = linked(&moved).unwrap_err();
        assert!(
            err.to_string().contains("belongs to another checkout"),
            "{err}"
        );
    }
}
