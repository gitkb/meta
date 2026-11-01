//! Git operations for worktree management.

use anyhow::{Context, Result};
use meta_core::display::{escape, escape_path};
use meta_core::git::{lookup, run, RepoStatus};
use meta_core::gitdir::{common_dir, Checkout};
use std::ffi::OsStr;
use std::path::Path;

/// An error if the checkout at `repo_path` has `core.bare=true` in its
/// config: git would make no worktree of it.
pub fn ensure_checkout_config_is_not_bare(repo_path: &Path) -> Result<()> {
    let checkout = Checkout::of(repo_path);
    if let Some(problem) = checkout.problem() {
        anyhow::bail!("{}: {problem}", escape_path(repo_path));
    }
    // `config --get` exits 1 when core.bare isn't configured.
    if lookup(
        repo_path,
        ["config", "--local", "--bool", "--get", "core.bare"],
    )?
    .as_deref()
        == Some("true")
    {
        if let Checkout::Repo { git_dir } = checkout {
            if repo_path.canonicalize()? == git_dir.canonicalize()? {
                return Ok(()); // The bare repository itself has no checkout.
            }
            anyhow::bail!(
                "Repository '{}' is a checked-out work tree but its shared Git config has \
                 core.bare=true.\nRepair it with:\n  git -C {} config --local core.bare false",
                escape_path(repo_path),
                escape(&meta_core::display::shell_quote(
                    &repo_path.to_string_lossy()
                ))
            );
        }
    }

    Ok(())
}

/// Where `git worktree add` starts a worktree of a repo on a branch.
pub enum WorktreeStart {
    /// A new branch at the commit a commit-ish names (the name is passed
    /// to git, so a remote-tracking ref still sets the upstream)
    Ref { name: String, commit: String },
    /// The existing local branch
    Existing,
    /// A new branch tracking `origin/<branch>`, at this commit
    Remote { commit: String },
    /// A new branch at HEAD's commit
    Head { commit: String },
}

impl WorktreeStart {
    /// The commit a new branch is made at; none for an existing branch.
    pub fn new_branch_at(self) -> Option<String> {
        match self {
            Self::Ref { commit, .. } | Self::Remote { commit } | Self::Head { commit } => {
                Some(commit)
            }
            Self::Existing => None,
        }
    }
}

/// How a worktree of `repo_path` on `branch` would start, from `from_ref`
/// if given: every check meta makes before running `git worktree add`, so
/// a preview makes them too.
pub fn worktree_start(
    repo_path: &Path,
    branch: &str,
    from_ref: Option<&str>,
) -> Result<WorktreeStart> {
    ensure_checkout_config_is_not_bare(repo_path)?;
    if let Some(ref_name) = from_ref {
        let commit =
            git_rev_parse(repo_path, &format!("{ref_name}^{{commit}}"))?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Ref '{}' not found in repo '{}'",
                    ref_name,
                    escape_path(repo_path)
                )
            })?;
        return Ok(WorktreeStart::Ref {
            name: ref_name.to_string(),
            commit,
        });
    }
    if git_rev_parse(repo_path, &format!("refs/heads/{branch}"))?.is_some() {
        return Ok(WorktreeStart::Existing);
    }
    if let Some(commit) = git_rev_parse(repo_path, &format!("refs/remotes/origin/{branch}"))? {
        return Ok(WorktreeStart::Remote { commit });
    }
    let commit = git_rev_parse(repo_path, "HEAD")?.ok_or_else(|| {
        anyhow::anyhow!("{} has no commit to branch from", escape_path(repo_path))
    })?;
    Ok(WorktreeStart::Head { commit })
}

/// Make a worktree of `repo_path` at `worktree_dest` on `branch` (see
/// [`worktree_start`]). The commit the branch was created at (`-b` refuses
/// an existing branch, so success proves this call made it), or None for
/// an existing branch.
pub fn git_worktree_add(
    repo_path: &Path,
    worktree_dest: &Path,
    branch: &str,
    from_ref: Option<&str>,
) -> Result<Option<String>> {
    let start = worktree_start(repo_path, branch, from_ref)?;
    let remote_ref = format!("origin/{branch}");
    let mut args: Vec<&OsStr> = vec!["worktree".as_ref(), "add".as_ref()];
    if matches!(start, WorktreeStart::Remote { .. }) {
        args.push("--track".as_ref());
    }
    if !matches!(start, WorktreeStart::Existing) {
        args.extend_from_slice(&["-b".as_ref(), branch.as_ref()]);
    }
    args.push(worktree_dest.as_os_str());
    match &start {
        WorktreeStart::Ref { name, .. } => args.push(name.as_ref()),
        WorktreeStart::Existing => args.push(branch.as_ref()),
        WorktreeStart::Remote { .. } => args.push(remote_ref.as_ref()),
        WorktreeStart::Head { .. } => {}
    }
    run(repo_path, args)?;
    Ok(start.new_branch_at())
}

