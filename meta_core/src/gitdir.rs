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

/// Whether `dir` is a git directory, as git decides before using one:
/// `objects` and `refs` directories, and a `HEAD` naming a ref under
/// `refs/` (as a symlink, or `ref: refs/...`) or holding a commit id.
/// Anything less, and git looks past it to the repository around.
pub fn is_bare(dir: &Path) -> bool {
    let head = dir.join("HEAD");
    let valid_head = match std::fs::read_link(&head) {
        Ok(target) => target.starts_with("refs"),
        Err(_) => {
            let head = std::fs::read_to_string(&head).unwrap_or_default();
            let head = head.trim_end();
            head.strip_prefix("ref:")
                .is_some_and(|target| target.trim_start().starts_with("refs/"))
                || (head.len() >= 40 && head.chars().all(|c| c.is_ascii_hexdigit()))
        }
    };
    valid_head && dir.join("objects").is_dir() && dir.join("refs").is_dir()
}

/// Whether `dir` has a git directory's shape, whatever its `HEAD` holds:
/// a `HEAD` entry, and `objects` and `refs` directories. What guards
/// deletion asks this, not [`is_bare`]: a repository whose `HEAD` was
/// damaged still holds objects and refs worth keeping.
pub fn looks_like_git_dir(dir: &Path) -> bool {
    dir.join("HEAD").symlink_metadata().is_ok()
        && dir.join("objects").is_dir()
        && dir.join("refs").is_dir()
}

/// The git directory the repository at `dir` keeps its objects, refs and
/// worktrees in: `.git` itself; for a linked worktree, what its admin
/// directory's `commondir` names; for a `--separate-git-dir` checkout, what
/// its `.git` file names; a bare repository itself. `None` when there is no
/// repository at `dir`: a `.git` directory that isn't a git directory (an
/// empty one, say) is none, as git looks past it to the repository around.
pub fn common_dir(dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    let dot_git = dir.join(".git");
    let target = match gitfile_target(dir)? {
        Some(target) => target,
        None if dot_git.is_dir() => return Ok(is_bare(&dot_git).then_some(dot_git)),
        None => return Ok(is_bare(dir).then(|| dir.to_path_buf())),
    };
    let commondir = target.join("commondir");
    match std::fs::read_to_string(&commondir) {
        Ok(common) => Ok(Some(target.join(common.trim_end_matches(['\n', '\r'])))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Some(target)),
        Err(e) => Err(e).with_context(|| format!("Cannot read {}", escape_path(&commondir))),
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
    anyhow::ensure!(
        is_bare(&common),
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
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(empty.join(".git")).unwrap();
        assert_eq!(common_dir(&empty).unwrap(), None);
        // a git directory whose HEAD git can't read is none either
        let truncated = repo.join(".git/HEAD");
        let head = std::fs::read_to_string(&truncated).unwrap();
        std::fs::write(&truncated, "").unwrap();
        assert_eq!(common_dir(&repo).unwrap(), None);
        // a detached HEAD holds a commit id
        std::fs::write(&truncated, format!("{}\n", "0".repeat(40))).unwrap();
        assert!(common_dir(&repo).unwrap().is_some());
        std::fs::write(&truncated, head).unwrap();
        // HEAD as a symlink to a branch that is only in packed-refs
        #[cfg(unix)]
        {
            std::fs::remove_file(&truncated).unwrap();
            std::os::unix::fs::symlink("refs/heads/packed", &truncated).unwrap();
            assert!(common_dir(&repo).unwrap().is_some());
        }

        let linked = linked(&wt).unwrap();
        assert_eq!(linked.common.canonicalize().unwrap(), git_dir);
        let admin = linked.admin.canonicalize().unwrap();
        assert!(admin.starts_with(git_dir.join("worktrees")), "{admin:?}");
        assert!(super::linked(&repo).is_err());
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
