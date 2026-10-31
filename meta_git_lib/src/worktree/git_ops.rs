//! Git operations for worktree management.

use anyhow::Result;
use meta_core::display::{escape, escape_path};
use meta_core::git::{git, RepoStatus};
use std::path::Path;
use std::process::Stdio;

/// An error if the checkout at `repo_path` has `core.bare=true` in its
/// config: git would make no worktree of it.
pub fn ensure_checkout_config_is_not_bare(repo_path: &Path) -> Result<()> {
    if !repo_path.join(".git").exists() {
        return Ok(());
    }

    let output = git(repo_path)
        .args(["config", "--local", "--bool", "--get", "core.bare"])
        .output()?;

    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout);
        if value.trim() == "true" {
            anyhow::bail!(
                "Repository '{}' is a checked-out work tree but its shared Git config has \
                 core.bare=true.\nRepair it with:\n  git -C {} config --local core.bare false",
                escape_path(repo_path),
                escape(&meta_core::display::shell_quote(
                    &repo_path.to_string_lossy()
                ))
            );
        }
    } else if output.status.code() != Some(1) {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "Failed to inspect core.bare for '{}': {}",
            escape_path(repo_path),
            escape(stderr.trim())
        );
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
    let dest = worktree_dest.to_string_lossy();
    let remote_ref = format!("origin/{branch}");
    let args: Vec<&str> = match &start {
        WorktreeStart::Ref { name, .. } => vec!["worktree", "add", "-b", branch, &dest, name],
        WorktreeStart::Existing => vec!["worktree", "add", &dest, branch],
        WorktreeStart::Remote { .. } => {
            vec![
                "worktree",
                "add",
                "--track",
                "-b",
                branch,
                &dest,
                &remote_ref,
            ]
        }
        WorktreeStart::Head { .. } => vec!["worktree", "add", "-b", branch, &dest],
    };
    let output = git(repo_path).args(&args).output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let from = match &start {
            WorktreeStart::Ref { name, .. } => format!(", ref: {}", escape(name)),
            _ => String::new(),
        };
        anyhow::bail!(
            "git worktree add failed for '{}' (branch: {}{from}): {}",
            escape_path(repo_path),
            escape(branch),
            escape(stderr.trim())
        );
    }
    Ok(start.new_branch_at())
}