pub fn git_worktree_remove(repo_path: &Path, worktree_path: &Path, force: bool) -> Result<()> {
    let mut args: Vec<&OsStr> = vec!["worktree".as_ref(), "remove".as_ref()];
    if force {
        args.push("--force".as_ref());
    }
    args.push(worktree_path.as_os_str());
    run(repo_path, args)?;
    Ok(())
}

/// The status of the checkout at `repo_path`, refusing one whose shared
/// config says it is bare (git would misreport it).
pub fn repo_status(repo_path: &Path) -> Result<RepoStatus> {
    ensure_checkout_config_is_not_bare(repo_path)?;
    RepoStatus::read(repo_path)
}

pub fn git_diff_stat(
    worktree_path: &Path,
    base_ref: &str,
) -> Result<(usize, usize, usize, Vec<String>)> {
    // `merge-base` exits 1 only when there is no common ancestor. Only then
    // use a two-dot diff; a bad ref or a failed read is still an error.
    let range = if lookup(worktree_path, ["merge-base", base_ref, "HEAD"])?.is_some() {
        format!("{base_ref}...HEAD")
    } else {
        format!("{base_ref}..HEAD")
    };
    let numstat_text = run(worktree_path, ["diff", "--numstat", &range, "--"])?;

    let mut files_changed = 0;
    let mut insertions = 0usize;
    let mut deletions = 0usize;
    let mut files = Vec::new();
    let count = |value: &str| -> Result<usize> {
        match value {
            "-" => Ok(0), // Binary files have no line counts.
            _ => value.parse().with_context(|| {
                format!(
                    "Invalid git numstat count '{}' in {}",
                    escape(value),
                    escape_path(worktree_path)
                )
            }),
        }
    };

    for line in numstat_text.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let (Some(added), Some(removed), Some(file)) = (parts.next(), parts.next(), parts.next())
        else {
            anyhow::bail!(
                "Invalid git numstat in {}: {}",
                escape_path(worktree_path),
                escape(line)
            );
        };
        files_changed += 1;
        insertions = insertions
            .checked_add(count(added)?)
            .context("git numstat insertions overflow")?;
        deletions = deletions
            .checked_add(count(removed)?)
            .context("git numstat deletions overflow")?;
        files.push(file.to_string());
    }

    Ok((files_changed, insertions, deletions, files))
}

/// Unregister and delete the checked worktrees of a set, deepest first
/// (nested checkouts before the checkouts that contain them).
/// Stops at the first failure, so the caller never deletes a set that git
/// still has registered (a locked worktree, say).
pub fn remove_worktree_repos(
    members: &[super::safety::Member],
    force: bool,
    verbose: bool,
) -> Result<()> {
    let mut ordered: Vec<_> = members.iter().collect();
    ordered.sort_by_key(|m| std::cmp::Reverse(m.path.components().count()));

    for m in ordered {
        if verbose {
            eprintln!("Removing worktree at {}", escape_path(&m.path));
        }
        git_worktree_remove(&m.repo_git_dir, &m.path, force)
            .map_err(|e| e.context(format!("Cannot remove {}", escape_path(&m.path))))?;
    }
    Ok(())
}

/// The commit `rev` names in `repo_path`, or `None` if git says no such name
/// exists. Any other failure (a broken repository, say) is an error with
/// git's message, never mistaken for a missing name.
pub fn git_rev_parse(repo_path: &Path, rev: &str) -> Result<Option<String>> {
    lookup(repo_path, ["rev-parse", "--verify", "--quiet", rev])
}

