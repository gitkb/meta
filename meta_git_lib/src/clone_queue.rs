use log::{debug, warn};
use meta_core::config;
use meta_core::display::escape_path;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// A clone task representing a single repository to clone
#[derive(Debug, Clone)]
pub struct CloneTask {
    /// Display name for progress output
    pub name: String,
    /// Git URL to clone from
    pub url: String,
    /// Target path to clone into
    pub target_path: PathBuf,
    /// Depth level (for display purposes)
    pub depth_level: usize,
    /// Whether this project is itself a meta-repo (declared with `meta: true` in config)
    pub is_meta: bool,
}

/// A clone that failed, with git's own error output.
#[derive(Debug, Clone)]
pub struct CloneFailure {
    pub name: String,
    pub url: String,
    pub error: String,
}

/// Thread-safe queue for managing clone tasks with dynamic discovery
pub struct CloneQueue {
    /// Pending tasks to process
    pending: Mutex<Vec<CloneTask>>,
    /// Completed task paths (to avoid duplicates)
    completed: Mutex<HashSet<PathBuf>>,
    /// Failed clones, in the order they failed
    failures: Mutex<Vec<CloneFailure>>,
    /// Total tasks discovered (for progress display)
    total_discovered: AtomicUsize,
    /// Total tasks finished, failed ones included (for progress display)
    total_completed: AtomicUsize,
    /// Git depth argument (if any)
    git_depth: Option<String>,
    /// Max meta depth for recursion (None = unlimited)
    meta_depth: Option<usize>,
}

