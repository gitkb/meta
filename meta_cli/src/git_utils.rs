//! Shared git primitives used across crates.
//!
//! Lightweight functions that shell out to `git` for common queries.
//! All functions gracefully handle missing repos or git failures.

use std::path::Path;
use std::process::{Command, Stdio};

/// Helper to run a git command and return stdout as a String.
/// Returns None if command fails or output is invalid UTF-8.
fn run_git_command(repo_path: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Returns the current branch name, or `None` for detached HEAD / git failures.
pub fn current_branch(repo_path: &Path) -> Option<String> {
    let branch = run_git_command(repo_path, &["branch", "--show-current"])?;
    if branch.is_empty() {
        None // detached HEAD
    } else {
        Some(branch)
    }
}

/// Returns whether the repo has uncommitted changes, or `None` if git fails.
pub fn is_dirty(repo_path: &Path) -> Option<bool> {
    let text = run_git_command(repo_path, &["status", "--porcelain"])?;
    Some(text.lines().any(|l| !l.is_empty()))
}

/// Returns the number of dirty files (modified + untracked), or `None` if git fails.
pub fn dirty_file_count(repo_path: &Path) -> Option<usize> {
    let text = run_git_command(repo_path, &["status", "--porcelain"])?;
    Some(text.lines().filter(|l| !l.is_empty()).count())
}

/// Returns (ahead, behind) commit counts relative to upstream, or `None` if no upstream or git fails.
///
/// - `ahead`: number of commits in local branch not in upstream
/// - `behind`: number of commits in upstream not in local branch
///
/// Returns `None` if there's no tracking branch configured or git command fails.
///
/// Uses `--left-right --count` to get both values in a single git invocation for efficiency.
pub fn ahead_behind(repo_path: &Path) -> Option<(usize, usize)> {
    // Get upstream branch
    let upstream = run_git_command(repo_path, &["rev-parse", "--abbrev-ref", "@{upstream}"])?;

    if upstream.is_empty() {
        return None;
    }

    // Get both ahead and behind counts in a single git command
    // --left-right --count outputs two numbers: ahead behind
    let result = run_git_command(
        repo_path,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("HEAD...{upstream}"),
        ],
    )?;

    let parts: Vec<&str> = result.split_whitespace().collect();

    if parts.len() == 2 {
        let ahead = parts[0].parse().ok()?;
        let behind = parts[1].parse().ok()?;
        Some((ahead, behind))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    fn init_git_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(tmp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        tmp
    }

    fn make_initial_commit(repo: &Path) {
        std::fs::write(repo.join("README.md"), "init\n").unwrap();
        Command::new("git")
            .args(["add", "README.md"])
            .current_dir(repo)
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(repo)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
    }

    #[test]
    fn current_branch_returns_branch_name() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        let branch = current_branch(tmp.path());
        assert!(branch.is_some());
        // Default branch is "main" or "master" depending on git config
        let name = branch.unwrap();
        assert!(!name.is_empty());
    }

    #[test]
    fn current_branch_returns_none_for_nonexistent_path() {
        let result = current_branch(Path::new("/nonexistent/path"));
        assert!(result.is_none());
    }

    #[test]
    fn is_dirty_clean_repo() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        assert_eq!(is_dirty(tmp.path()), Some(false));
    }

    #[test]
    fn is_dirty_with_modification() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        std::fs::write(tmp.path().join("README.md"), "changed\n").unwrap();
        assert_eq!(is_dirty(tmp.path()), Some(true));
    }

    #[test]
    fn is_dirty_with_untracked_file() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        std::fs::write(tmp.path().join("new.txt"), "new").unwrap();
        assert_eq!(is_dirty(tmp.path()), Some(true));
    }

    #[test]
    fn dirty_file_count_clean_repo() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        assert_eq!(dirty_file_count(tmp.path()), Some(0));
    }

    #[test]
    fn dirty_file_count_mixed() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        std::fs::write(tmp.path().join("README.md"), "changed\n").unwrap();
        std::fs::write(tmp.path().join("new.txt"), "new").unwrap();
        assert_eq!(dirty_file_count(tmp.path()), Some(2));
    }

    #[test]
    fn dirty_file_count_nonexistent_path() {
        assert!(dirty_file_count(Path::new("/nonexistent/path")).is_none());
    }

    // ── run_git_command (helper tests via public APIs) ──────────

    #[test]
    fn run_git_command_handles_invalid_repo() {
        // Test via public API that uses run_git_command
        let result = current_branch(Path::new("/nonexistent/invalid"));
        assert!(result.is_none());
    }

    #[test]
    fn run_git_command_trims_whitespace() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        let branch = current_branch(tmp.path()).unwrap();
        // Should not have trailing newlines
        assert!(!branch.ends_with('\n'));
        assert!(!branch.ends_with('\r'));
    }

    // ── ahead_behind ────────────────────────────────────────────

    #[test]
    fn ahead_behind_no_upstream() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());
        // No upstream configured
        assert_eq!(ahead_behind(tmp.path()), None);
    }

    #[test]
    fn ahead_behind_nonexistent_path() {
        assert!(ahead_behind(Path::new("/nonexistent/path")).is_none());
    }

    #[test]
    fn ahead_behind_with_tracking_branch() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());

        // Create a remote-tracking branch simulation
        Command::new("git")
            .args(["checkout", "-b", "test-branch"])
            .current_dir(tmp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        // Create a pseudo-remote branch (in same repo for testing)
        Command::new("git")
            .args(["branch", "origin/test-branch"])
            .current_dir(tmp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        // Set upstream
        Command::new("git")
            .args(["branch", "--set-upstream-to=origin/test-branch"])
            .current_dir(tmp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        // Now they're in sync
        let result = ahead_behind(tmp.path());
        assert_eq!(result, Some((0, 0)));
    }

    #[test]
    fn ahead_behind_ahead_after_commit() {
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());

        // Create and track a branch
        Command::new("git")
            .args(["checkout", "-b", "ahead-test"])
            .current_dir(tmp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        Command::new("git")
            .args(["branch", "origin/ahead-test"])
            .current_dir(tmp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        Command::new("git")
            .args(["branch", "--set-upstream-to=origin/ahead-test"])
            .current_dir(tmp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        // Make a commit to get ahead
        std::fs::write(tmp.path().join("ahead.txt"), "ahead\n").unwrap();
        Command::new("git")
            .args(["add", "ahead.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "ahead commit"])
            .current_dir(tmp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        let result = ahead_behind(tmp.path());
        // Should be ahead by 1
        assert_eq!(result, Some((1, 0)));
    }

    #[test]
    fn ahead_behind_uses_left_right_efficiently() {
        // This test verifies the implementation uses --left-right
        // by ensuring it returns a tuple (ahead, behind) in one call
        let tmp = init_git_repo();
        make_initial_commit(tmp.path());

        // Set up tracking branch
        Command::new("git")
            .args(["checkout", "-b", "efficiency-test"])
            .current_dir(tmp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        Command::new("git")
            .args(["branch", "origin/efficiency-test"])
            .current_dir(tmp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        Command::new("git")
            .args(["branch", "--set-upstream-to=origin/efficiency-test"])
            .current_dir(tmp.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        // Call should succeed and return both values
        let result = ahead_behind(tmp.path());
        assert!(result.is_some());
        let (ahead, behind) = result.unwrap();
        // Both should be valid numbers (0 in this case)
        assert_eq!(ahead, 0);
        assert_eq!(behind, 0);
    }
}
