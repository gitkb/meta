//! Git operations for worktree management.

use anyhow::Result;
use meta_core::display::{escape, escape_path};
use std::path::Path;
use std::process::{Command, Stdio};

use super::types::GitStatusSummary;

const GIT_LOCAL_ENV_VARS: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CONFIG",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    "GIT_DIR",
    "GIT_GRAFT_FILE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_INTERNAL_SUPER_PREFIX",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "GIT_REPLACE_REF_BASE",
    "GIT_SHALLOW_FILE",
    "GIT_WORK_TREE",
];

fn clear_git_local_env(command: &mut Command) {
    for key in GIT_LOCAL_ENV_VARS {
        command.env_remove(key);
    }
}

pub(crate) fn git_command(repo_path: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(repo_path);
    clear_git_local_env(&mut command);
    command
}

fn shell_quote_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn ensure_checkout_config_is_not_bare(repo_path: &Path) -> Result<()> {
    if !repo_path.join(".git").exists() {
        return Ok(());
    }

    let output = git_command(repo_path)
        .args(["config", "--local", "--bool", "--get", "core.bare"])
        .output()?;

    if output.status.success() {
        let value = String::from_utf8_lossy(&output.stdout);
        if value.trim() == "true" {
            anyhow::bail!(
                "Repository '{}' is a checked-out work tree but its shared Git config has \
                 core.bare=true.\nRepair it with:\n  git -C {} config --local core.bare false",
                escape_path(repo_path),
                escape(&shell_quote_path(repo_path))
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

pub fn git_worktree_add(
    repo_path: &Path,
    worktree_dest: &Path,
    branch: &str,
    from_ref: Option<&str>,
) -> Result<Option<String>> {
    ensure_checkout_config_is_not_bare(repo_path)?;
    let dest_str = worktree_dest.to_string_lossy();

    // If from_ref is specified, create the branch at the commit it names
    if let Some(ref_name) = from_ref {
        let start =
            git_rev_parse(repo_path, &format!("{ref_name}^{{commit}}"))?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Ref '{}' not found in repo '{}'",
                    ref_name,
                    escape_path(repo_path)
                )
            })?;
        // Pass the name, not the commit: a remote-tracking ref still sets the
        // branch's upstream. If it moved meanwhile, the branch isn't at
        // `start` and a rollback keeps it.
        let output = git_command(repo_path)
            .args(["worktree", "add", "-b", branch, &dest_str, ref_name])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "git worktree add failed for '{}' (branch: {}, ref: {}): {}",
                escape_path(repo_path),
                escape(branch),
                escape(ref_name),
                escape(stderr.trim())
            );
        }
        return Ok(Some(start)); // Always creates a new branch from ref
    }

    let branch_exists = git_rev_parse(repo_path, &format!("refs/heads/{branch}"))?.is_some();
    let remote_ref = format!("origin/{branch}");
    let remote_start = if branch_exists {
        None
    } else {
        git_rev_parse(repo_path, &format!("refs/remotes/{remote_ref}"))?
    };

    let (wt_args, start): (Vec<&str>, Option<String>) = if branch_exists {
        // Use existing local branch
        (vec!["worktree", "add", &dest_str, branch], None)
    } else if let Some(start) = remote_start {
        // Create local tracking branch from remote
        let args = vec![
            "worktree",
            "add",
            "--track",
            "-b",
            branch,
            &dest_str,
            &remote_ref,
        ];
        (args, Some(start))
    } else {
        // Create new branch from HEAD
        let start = git_rev_parse(repo_path, "HEAD")?.ok_or_else(|| {
            anyhow::anyhow!("{} has no commit to branch from", escape_path(repo_path))
        })?;
        (
            vec!["worktree", "add", "-b", branch, &dest_str],
            Some(start),
        )
    };

    let output = git_command(repo_path).args(&wt_args).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "git worktree add failed for '{}' (branch: {}): {}",
            escape_path(repo_path),
            escape(branch),
            escape(stderr.trim())
        );
    }

    // The commit the branch was created at (`-b` refuses an existing branch,
    // so success proves this call made it), or None for an existing branch.
    Ok(start)
}