/// Fail if any worktree of the repository has `branch` checked out, or is
/// rebasing or bisecting it: the reservations git itself enforces. Checked-out
/// branches come from `git worktree list` (git resolves each HEAD through its
/// ref store, files or reftable); rebase and bisect state comes from each
/// worktree's state files (`rebase-merge/head-name`, `rebase-apply/head-name`,
/// `rebase-merge/update-refs`, `BISECT_START`). Changes nothing; anything it
/// can't read is an error, not "free".
pub fn git_branch_is_free(repo_path: &Path, branch: &str) -> Result<()> {
    let full = format!("refs/heads/{branch}");
    let in_use = |by: &str| anyhow::anyhow!("'{}' is {by}", escape(branch));

    // A symbolic branch would carry a ref transaction to the branch it points
    // at, which this check never looked at.
    // `symbolic-ref -q` exits 1 for a plain or absent ref.
    if lookup(repo_path, ["symbolic-ref", "-q", &full])?.is_some() {
        return Err(in_use("a symbolic ref; meta only moves plain branches"));
    }

    let list = run(repo_path, ["worktree", "list", "--porcelain"])?;
    let wanted = format!("branch {full}");
    if list.lines().any(|line| line == wanted) {
        return Err(in_use("checked out in a worktree"));
    }

    let common = common_dir(repo_path)?.with_context(|| {
        format!(
            "Cannot find the git directory of {}",
            escape_path(repo_path)
        )
    })?;
    let mut admin_dirs = vec![common.clone()];
    match std::fs::read_dir(common.join("worktrees")) {
        Ok(entries) => {
            for entry in entries {
                admin_dirs.push(entry?.path());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }

    let read = |path: std::path::PathBuf| -> Result<String> {
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => {
                Err(anyhow::Error::from(e).context(format!("Cannot read {}", escape_path(&path))))
            }
        }
    };
    for dir in admin_dirs {
        let rebasing = [
            read(dir.join("rebase-merge/head-name"))?,
            read(dir.join("rebase-apply/head-name"))?,
        ];
        let updating = read(dir.join("rebase-merge/update-refs"))?;
        if rebasing.iter().any(|name| name.trim() == full)
            || updating.lines().any(|line| line.trim() == full)
        {
            return Err(in_use(&format!(
                "being rebased by the worktree whose git dir is {}",
                escape_path(&dir)
            )));
        }
        let bisect = read(dir.join("BISECT_START"))?;
        if [branch, full.as_str()].contains(&bisect.trim()) {
            return Err(in_use(&format!(
                "being bisected by the worktree whose git dir is {}",
                escape_path(&dir)
            )));
        }
    }
    Ok(())
}

/// Delete a branch, but only if git would let it go (see
/// [`git_branch_is_free`]) and it still points at `expected`, atomically
/// through git's ref transaction.
pub fn git_delete_branch_at(repo_path: &Path, branch: &str, expected: &str) -> Result<()> {
    git_branch_is_free(repo_path, branch)
        .map_err(|e| anyhow::anyhow!("Branch '{}' is in use; kept ({e})", escape(branch)))?;
    run(
        repo_path,
        [
            "update-ref",
            "--no-deref",
            "-d",
            &format!("refs/heads/{branch}"),
            expected,
        ],
    )
    .with_context(|| {
        format!(
            "Branch '{}' no longer points where it was created; kept",
            escape(branch)
        )
    })?;
    // Only remove settings that exist: `--remove-section` treats an absent
    // section as fatal, so its exit status can't distinguish a failed write.
    let section = format!("branch.{branch}");
    if run(repo_path, ["config", "--local", "--name-only", "--list"])?
        .lines()
        .any(|key| {
            key.rsplit_once('.')
                .is_some_and(|(name, _)| name == section)
        })
    {
        run(
            repo_path,
            ["config", "--local", "--remove-section", &section],
        )?;
    }
    Ok(())
}

/// Fetch a branch from origin if not locally available.
pub fn git_fetch_branch(repo_path: &Path, branch: &str) -> Result<()> {
    run(repo_path, ["fetch", "origin", branch])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use meta_core::git::run;
    use std::process::{Command, Stdio};

    fn git(dir: &Path) -> Command {
        let mut command = meta_core::git::git(dir);
        command.args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@test.com",
            "-c",
            "commit.gpgsign=false",
        ]);
        command
    }

    #[test]
    fn checkout_guard_rejects_missing_plain_and_broken_repositories() {
        let tmp = init_git_repo();
        let plain = tmp.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        for path in [&tmp.path().join("missing"), &plain] {
            let err = ensure_checkout_config_is_not_bare(path).unwrap_err();
            assert!(
                err.to_string().contains(&path.display().to_string()),
                "{err}"
            );
        }
        std::fs::write(plain.join(".git"), "gitdir: nowhere\n").unwrap();
        assert!(ensure_checkout_config_is_not_bare(&plain).is_err());
    }

    // macOS and Windows file systems refuse a name that isn't UTF-8
    #[cfg(target_os = "linux")]
    #[test]
    fn worktree_add_preserves_non_utf8_destination() {
        use std::os::unix::ffi::OsStrExt;

        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        let dest = tmp.path().join(std::ffi::OsStr::from_bytes(b"wt-\xff"));
        git_worktree_add(tmp.path(), &dest, "topic", None).unwrap();
        assert!(dest.join(".git").is_file());
        assert!(!tmp.path().join("wt-\u{fffd}").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worktree_remove_preserves_non_utf8_destination() {
        use std::os::unix::ffi::OsStrExt;

        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        let dest = tmp.path().join(std::ffi::OsStr::from_bytes(b"wt-\xff"));
        let out = git(tmp.path())
            .args(["worktree", "add", "-b", "topic"])
            .arg(&dest)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        git_worktree_remove(&tmp.path().join(".git"), &dest, false).unwrap();
        assert!(!dest.exists());
    }

    #[test]
    fn delete_branch_reports_a_failed_config_cleanup() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        let head = git_rev_parse(tmp.path(), "HEAD").unwrap().unwrap();
        git(tmp.path()).args(["branch", "t"]).status().unwrap();
        git(tmp.path())
            .args(["config", "branch.t.remote", "origin"])
            .status()
            .unwrap();
        std::fs::write(tmp.path().join(".git/config.lock"), "locked").unwrap();

        let err = git_delete_branch_at(tmp.path(), "t", &head).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("git config --local --remove-section branch.t"),
            "{message}"
        );
        assert!(
            message.contains(&tmp.path().display().to_string()),
            "{message}"
        );
        assert!(message.contains("lock"), "{message}");
    }

    #[test]
    fn diff_handles_unrelated_histories_but_rejects_a_bad_ref() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        git(tmp.path()).args(["branch", "base"]).status().unwrap();
        git(tmp.path())
            .args(["checkout", "-q", "--orphan", "other"])
            .status()
            .unwrap();
        assert!(git(tmp.path())
            .args(["rm", "-qf", "README.md"])
            .status()
            .unwrap()
            .success());
        std::fs::write(tmp.path().join("other"), "other\n").unwrap();
        git(tmp.path()).args(["add", "other"]).status().unwrap();
        git(tmp.path())
            .args(["commit", "-qm", "unrelated"])
            .status()
            .unwrap();

        let (files, added, deleted, _) = git_diff_stat(tmp.path(), "base").unwrap();
        assert_eq!((files, added, deleted), (2, 1, 1));
        let err = git_diff_stat(tmp.path(), "no-such-base").unwrap_err();
        assert!(err.to_string().contains("no-such-base"), "{err}");
    }

    #[test]
    fn a_failed_fetch_is_reported_on_one_line() {
        let tmp = init_git_repo();
        git(tmp.path())
            .args(["remote", "add", "origin", "/nonexistent/r\u{1b}[2J\nFORGED"])
            .status()
            .unwrap();
        let err = git_fetch_branch(tmp.path(), "b\u{1b}[2J\nFORGED").unwrap_err();
        let message = format!("{err:#}");
        assert!(
            !message.contains('\n') && !message.contains('\u{1b}'),
            "{message}"
        );
    }

    /// Create a minimal git repo in a tempdir and return the tempdir handle.
    fn init_git_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path())
            .args(["init"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        tmp
    }

    /// Create an initial commit so HEAD exists.
    fn make_initial_commit(repo: &std::path::Path) {
        std::fs::write(repo.join("README.md"), "init\n").unwrap();
        git(repo).args(["add", "README.md"]).status().unwrap();
        git(repo)
            .args(["commit", "-m", "initial"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
    }

    // ── git_delete_branch_at ────────────────────────────────

    #[test]
    fn delete_branch_keeps_a_branch_that_moved() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        let start = git_rev_parse(tmp.path(), "HEAD").unwrap().unwrap();
        git(tmp.path()).args(["branch", "t"]).status().unwrap();
        std::fs::write(tmp.path().join("more"), "x").unwrap();
        git(tmp.path()).args(["add", "more"]).status().unwrap();
        git(tmp.path())
            .args(["commit", "-q", "-m", "more"])
            .status()
            .unwrap();
        let moved = git_rev_parse(tmp.path(), "HEAD").unwrap().unwrap();
        git(tmp.path())
            .args(["config", "branch.t.foo.remote", "origin"])
            .status()
            .unwrap();
        git(tmp.path())
            .args(["update-ref", "refs/heads/t", &moved])
            .status()
            .unwrap();

        assert!(git_delete_branch_at(tmp.path(), "t", &start).is_err());
        assert_eq!(
            git_rev_parse(tmp.path(), "refs/heads/t").unwrap(),
            Some(moved.clone())
        );
        git_delete_branch_at(tmp.path(), "t", &moved).unwrap();
        assert_eq!(git_rev_parse(tmp.path(), "refs/heads/t").unwrap(), None);
        assert_eq!(
            run(tmp.path(), ["config", "branch.t.foo.remote"]).unwrap(),
            "origin"
        );
    }

    #[test]
    fn branch_is_free_reads_reservations_and_changes_nothing() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        git(tmp.path()).args(["branch", "t"]).status().unwrap();
        git(tmp.path())
            .args(["config", "branch.autoSetupMerge", "always"])
            .status()
            .unwrap();
        git(tmp.path())
            .args(["config", "branch.t.remote", "origin"])
            .status()
            .unwrap();
        git_branch_is_free(tmp.path(), "t").unwrap();
        let remote = git(tmp.path())
            .args(["config", "branch.t.remote"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&remote.stdout).trim(), "origin");

        // Checked out in another worktree.
        let other = tmp.path().join("other");
        git(tmp.path())
            .args(["worktree", "add", "-q", other.to_str().unwrap(), "t"])
            .status()
            .unwrap();
        assert!(git_branch_is_free(tmp.path(), "t").is_err());
        // Bisected there instead: HEAD detached, branch still reserved.
        git(&other)
            .args(["checkout", "-q", "--detach"])
            .status()
            .unwrap();
        git_branch_is_free(tmp.path(), "t").unwrap();
        let admin = tmp.path().join(".git/worktrees/other");
        std::fs::write(admin.join("BISECT_START"), "t\n").unwrap();
        assert!(git_branch_is_free(tmp.path(), "t").is_err());
    }

    #[test]
    fn branch_is_free_sees_checkouts_in_a_reftable_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let init = git(tmp.path())
            .args(["init", "-q", "--ref-format=reftable"])
            .arg(&repo)
            .output()
            .unwrap();
        if !init.status.success() {
            return; // git older than 2.45 has no reftable support
        }
        make_initial_commit(&repo);
        assert!(git_rev_parse(&repo, "HEAD").unwrap().is_some());
        git(&repo).args(["branch", "t"]).status().unwrap();
        git_branch_is_free(&repo, "t").unwrap();
        let other = tmp.path().join("other");
        git(&repo)
            .args(["worktree", "add", "-q", other.to_str().unwrap(), "t"])
            .status()
            .unwrap();
        assert!(git_branch_is_free(&repo, "t").is_err());
    }

    #[test]
    fn a_symbolic_branch_is_never_moved_or_deleted() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        let main = git_rev_parse(tmp.path(), "HEAD").unwrap().unwrap();
        let current = git(tmp.path())
            .args(["symbolic-ref", "HEAD"])
            .output()
            .unwrap();
        let current = String::from_utf8_lossy(&current.stdout).trim().to_string();
        git(tmp.path())
            .args(["symbolic-ref", "refs/heads/t", &current])
            .status()
            .unwrap();

        assert!(git_branch_is_free(tmp.path(), "t").is_err());
        assert!(git_delete_branch_at(tmp.path(), "t", &main).is_err());
        assert_eq!(git_rev_parse(tmp.path(), &current).unwrap(), Some(main));
    }

    #[test]
    fn checkout_config_reports_core_bare_corruption_with_scoped_repair() {
        let tmp = init_git_repo();
        let status = git(tmp.path())
            .args(["config", "--local", "core.bare", "true"])
            .status()
            .unwrap();
        assert!(status.success());

        let error = repo_status(tmp.path()).err().unwrap().to_string();
        assert!(error.contains("core.bare=true"));
        assert!(error.contains(&format!(
            "git -C {} config --local core.bare false",
            meta_core::display::shell_quote(&tmp.path().to_string_lossy())
        )));
    }

    #[test]
    fn checkout_config_guard_allows_legitimate_bare_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let status = git(tmp.path())
            .args(["init", "--bare", "--quiet"])
            .status()
            .unwrap();
        assert!(status.success());

        ensure_checkout_config_is_not_bare(tmp.path()).unwrap();
    }
}