pub fn git_worktree_remove(repo_path: &Path, worktree_path: &Path, force: bool) -> Result<()> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    let wt_str = worktree_path.to_string_lossy();
    args.push(&wt_str);

    let output = git(repo_path).args(&args).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git worktree remove failed: {}", escape(stderr.trim()));
    }
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
    // Try three-dot diff first (changes since divergence)
    let numstat_output = git(worktree_path)
        .args(["diff", "--numstat", &format!("{base_ref}...HEAD")])
        .stderr(Stdio::null())
        .output()?;

    let numstat_text = if numstat_output.status.success() {
        String::from_utf8_lossy(&numstat_output.stdout).to_string()
    } else {
        // Fallback to two-dot diff
        let fallback = git(worktree_path)
            .args(["diff", "--numstat", &format!("{base_ref}..HEAD")])
            .stderr(Stdio::null())
            .output()?;
        if fallback.status.success() {
            String::from_utf8_lossy(&fallback.stdout).to_string()
        } else {
            String::new()
        }
    };

    let mut files_changed = 0;
    let mut insertions = 0;
    let mut deletions = 0;
    let mut files = Vec::new();

    for line in numstat_text.lines() {
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 3 {
            files_changed += 1;
            insertions += parts[0].parse::<usize>().unwrap_or(0);
            deletions += parts[1].parse::<usize>().unwrap_or(0);
            files.push(parts[2].to_string());
        }
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
    let output = git(repo_path)
        .args(["rev-parse", "--verify", "--quiet", rev])
        .output()?;
    if output.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() == Some(1) && stderr.trim().is_empty() {
        return Ok(None);
    }
    anyhow::bail!(
        "git rev-parse {} failed in {}: {}",
        escape(rev),
        escape_path(repo_path),
        escape(stderr.trim())
    )
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
    let symbolic = git(repo_path)
        .args(["symbolic-ref", "-q", &full])
        .output()?;
    match symbolic.status.code() {
        Some(1) => {}
        Some(0) => return Err(in_use("a symbolic ref; meta only moves plain branches")),
        _ => anyhow::bail!(
            "Cannot inspect '{}': {}",
            escape(branch),
            escape(String::from_utf8_lossy(&symbolic.stderr).trim())
        ),
    }

    let list = git(repo_path)
        .args(["worktree", "list", "--porcelain"])
        .output()?;
    anyhow::ensure!(
        list.status.success(),
        "Cannot list the worktrees of {}: {}",
        escape_path(repo_path),
        escape(String::from_utf8_lossy(&list.stderr).trim())
    );
    let wanted = format!("branch {full}");
    if String::from_utf8_lossy(&list.stdout)
        .lines()
        .any(|line| line == wanted)
    {
        return Err(in_use("checked out in a worktree"));
    }

    let output = git(repo_path)
        .args(["rev-parse", "--git-common-dir"])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "Cannot find the git directory of {}: {}",
        escape_path(repo_path),
        escape(String::from_utf8_lossy(&output.stderr).trim())
    );
    // Relative output is relative to the repository, where git ran.
    let common =
        repo_path.join(String::from_utf8_lossy(&output.stdout).trim_end_matches(['\n', '\r']));
    anyhow::ensure!(
        common.is_dir(),
        "Cannot find the git directory of {} (got {})",
        escape_path(repo_path),
        escape_path(&common)
    );
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
    let output = git(repo_path)
        .args([
            "update-ref",
            "--no-deref",
            "-d",
            &format!("refs/heads/{branch}"),
            expected,
        ])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "Branch '{}' no longer points where it was created; kept ({})",
            escape(branch),
            escape(stderr.trim())
        );
    }
    // Upstream settings of a deleted branch; there may be none.
    let _ = git(repo_path)
        .args(["config", "--remove-section", &format!("branch.{branch}")])
        .output();
    Ok(())
}

/// Fetch a branch from origin if not locally available.
pub fn git_fetch_branch(repo_path: &Path, branch: &str) -> Result<()> {
    let output = git(repo_path)
        .args(["fetch", "origin", branch])
        .stderr(Stdio::piped())
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // git's own errors may show the URL with its credentials, and a
        // remote helper origin's address in any form (see GitUrl::hide_in)
        let mut shown = meta_core::git::redact_git_output(&stderr)
            .trim_ascii()
            .to_string();
        if let Some(origin) = meta_core::git::remote_url(repo_path) {
            shown = origin.hide_in(&shown).into_owned();
        }
        anyhow::bail!(
            "Failed to fetch branch '{}': {}",
            escape(branch),
            escape(&shown)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn a_failed_fetch_shows_no_credentials() {
        let tmp = init_git_repo();
        // git refuses this URL before connecting, and quotes it whole
        git(tmp.path())
            .args([
                "remote",
                "add",
                "origin",
                "http://a:ghp_SECRET%0A@127.0.0.1:9/r",
            ])
            .status()
            .unwrap();
        let message = format!("{:#}", git_fetch_branch(tmp.path(), "b").unwrap_err());
        assert!(message.contains("http://***@127.0.0.1:9/r"), "{message}");
        assert!(!message.contains("ghp_SECRET"), "{message}");
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
        git(tmp.path())
            .args(["config", "user.email", "test@test.com"])
            .status()
            .unwrap();
        git(tmp.path())
            .args(["config", "user.name", "Test"])
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
        for (key, value) in [("user.name", "Test"), ("user.email", "test@test.com")] {
            git(&repo).args(["config", key, value]).status().unwrap();
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