pub fn git_worktree_remove(repo_path: &Path, worktree_path: &Path, force: bool) -> Result<()> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    let wt_str = worktree_path.to_string_lossy();
    args.push(&wt_str);

    let output = git_command(repo_path).args(&args).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git worktree remove failed: {}", escape(stderr.trim()));
    }
    Ok(())
}

pub fn git_status_summary(repo_path: &Path) -> Result<GitStatusSummary> {
    ensure_checkout_config_is_not_bare(repo_path)?;

    let output = git_command(repo_path)
        .args(["status", "--porcelain"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "git status failed for '{}': {}",
            escape_path(repo_path),
            escape(stderr.trim())
        );
    }

    let mut modified_files = Vec::new();
    let mut untracked_count = 0;

    // git status --porcelain format: "XY filename"
    // Positions 0-1: index (X) and work-tree (Y) status codes
    // Position 2: space separator
    // Position 3+: filename
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.len() < 3 {
            continue;
        }
        let status = &line[..2];
        let file = &line[3..];

        if status == "??" {
            untracked_count += 1;
        } else if !file.is_empty() {
            // Tracked file with modifications (staged, unstaged, or both).
            // For renames ("R  old -> new"), extract the new name.
            let name = file.split(" -> ").last().unwrap_or(file);
            modified_files.push(name.to_string());
        }
    }

    let dirty = !modified_files.is_empty() || untracked_count > 0;
    Ok(GitStatusSummary {
        dirty,
        modified_files,
        untracked_count,
    })
}