impl CloneQueue {
    pub fn new(git_depth: Option<String>, meta_depth: Option<usize>) -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
            completed: Mutex::new(HashSet::new()),
            failures: Mutex::new(Vec::new()),
            total_discovered: AtomicUsize::new(0),
            total_completed: AtomicUsize::new(0),
            git_depth,
            meta_depth,
        }
    }

    /// Add a task to the queue if not already completed or pending
    pub fn push(&self, task: CloneTask) -> bool {
        let path = task.target_path.clone();

        // Lock both completed and pending atomically to prevent TOCTOU races
        let completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
        if completed.contains(&path) {
            return false;
        }

        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if pending.iter().any(|t| t.target_path == path) {
            return false;
        }
        pending.push(task);
        drop(pending);
        drop(completed);
        self.total_discovered.fetch_add(1, Ordering::SeqCst);

        true
    }

    /// Add multiple tasks from a .meta.yaml file
    pub fn push_from_meta(&self, base_dir: &Path, depth_level: usize) -> anyhow::Result<usize> {
        // Check meta depth limit
        if let Some(max_depth) = self.meta_depth {
            if depth_level > max_depth {
                debug!(
                    "Skipping nested discovery at depth {} (max: {})",
                    depth_level, max_depth
                );
                return Ok(0);
            }
        }

        let Some(meta_path) = config::find_meta_config_in(base_dir)? else {
            debug!("No .meta.yaml found in {}", escape_path(base_dir));
            return Ok(0);
        };

        let (projects, _) = config::parse_meta_config(&meta_path)?;
        debug!(
            "Discovered {} projects in {} at depth {}",
            projects.len(),
            escape_path(base_dir),
            depth_level
        );

        let mut added = 0;
        for project in projects {
            let target_path = base_dir.join(&project.path);

            // Skip if already exists
            if target_path.exists() {
                // But still look in it for nested discovery (push_from_meta
                // checks the depth limit before it looks for a config)
                added += self.push_from_meta(&target_path, depth_level + 1)?;
                continue;
            }

            // Skip projects without a repo URL (cannot clone)
            let Some(url) = project.repo else {
                continue;
            };

            let task = CloneTask {
                name: project.name.clone(),
                url,
                target_path,
                depth_level,
                is_meta: project.meta,
            };

            let task_name = task.name.clone();
            let task_is_meta = task.is_meta;
            if self.push(task) {
                debug!(
                    "Queued clone task: {} (depth: {}, is_meta: {})",
                    task_name, depth_level, task_is_meta
                );
                added += 1;
            }
        }

        Ok(added)
    }

    /// Take a single task from the queue (for worker threads)
    pub fn take_one(&self) -> Option<CloneTask> {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.pop()
    }

    /// Check if queue is finished (no pending and no active workers)
    pub fn is_finished(&self, active_workers: &AtomicUsize) -> bool {
        let pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.is_empty() && active_workers.load(Ordering::SeqCst) == 0
    }

    /// Drain all pending tasks (for dry-run display)
    pub fn drain_all(&self) -> Vec<CloneTask> {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.drain(..).collect()
    }

    /// Get current counts for display
    pub fn get_counts(&self) -> (usize, usize) {
        (
            self.total_completed.load(Ordering::SeqCst),
            self.total_discovered.load(Ordering::SeqCst),
        )
    }

    /// Get the git depth argument (if any)
    pub fn git_depth(&self) -> Option<&str> {
        self.git_depth.as_deref()
    }

    /// Mark a task as completed and check for nested .meta.yaml files
    pub fn mark_completed(&self, task: &CloneTask) -> anyhow::Result<usize> {
        self.total_completed.fetch_add(1, Ordering::SeqCst);

        {
            let mut completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
            completed.insert(task.target_path.clone());
        }

        // Check for nested .meta.yaml file and add children to queue
        let added = self.push_from_meta(&task.target_path, task.depth_level + 1)?;
        debug!(
            "mark_completed: {} -> {} nested tasks discovered",
            task.name, added
        );

        // Warn if the config declared meta: true but no nested .meta.yaml was found
        if task.is_meta && added == 0 {
            warn!(
                "'{}' is declared with `meta: true` but no .meta.yaml was found inside it",
                task.name
            );
        }

        Ok(added)
    }

    /// Get all URLs from pending tasks (for SSH target discovery).
    /// Does not remove tasks from the queue.
    pub fn peek_urls(&self) -> Vec<String> {
        let pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.iter().map(|t| t.url.clone()).collect()
    }

    /// Mark a task as failed, keeping git's error output for the final report
    pub fn mark_failed(&self, task: &CloneTask, error: String) {
        self.total_completed.fetch_add(1, Ordering::SeqCst);

        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        failures.push(CloneFailure {
            name: task.name.clone(),
            url: task.url.clone(),
            error,
        });
    }

    /// Clones that failed so far
    pub fn failures(&self) -> Vec<CloneFailure> {
        self.failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(name: &str, path: &Path) -> CloneTask {
        CloneTask {
            name: name.to_string(),
            url: format!("git@github.com:org/{name}.git"),
            target_path: path.to_path_buf(),
            depth_level: 0,
            is_meta: false,
        }
    }

    // ── push / dedup ──────────────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn push_from_meta_does_not_look_past_the_depth_limit() {
        // child exists with a dangling .meta.yaml; at depth limit 0 it is never
        // read, so seeding still queues the missing sibling
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("child")).unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"child": {"repo": "u", "meta": true}, "sib": "u2"}}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink("missing.yaml", dir.path().join("child/.meta.yaml")).unwrap();
        let queue = CloneQueue::new(None, Some(0));
        assert_eq!(queue.push_from_meta(dir.path(), 0).unwrap(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn push_from_meta_fails_on_a_nested_config_it_cannot_read() {
        // child/.meta.yaml is a link to nowhere: its repos are unknown, which is
        // an error, not "no nested repos"
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("child")).unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"child": {"repo": "u", "meta": true}}}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink("missing.yaml", dir.path().join("child/.meta.yaml")).unwrap();
        let queue = CloneQueue::new(None, None);
        let err = queue.push_from_meta(dir.path(), 0).unwrap_err();
        assert!(format!("{err:#}").contains("Cannot read"), "{err:#}");
    }

    #[test]
    fn push_adds_task_and_increments_count() {
        let queue = CloneQueue::new(None, None);
        let dir = tempfile::tempdir().unwrap();
        let task = make_task("repo1", &dir.path().join("repo1"));

        assert!(queue.push(task));
        assert_eq!(queue.total_discovered.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn push_dedup_by_path() {
        let queue = CloneQueue::new(None, None);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repo1");

        let task1 = make_task("repo1", &path);
        let task2 = make_task("repo1-dup", &path);

        assert!(queue.push(task1));
        assert!(!queue.push(task2)); // same path → rejected
        assert_eq!(queue.total_discovered.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn push_rejects_completed_path() {
        let queue = CloneQueue::new(None, None);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repo1");

        // Manually mark path as completed
        {
            let mut completed = queue.completed.lock().unwrap();
            completed.insert(path.clone());
        }

        let task = make_task("repo1", &path);
        assert!(!queue.push(task));
        assert_eq!(queue.total_discovered.load(Ordering::SeqCst), 0);
    }

    // ── take_one ──────────────────────────────────────────────

    #[test]
    fn take_one_returns_none_when_empty() {
        let queue = CloneQueue::new(None, None);
        assert!(queue.take_one().is_none());
    }

    #[test]
    fn take_one_returns_task_when_available() {
        let queue = CloneQueue::new(None, None);
        let dir = tempfile::tempdir().unwrap();
        queue.push(make_task("repo1", &dir.path().join("repo1")));

        let task = queue.take_one();
        assert!(task.is_some());
        assert_eq!(task.unwrap().name, "repo1");
        assert!(queue.take_one().is_none()); // now empty
    }

    // ── is_finished ───────────────────────────────────────────

    #[test]
    fn is_finished_true_when_empty_and_no_active() {
        let queue = CloneQueue::new(None, None);
        let active = AtomicUsize::new(0);
        assert!(queue.is_finished(&active));
    }

    #[test]
    fn is_finished_false_when_pending() {
        let queue = CloneQueue::new(None, None);
        let dir = tempfile::tempdir().unwrap();
        queue.push(make_task("repo1", &dir.path().join("repo1")));

        let active = AtomicUsize::new(0);
        assert!(!queue.is_finished(&active));
    }

    #[test]
    fn is_finished_false_when_active_workers() {
        let queue = CloneQueue::new(None, None);
        let active = AtomicUsize::new(1);
        assert!(!queue.is_finished(&active));
    }

    // ── push_from_meta ────────────────────────────────────────

    #[test]
    fn push_from_meta_discovers_projects() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {
                "alpha": "git@github.com:org/alpha.git",
                "beta": "git@github.com:org/beta.git"
            }}"#,
        )
        .unwrap();

        let queue = CloneQueue::new(None, None);
        let added = queue.push_from_meta(dir.path(), 0).unwrap();
        assert_eq!(added, 2);
        assert_eq!(queue.total_discovered.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn push_from_meta_no_config_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let queue = CloneQueue::new(None, None);
        let added = queue.push_from_meta(dir.path(), 0).unwrap();
        assert_eq!(added, 0);
    }

    #[test]
    fn push_from_meta_respects_depth_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"repo": "git@github.com:org/repo.git"}}"#,
        )
        .unwrap();

        // meta_depth = Some(0) means only depth 0 is allowed
        let queue = CloneQueue::new(None, Some(0));
        let added = queue.push_from_meta(dir.path(), 0).unwrap();
        assert_eq!(added, 1); // depth 0 is allowed

        // Trying at depth 1 should be blocked
        let added = queue.push_from_meta(dir.path(), 1).unwrap();
        assert_eq!(added, 0);
    }

    #[test]
    fn push_from_meta_skips_existing_dirs() {
        let dir = tempfile::tempdir().unwrap();
        // Create the project directory so it appears "already cloned"
        std::fs::create_dir(dir.path().join("existing")).unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"existing": "git@github.com:org/existing.git"}}"#,
        )
        .unwrap();

        let queue = CloneQueue::new(None, None);
        let added = queue.push_from_meta(dir.path(), 0).unwrap();
        assert_eq!(added, 0); // skipped because dir exists
    }

    #[test]
    fn push_from_meta_skips_projects_without_repo() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"no-repo": {"path": "no-repo"}}}"#,
        )
        .unwrap();

        let queue = CloneQueue::new(None, None);
        let added = queue.push_from_meta(dir.path(), 0).unwrap();
        assert_eq!(added, 0);
    }

    #[test]
    fn push_from_meta_preserves_is_meta_flag() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {
                "nested": {"repo": "git@github.com:org/nested.git", "meta": true},
                "plain": "git@github.com:org/plain.git"
            }}"#,
        )
        .unwrap();

        let queue = CloneQueue::new(None, None);
        queue.push_from_meta(dir.path(), 0).unwrap();

        let tasks = queue.drain_all();
        let nested = tasks.iter().find(|t| t.name == "nested").unwrap();
        let plain = tasks.iter().find(|t| t.name == "plain").unwrap();

        assert!(nested.is_meta);
        assert!(!plain.is_meta);
    }

    // ── mark_completed / nested discovery ─────────────────────

    #[test]
    fn mark_completed_discovers_nested_meta() {
        let dir = tempfile::tempdir().unwrap();
        let child_dir = dir.path().join("child");
        std::fs::create_dir(&child_dir).unwrap();

        // Child has its own .meta.yaml with a grandchild
        std::fs::write(
            child_dir.join(".meta.yaml"),
            r#"{"projects": {"grandchild": "git@github.com:org/grandchild.git"}}"#,
        )
        .unwrap();

        let queue = CloneQueue::new(None, None);
        let task = CloneTask {
            name: "child".to_string(),
            url: "git@github.com:org/child.git".to_string(),
            target_path: child_dir,
            depth_level: 0,
            is_meta: true,
        };

        let added = queue.mark_completed(&task).unwrap();
        assert_eq!(added, 1);
        assert_eq!(queue.total_discovered.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn mark_completed_no_nested_meta() {
        let dir = tempfile::tempdir().unwrap();
        let child_dir = dir.path().join("child");
        std::fs::create_dir(&child_dir).unwrap();
        // No .meta.yaml inside child

        let queue = CloneQueue::new(None, None);
        let task = CloneTask {
            name: "child".to_string(),
            url: "git@github.com:org/child.git".to_string(),
            target_path: child_dir,
            depth_level: 0,
            is_meta: false,
        };

        let added = queue.mark_completed(&task).unwrap();
        assert_eq!(added, 0);
    }

    // ── get_counts ────────────────────────────────────────────

    #[test]
    fn get_counts_tracks_discovered_and_completed() {
        let dir = tempfile::tempdir().unwrap();
        let queue = CloneQueue::new(None, None);

        queue.push(make_task("a", &dir.path().join("a")));
        queue.push(make_task("b", &dir.path().join("b")));

        let (completed, discovered) = queue.get_counts();
        assert_eq!(discovered, 2);
        assert_eq!(completed, 0);
    }

    #[test]
    fn failed_clones_are_reported_with_their_error() {
        let dir = tempfile::tempdir().unwrap();
        let queue = CloneQueue::new(None, None);
        let task = make_task("missing", &dir.path().join("missing"));
        queue.push(task.clone());

        queue.mark_failed(&task, "fatal: repository not found".to_string());

        let failures = queue.failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].name, "missing");
        assert_eq!(failures[0].url, task.url);
        assert_eq!(failures[0].error, "fatal: repository not found");
    }

    // ── drain_all ─────────────────────────────────────────────

    #[test]
    fn drain_all_empties_queue() {
        let dir = tempfile::tempdir().unwrap();
        let queue = CloneQueue::new(None, None);
        queue.push(make_task("a", &dir.path().join("a")));
        queue.push(make_task("b", &dir.path().join("b")));

        let tasks = queue.drain_all();
        assert_eq!(tasks.len(), 2);
        assert!(queue.take_one().is_none());
    }

    fn make_task_with_url(name: &str, url: &str, path: &Path) -> CloneTask {
        CloneTask {
            name: name.to_string(),
            url: url.to_string(),
            target_path: path.to_path_buf(),
            depth_level: 0,
            is_meta: false,
        }
    }

    // ── peek_urls ──────────────────────────────────────────────

    #[test]
    fn peek_urls_empty_queue() {
        let queue = CloneQueue::new(None, None);
        assert!(queue.peek_urls().is_empty());
    }

    #[test]
    fn peek_urls_returns_all_pending_urls_without_dequeue() {
        let dir = tempfile::tempdir().unwrap();
        let queue = CloneQueue::new(None, None);

        queue.push(make_task_with_url(
            "r1",
            "git@github.com:org/r1.git",
            &dir.path().join("r1"),
        ));
        queue.push(make_task_with_url(
            "r2",
            "https://github.com/org/r2.git",
            &dir.path().join("r2"),
        ));

        let urls = queue.peek_urls();
        assert_eq!(urls.len(), 2);
        assert!(urls.contains(&"git@github.com:org/r1.git".to_string()));
        assert!(urls.contains(&"https://github.com/org/r2.git".to_string()));

        // Verify tasks are still in the queue (not dequeued)
        assert!(queue.take_one().is_some());
        assert!(queue.take_one().is_some());
        assert!(queue.take_one().is_none());
    }
}