pub fn git_ahead_behind(repo_path: &Path) -> Result<(u32, u32)> {
    let output = git_command(repo_path)
        .args(["rev-list", "--left-right", "--count", "HEAD...@{upstream}"])
        .stderr(Stdio::null())
        .output()?;

    if !output.status.success() {
        // No upstream configured
        return Ok((0, 0));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let parts: Vec<&str> = text.trim().split('\t').collect();
    if parts.len() == 2 {
        let ahead = parts[0].parse::<u32>().unwrap_or(0);
        let behind = parts[1].parse::<u32>().unwrap_or(0);
        Ok((ahead, behind))
    } else {
        Ok((0, 0))
    }
}

pub fn git_diff_stat(
    worktree_path: &Path,
    base_ref: &str,
) -> Result<(usize, usize, usize, Vec<String>)> {
    // Try three-dot diff first (changes since divergence)
    let numstat_output = git_command(worktree_path)
        .args(["diff", "--numstat", &format!("{base_ref}...HEAD")])
        .stderr(Stdio::null())
        .output()?;

    let numstat_text = if numstat_output.status.success() {
        String::from_utf8_lossy(&numstat_output.stdout).to_string()
    } else {
        // Fallback to two-dot diff
        let fallback = git_command(worktree_path)
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
    let output = git_command(repo_path)
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
    let symbolic = git_command(repo_path)
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

    let list = git_command(repo_path)
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

    let output = git_command(repo_path)
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
    let output = git_command(repo_path)
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
    let _ = git_command(repo_path)
        .args(["config", "--remove-section", &format!("branch.{branch}")])
        .output();
    Ok(())
}

/// Fetch a branch from origin if not locally available.
pub fn git_fetch_branch(repo_path: &Path, branch: &str) -> Result<()> {
    let output = git_command(repo_path)
        .args(["fetch", "origin", branch])
        .stderr(Stdio::piped())
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "Failed to fetch branch '{}': {}",
            escape(branch),
            escape(stderr.trim())
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn a_failed_fetch_is_reported_on_one_line() {
        let tmp = init_git_repo();
        git_command(tmp.path())
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
        git_command(tmp.path())
            .args(["init"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        git_command(tmp.path())
            .args(["config", "user.email", "test@test.com"])
            .status()
            .unwrap();
        git_command(tmp.path())
            .args(["config", "user.name", "Test"])
            .status()
            .unwrap();
        tmp
    }

    /// Create an initial commit so HEAD exists.
    fn make_initial_commit(repo: &std::path::Path) {
        std::fs::write(repo.join("README.md"), "init\n").unwrap();
        git_command(repo)
            .args(["add", "README.md"])
            .status()
            .unwrap();
        git_command(repo)
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
        git_command(tmp.path())
            .args(["branch", "t"])
            .status()
            .unwrap();
        std::fs::write(tmp.path().join("more"), "x").unwrap();
        git_command(tmp.path())
            .args(["add", "more"])
            .status()
            .unwrap();
        git_command(tmp.path())
            .args(["commit", "-q", "-m", "more"])
            .status()
            .unwrap();
        let moved = git_rev_parse(tmp.path(), "HEAD").unwrap().unwrap();
        git_command(tmp.path())
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
        git_command(tmp.path())
            .args(["branch", "t"])
            .status()
            .unwrap();
        git_command(tmp.path())
            .args(["config", "branch.autoSetupMerge", "always"])
            .status()
            .unwrap();
        git_command(tmp.path())
            .args(["config", "branch.t.remote", "origin"])
            .status()
            .unwrap();
        git_branch_is_free(tmp.path(), "t").unwrap();
        let remote = git_command(tmp.path())
            .args(["config", "branch.t.remote"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&remote.stdout).trim(), "origin");

        // Checked out in another worktree.
        let other = tmp.path().join("other");
        git_command(tmp.path())
            .args(["worktree", "add", "-q", other.to_str().unwrap(), "t"])
            .status()
            .unwrap();
        assert!(git_branch_is_free(tmp.path(), "t").is_err());
        // Bisected there instead: HEAD detached, branch still reserved.
        git_command(&other)
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
        let init = git_command(tmp.path())
            .args(["init", "-q", "--ref-format=reftable"])
            .arg(&repo)
            .output()
            .unwrap();
        if !init.status.success() {
            return; // git older than 2.45 has no reftable support
        }
        for (key, value) in [("user.name", "Test"), ("user.email", "test@test.com")] {
            git_command(&repo)
                .args(["config", key, value])
                .status()
                .unwrap();
        }
        make_initial_commit(&repo);
        assert!(git_rev_parse(&repo, "HEAD").unwrap().is_some());
        git_command(&repo).args(["branch", "t"]).status().unwrap();
        git_branch_is_free(&repo, "t").unwrap();
        let other = tmp.path().join("other");
        git_command(&repo)
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
        let current = git_command(tmp.path())
            .args(["symbolic-ref", "HEAD"])
            .output()
            .unwrap();
        let current = String::from_utf8_lossy(&current.stdout).trim().to_string();
        git_command(tmp.path())
            .args(["symbolic-ref", "refs/heads/t", &current])
            .status()
            .unwrap();

        assert!(git_branch_is_free(tmp.path(), "t").is_err());
        assert!(git_delete_branch_at(tmp.path(), "t", &main).is_err());
        assert_eq!(git_rev_parse(tmp.path(), &current).unwrap(), Some(main));
    }

    // ── git_status_summary ──────────────────────────────────

    #[test]
    fn git_local_env_vars_cover_git_reported_variables() {
        let output = Command::new("git")
            .args(["rev-parse", "--local-env-vars"])
            .output()
            .expect("git rev-parse should run");
        assert!(
            output.status.success(),
            "git rev-parse --local-env-vars failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let reported: BTreeSet<_> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
        let configured: BTreeSet<_> = GIT_LOCAL_ENV_VARS
            .iter()
            .map(|value| (*value).to_string())
            .collect();

        for name in &reported {
            assert!(
                configured.contains(name),
                "GIT_LOCAL_ENV_VARS is missing git-reported local env var {name}"
            );
        }

        let allowed_extra: BTreeSet<_> = ["GIT_INTERNAL_SUPER_PREFIX".to_string()]
            .into_iter()
            .collect();
        for name in configured.difference(&reported) {
            assert!(
                allowed_extra.contains(name),
                "GIT_LOCAL_ENV_VARS contains unexpected non-local env var {name}"
            );
        }
    }

    #[test]
    fn clear_git_local_env_removes_configured_variables() {
        let mut command = Command::new("sh");
        command.args(["-c", "env"]);
        for key in GIT_LOCAL_ENV_VARS {
            command.env(key, "hostile");
        }

        clear_git_local_env(&mut command);
        let output = command.output().expect("environment probe should run");
        assert!(output.status.success());

        let environment = String::from_utf8(output.stdout).expect("environment should be UTF-8");
        for key in GIT_LOCAL_ENV_VARS {
            assert!(
                !environment
                    .lines()
                    .any(|line| line.starts_with(&format!("{key}="))),
                "{key} leaked into the subprocess environment"
            );
        }
    }

    #[test]
    fn checkout_config_reports_core_bare_corruption_with_scoped_repair() {
        let tmp = init_git_repo();
        let status = git_command(tmp.path())
            .args(["config", "--local", "core.bare", "true"])
            .status()
            .unwrap();
        assert!(status.success());

        let error = git_status_summary(tmp.path()).err().unwrap().to_string();
        assert!(error.contains("core.bare=true"));
        assert!(error.contains(&format!(
            "git -C '{}' config --local core.bare false",
            tmp.path().display()
        )));
    }

    #[test]
    fn repair_command_shell_quotes_repository_path() {
        assert_eq!(
            shell_quote_path(Path::new("/tmp/operator's repo")),
            "'/tmp/operator'\"'\"'s repo'"
        );
    }

    #[test]
    fn checkout_config_guard_allows_legitimate_bare_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let status = git_command(tmp.path())
            .args(["init", "--bare", "--quiet"])
            .status()
            .unwrap();
        assert!(status.success());

        ensure_checkout_config_is_not_bare(tmp.path()).unwrap();
    }

    #[test]
    fn status_summary_clean_repo() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());

        let summary = git_status_summary(tmp.path()).unwrap();
        assert!(!summary.dirty);
        assert!(summary.modified_files.is_empty());
        assert_eq!(summary.untracked_count, 0);
    }

    #[test]
    fn status_summary_untracked_file() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());

        std::fs::write(tmp.path().join("new.txt"), "hello").unwrap();

        let summary = git_status_summary(tmp.path()).unwrap();
        assert!(summary.dirty);
        assert_eq!(summary.untracked_count, 1);
        assert!(summary.modified_files.is_empty());
    }

    #[test]
    fn status_summary_modified_tracked_file() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());

        // Modify an existing tracked file
        std::fs::write(tmp.path().join("README.md"), "changed\n").unwrap();

        let summary = git_status_summary(tmp.path()).unwrap();
        assert!(summary.dirty);
        assert_eq!(summary.untracked_count, 0);
        assert!(summary.modified_files.contains(&"README.md".to_string()));
    }

    #[test]
    fn status_summary_staged_file() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());

        std::fs::write(tmp.path().join("README.md"), "staged\n").unwrap();
        git_command(tmp.path())
            .args(["add", "README.md"])
            .status()
            .unwrap();

        let summary = git_status_summary(tmp.path()).unwrap();
        assert!(summary.dirty);
        assert!(summary.modified_files.contains(&"README.md".to_string()));
    }

    #[test]
    fn status_summary_mixed_state() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());

        // Untracked
        std::fs::write(tmp.path().join("untracked.txt"), "u").unwrap();
        // Modified tracked
        std::fs::write(tmp.path().join("README.md"), "mod\n").unwrap();

        let summary = git_status_summary(tmp.path()).unwrap();
        assert!(summary.dirty);
        assert_eq!(summary.untracked_count, 1);
        assert!(summary.modified_files.contains(&"README.md".to_string()));
    }

    // ── git_ahead_behind ────────────────────────────────────

    #[test]
    fn ahead_behind_no_upstream_returns_zeros() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());

        let (ahead, behind) = git_ahead_behind(tmp.path()).unwrap();
        assert_eq!(ahead, 0);
        assert_eq!(behind, 0);
    }
}
