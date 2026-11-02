use log::debug;
use meta_core::config;
use meta_core::display::escape_path;
use meta_core::git::GitUrl;
use meta_core::gitdir::Checkout;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// What a task does to its project's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Work {
    /// Clone the repository from this URL into the empty or missing folder.
    Clone(GitUrl),
    /// Fast-forward the repository there (`git pull --ff-only`).
    Pull,
}

/// A meta repo on the way down to a task: its name, its folder (resolved),
/// and the URL it comes from, if known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaRepo {
    pub name: String,
    pub dir: PathBuf,
    pub url: Option<GitUrl>,
}

impl MetaRepo {
    /// The meta repo `name` at `dir`, from its checkout's origin if it has
    /// one, else from `url` (what .meta.yaml lists, or the URL given).
    pub fn at(name: &str, dir: &Path, url: Option<GitUrl>) -> Self {
        Self {
            name: name.to_string(),
            dir: resolved(dir),
            url: origin(dir).or(url),
        }
    }
}

/// One repository to clone or pull.
#[derive(Debug, Clone)]
pub struct CloneTask {
    /// Display name for progress output
    pub name: String,
    pub work: Work,
    /// The project's folder
    pub target_path: PathBuf,
    /// The URL `.meta.yaml` gives for it, if any
    pub url: Option<GitUrl>,
    /// The level of the `.meta.yaml` inside it (the root's is 0)
    pub level: usize,
    /// Whether this project is itself a meta-repo (declared with `meta: true` in config)
    pub is_meta: bool,
    /// The meta repos above it, the top one first
    pub above: Arc<[MetaRepo]>,
}

/// A clone or pull that failed, with git's own error output.
#[derive(Debug, Clone)]
pub struct CloneFailure {
    pub name: String,
    pub work: Work,
    pub error: String,
}

/// What became of the repositories a clone or update found.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// Repositories pulled and cloned
    pub pulled: usize,
    pub cloned: usize,
    /// Failed clones and pulls, in the order they failed
    pub failures: Vec<CloneFailure>,
    /// Repositories not pulled because there is nothing to pull from: the
    /// name, and why
    pub not_pulled: Vec<(String, String)>,
    /// Projects not cloned because their folder holds something that is no
    /// repository of its own: the name, and why
    pub refused: Vec<(String, String)>,
    /// Projects not entered because they repeat a meta repo above them:
    /// the chain of names, the top one first
    pub cycles: Vec<Vec<String>>,
    /// Nested `.meta.yaml` files that can't be read or used, each with its repo
    pub unreadable: Vec<String>,
    /// Repositories in a meta repo's folder that its `.meta.yaml` doesn't
    /// list (found only when pulling)
    pub unlisted: Vec<PathBuf>,
}

/// The repositories a clone or update finds, and what became of them.
///
/// Reading a meta repo's `.meta.yaml` hands back the tasks for its
/// projects; each folder is handed out once.
pub struct CloneQueue {
    /// Folders a task was handed out for
    claimed: Mutex<HashSet<PathBuf>>,
    outcome: Mutex<Outcome>,
    /// Tasks handed out (for progress display)
    total_discovered: AtomicUsize,
    /// Tasks finished, failed ones included (for progress display)
    total_completed: AtomicUsize,
    /// Git depth argument (if any)
    git_depth: Option<String>,
    /// Max meta depth for recursion (None = unlimited)
    meta_depth: Option<usize>,
    /// Whether projects already cloned are pulled
    pull: bool,
}

impl CloneQueue {
    /// A queue that clones with `git_depth`, reads `.meta.yaml` files down
    /// to level `meta_depth` (all of them with `None`), and pulls the
    /// projects already cloned when `pull`.
    pub fn new(git_depth: Option<String>, meta_depth: Option<usize>, pull: bool) -> Self {
        Self {
            claimed: Mutex::new(HashSet::new()),
            outcome: Mutex::new(Outcome::default()),
            total_discovered: AtomicUsize::new(0),
            total_completed: AtomicUsize::new(0),
            git_depth,
            meta_depth,
            pull,
        }
    }

    /// The task for the meta repo at `dir` itself, which is `name` and comes
    /// from `url`: its projects are found once it is done.
    pub fn root(&self, name: &str, dir: &Path, url: Option<GitUrl>, work: Work) -> CloneTask {
        let task = CloneTask {
            name: name.to_string(),
            work,
            target_path: dir.to_path_buf(),
            url,
            level: 0,
            is_meta: true,
            above: Arc::from([]),
        };
        self.claim(&task.target_path);
        task
    }

    /// Change the outcome.
    fn record(&self, change: impl FnOnce(&mut Outcome)) {
        change(&mut self.outcome.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// What became of the repositories so far.
    pub fn outcome(&self) -> Outcome {
        self.outcome
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Hand out the task for the folder at `path`: false if one was handed
    /// out for it before, however spelled.
    fn claim(&self, path: &Path) -> bool {
        let fresh = self
            .claimed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(resolved(path));
        if fresh {
            self.total_discovered.fetch_add(1, Ordering::SeqCst);
        }
        fresh
    }

    /// The tasks for the projects of the `.meta.yaml` in `dir`, at `level`,
    /// below the meta repos `above` (the one at `dir` last).
    pub fn discover(
        &self,
        dir: &Path,
        level: usize,
        above: &Arc<[MetaRepo]>,
    ) -> anyhow::Result<Vec<CloneTask>> {
        if self.meta_depth.is_some_and(|max| level > max) {
            debug!("Skipping nested discovery at depth {level}");
            return Ok(Vec::new());
        }
        match config::Workspace::in_dir(dir)? {
            Some(workspace) => self.discover_in(&workspace, level, above),
            None => {
                debug!("No .meta.yaml found in {}", escape_path(dir));
                Ok(Vec::new())
            }
        }
    }

    /// [`Self::discover`], for a workspace already loaded.
    pub fn discover_in(
        &self,
        workspace: &config::Workspace,
        level: usize,
        above: &Arc<[MetaRepo]>,
    ) -> anyhow::Result<Vec<CloneTask>> {
        let base_dir = workspace.dir.as_path();
        if self.pull {
            self.note_unlisted(workspace);
        }
        let mut tasks = Vec::new();
        for project in &workspace.projects {
            let target_path = base_dir.join(&project.path);

            let work = match Checkout::of(&target_path) {
                Checkout::Repo { .. } if self.pull => Work::Pull,
                // Cloned: still look in it for nested discovery
                Checkout::Repo { .. } => {
                    if self.claim(&target_path) {
                        self.total_completed.fetch_add(1, Ordering::SeqCst);
                        let this = MetaRepo::at(&project.name, &target_path, project.repo.clone());
                        tasks.extend(self.enter(this, &target_path, level + 1, above)?);
                    }
                    continue;
                }
                // git clones into an empty folder, and refuses any other
                Checkout::NotARepo(why) if !is_empty_dir(&target_path) => {
                    self.record(|o| o.refused.push((project.name.clone(), why)));
                    continue;
                }
                Checkout::Missing | Checkout::NotARepo(_) => match &project.repo {
                    Some(url) => Work::Clone(url.clone()),
                    // Skip projects without a repo URL (cannot clone)
                    None => continue,
                },
            };
            // A declared meta repo that is one above it would be cloned
            // inside itself again and again: refused before it is cloned.
            // Any other repeat is caught before its .meta.yaml is read.
            if let Work::Clone(url) = &work {
                let candidate = MetaRepo {
                    name: project.name.clone(),
                    dir: resolved(&target_path),
                    url: Some(url.clone()),
                };
                if project.meta && self.reads(level + 1) && self.repeats(&candidate, above) {
                    continue;
                }
            }
            if self.claim(&target_path) {
                debug!("Queued {work:?} for {} at depth {level}", project.name);
                tasks.push(CloneTask {
                    name: project.name.clone(),
                    work,
                    target_path,
                    url: project.repo.clone(),
                    level: level + 1,
                    is_meta: project.meta,
                    above: Arc::clone(above),
                });
            }
        }
        Ok(tasks)
    }

    /// Record the repositories in `workspace`'s folder that it doesn't list.
    fn note_unlisted(&self, workspace: &config::Workspace) {
        let listed: HashSet<&str> = workspace.projects.iter().map(|p| p.path.as_str()).collect();
        let entries = std::fs::read_dir(&workspace.dir)
            .into_iter()
            .flatten()
            .flatten();
        let mut found: Vec<PathBuf> = entries
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|name| !name.starts_with('.') && !listed.contains(name))
                    && matches!(Checkout::of(path), Checkout::Repo { .. })
            })
            .collect();
        found.sort();
        self.record(|o| o.unlisted.extend(found));
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

    /// Mark `task` done, and hand back the tasks for the projects of the
    /// `.meta.yaml` in it. One that can't be read is recorded.
    pub fn mark_completed(&self, task: &CloneTask) -> Vec<CloneTask> {
        self.total_completed.fetch_add(1, Ordering::SeqCst);
        self.record(|o| match task.work {
            Work::Pull => o.pulled += 1,
            Work::Clone(_) => o.cloned += 1,
        });
        let found = self.nested(task);

        // Warn if the config declared meta: true but no nested .meta.yaml was found
        if task.is_meta && found.is_empty() && task.level > 0 && self.reads(task.level) {
            if let Ok(None) = config::find_meta_config_in(&task.target_path) {
                meta_core::out::warn(format!(
                    "'{}' is declared with `meta: true` but no .meta.yaml was found inside it",
                    meta_core::display::escape(&task.name)
                ));
            }
        }
        found
    }

    /// The tasks for the projects of the `.meta.yaml` in `task`'s folder; a
    /// config that can't be read is recorded.
    fn nested(&self, task: &CloneTask) -> Vec<CloneTask> {
        let this = MetaRepo::at(&task.name, &task.target_path, task.url.clone());
        let found = self
            .enter(this, &task.target_path, task.level, &task.above)
            .unwrap_or_else(|e| {
                self.record(|o| o.unreadable.push(format!("{}: {e:#}", task.name)));
                Vec::new()
            });
        debug!("{}: {} nested tasks discovered", task.name, found.len());
        found
    }

    /// The tasks for the projects of the `.meta.yaml` of `this`, the meta
    /// repo at `dir`, at `level` below `above`; none if `this` has one and
    /// is one of `above`, which is recorded as a cycle.
    fn enter(
        &self,
        this: MetaRepo,
        dir: &Path,
        level: usize,
        above: &[MetaRepo],
    ) -> anyhow::Result<Vec<CloneTask>> {
        if self.reads(level)
            && config::find_meta_config_in(dir)?.is_some()
            && self.repeats(&this, above)
        {
            return Ok(Vec::new());
        }
        self.discover(dir, level, &below(above, this))
    }

    /// Whether `this` is one of the meta repos `above`, by folder or URL;
    /// if so, the chain from it is recorded as a cycle.
    fn repeats(&self, this: &MetaRepo, above: &[MetaRepo]) -> bool {
        let first = above.iter().position(|meta| {
            meta.dir == this.dir
                || meta
                    .url
                    .as_ref()
                    .zip(this.url.as_ref())
                    .is_some_and(same_repository)
        });
        if let Some(first) = first {
            let chain = above.iter().skip(first).map(|m| m.name.clone());
            let chain = chain.chain([this.name.clone()]).collect();
            self.record(|o| o.cycles.push(chain));
        }
        first.is_some()
    }

    /// Whether a `.meta.yaml` at `level` is read.
    fn reads(&self, level: usize) -> bool {
        self.meta_depth.is_none_or(|max| level <= max)
    }

    /// Mark a pull as not done because the repository has nothing to pull
    /// from (`why`): the tasks for the projects of its `.meta.yaml`.
    pub fn mark_not_pulled(&self, task: &CloneTask, why: String) -> Vec<CloneTask> {
        self.total_completed.fetch_add(1, Ordering::SeqCst);
        self.record(|o| o.not_pulled.push((task.name.clone(), why)));
        self.nested(task)
    }

    /// Mark a task as failed, keeping git's error output for the final
    /// report. A repository that couldn't be pulled is there all the same:
    /// the tasks for the projects of its `.meta.yaml` as it is.
    pub fn mark_failed(&self, task: &CloneTask, error: String) -> Vec<CloneTask> {
        self.total_completed.fetch_add(1, Ordering::SeqCst);
        self.record(|o| {
            o.failures.push(CloneFailure {
                name: task.name.clone(),
                work: task.work.clone(),
                error,
            })
        });
        match task.work {
            Work::Pull => self.nested(task),
            Work::Clone(_) => Vec::new(),
        }
    }
}

/// `above`, and `this` below it.
fn below(above: &[MetaRepo], this: MetaRepo) -> Arc<[MetaRepo]> {
    above.iter().cloned().chain([this]).collect()
}

/// The origin URL of the repository at `dir`, if it is one and has one.
fn origin(dir: &Path) -> Option<GitUrl> {
    match Checkout::of(dir) {
        Checkout::Repo { .. } => meta_core::git::remote_url(dir).ok().flatten(),
        _ => None,
    }
}

/// Whether two URLs name the same repository, local ones by the folder they
/// lead to.
fn same_repository((a, b): (&GitUrl, &GitUrl)) -> bool {
    let local = |url: &GitUrl| {
        let text = url.as_git_arg().to_str()?;
        let text = text.strip_prefix("file://").unwrap_or(text);
        // file:///C:/x names C:/x on Windows
        #[cfg(windows)]
        let text = match text.strip_prefix('/') {
            Some(rest) if rest.get(1..2) == Some(":") => rest,
            _ => text,
        };
        let path = Path::new(text);
        path.is_absolute()
            .then(|| path.canonicalize().ok())
            .flatten()
    };
    a.same_repository(b) || local(a).is_some_and(|dir| local(b) == Some(dir))
}

/// `path` with links and `..` resolved as far as it exists.
fn resolved(path: &Path) -> PathBuf {
    if let Ok(real) = path.canonicalize() {
        return real;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => resolved(parent).join(name),
        _ => path.to_path_buf(),
    }
}

/// Whether `dir` is a folder with nothing in it (git clones into one).
pub fn is_empty_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(dir: &Path, url: Option<&str>) -> Arc<[MetaRepo]> {
        Arc::from([MetaRepo::at("ws", dir, url.map(GitUrl::from))])
    }

    fn names(tasks: &[CloneTask]) -> Vec<&str> {
        let mut names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
        names.sort();
        names
    }

    fn write_meta(dir: &Path, json: &str) {
        std::fs::write(dir.join(".meta.yaml"), json).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn discover_does_not_look_past_the_depth_limit() {
        // child exists with a dangling .meta.yaml; at depth limit 0 it is never
        // read, so the missing sibling is still found
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("child")).unwrap();
        meta_core::git::run(&dir.path().join("child"), ["init", "-q"]).unwrap();
        write_meta(
            dir.path(),
            r#"{"projects": {"child": {"repo": "u", "meta": true}, "sib": "u2"}}"#,
        );
        std::os::unix::fs::symlink("missing.yaml", dir.path().join("child/.meta.yaml")).unwrap();
        let queue = CloneQueue::new(None, Some(0), false);
        let tasks = queue
            .discover(dir.path(), 0, &top(dir.path(), None))
            .unwrap();
        assert_eq!(names(&tasks), ["sib"]);
    }

    #[cfg(unix)]
    #[test]
    fn discover_fails_on_a_nested_config_it_cannot_read() {
        // child/.meta.yaml is a link to nowhere: its repos are unknown, which is
        // an error, not "no nested repos"
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("child")).unwrap();
        meta_core::git::run(&dir.path().join("child"), ["init", "-q"]).unwrap();
        write_meta(
            dir.path(),
            r#"{"projects": {"child": {"repo": "u", "meta": true}}}"#,
        );
        std::os::unix::fs::symlink("missing.yaml", dir.path().join("child/.meta.yaml")).unwrap();
        let queue = CloneQueue::new(None, None, false);
        let err = queue
            .discover(dir.path(), 0, &top(dir.path(), None))
            .unwrap_err();
        assert!(format!("{err:#}").contains("Cannot read"), "{err:#}");
    }

    #[test]
    fn discover_clones_only_where_no_repository_is_and_pulls_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        // a clone, an empty folder git can clone into, and a plain folder
        for name in ["cloned", "empty", "plain"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        meta_core::git::run(&dir.path().join("cloned"), ["init", "-q"]).unwrap();
        std::fs::write(dir.path().join("plain/notes.txt"), "mine").unwrap();
        write_meta(
            dir.path(),
            r#"{"projects": {"cloned": "u1", "empty": "u2", "plain": "u3", "missing": "u4",
                "no-repo": {"path": "no-repo"}}}"#,
        );

        let queue = CloneQueue::new(None, None, true);
        let tasks = queue
            .discover(dir.path(), 0, &top(dir.path(), None))
            .unwrap();
        let work = |name: &str| {
            tasks
                .iter()
                .find(|t| t.name == name)
                .map(|t| t.work.clone())
        };
        assert_eq!(names(&tasks), ["cloned", "empty", "missing"]);
        assert_eq!(work("cloned"), Some(Work::Pull));
        assert_eq!(work("empty"), Some(Work::Clone("u2".into())));
        // the plain folder is named, never cloned over nor taken for a clone
        assert_eq!(
            queue.outcome().refused,
            [("plain".to_string(), "folder exists, no .git".to_string())]
        );
        // a folder is handed out once
        assert!(queue
            .discover(dir.path(), 0, &top(dir.path(), None))
            .unwrap()
            .is_empty());
        assert_eq!(queue.get_counts(), (0, 3));
    }

    #[test]
    fn a_nested_meta_repo_is_read_once_its_task_is_done() {
        let dir = tempfile::tempdir().unwrap();
        write_meta(
            dir.path(),
            r#"{"projects": {"child": {"repo": "u", "meta": true}}}"#,
        );

        let queue = CloneQueue::new(None, None, false);
        let tasks = queue
            .discover(dir.path(), 0, &top(dir.path(), None))
            .unwrap();
        assert_eq!(names(&tasks), ["child"]);
        // as the clone leaves it
        let child = dir.path().join("child");
        std::fs::create_dir(&child).unwrap();
        write_meta(&child, r#"{"projects": {"grandchild": "u2"}}"#);
        assert!(tasks[0].is_meta);
        let nested = queue.mark_completed(&tasks[0]);
        assert_eq!(names(&nested), ["grandchild"]);
        assert_eq!((queue.outcome().pulled, queue.outcome().cloned), (0, 1));

        // with depth limit 0, the clone's .meta.yaml isn't read
        let queue = CloneQueue::new(None, Some(0), false);
        assert!(queue.mark_completed(&tasks[0]).is_empty());
    }

    #[test]
    fn a_meta_repo_that_lists_one_above_it_is_refused_by_name() {
        // ws lists kid, whose .meta.yaml lists ws's own URL
        let dir = tempfile::tempdir().unwrap();
        write_meta(
            dir.path(),
            r#"{"projects": {"kid": {"repo": "kid.git", "meta": true}}}"#,
        );
        let queue = CloneQueue::new(None, None, false);
        let tasks = queue
            .discover(dir.path(), 0, &top(dir.path(), Some("top.git")))
            .unwrap();
        std::fs::create_dir(dir.path().join("kid")).unwrap();
        write_meta(
            &dir.path().join("kid"),
            r#"{"projects": {"top": {"repo": "top.git", "meta": true}, "lib": "lib.git"}}"#,
        );
        let nested = queue.mark_completed(&tasks[0]);
        assert_eq!(names(&nested), ["lib"]);
        assert_eq!(queue.outcome().cycles, [vec!["ws", "kid", "top"]]);
    }

    #[test]
    fn two_meta_repos_listing_one_repo_is_no_cycle_and_local_urls_match_by_folder() {
        // b and c both list d; d lists the workspace by another spelling
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let remote = ws.join("remote.git");
        std::fs::create_dir(&remote).unwrap();
        write_meta(ws, r#"{"projects": {"b": "b.git", "c": "c.git"}}"#);
        let queue = CloneQueue::new(None, None, false);
        let above = top(ws, Some(&remote.to_string_lossy()));
        let tasks = queue.discover(ws, 0, &above).unwrap();
        for task in &tasks {
            std::fs::create_dir(&task.target_path).unwrap();
            write_meta(&task.target_path, r#"{"projects": {"d": "d.git"}}"#);
        }
        let mut nested: Vec<CloneTask> =
            tasks.iter().flat_map(|t| queue.mark_completed(t)).collect();
        assert_eq!(names(&nested), ["d", "d"]);
        assert!(queue.outcome().cycles.is_empty());
        let d = nested.remove(0);
        std::fs::create_dir(&d.target_path).unwrap();
        // single-quoted: a Windows path's backslashes stay as they are
        let back = format!(
            r#"{{"projects": {{"ws": {{"repo": 'file://{}/', "meta": true}}}}}}"#,
            remote.to_string_lossy()
        );
        write_meta(&d.target_path, &back);
        assert!(queue.mark_completed(&d).is_empty());
        assert_eq!(queue.outcome().cycles.len(), 1);
    }

    #[test]
    fn without_recursion_a_repeated_repo_is_no_cycle() {
        // a second checkout of the workspace's own repo: its .meta.yaml is
        // never read, so it is cloned like any project
        let dir = tempfile::tempdir().unwrap();
        write_meta(
            dir.path(),
            r#"{"projects": {"pages": {"repo": "top.git", "meta": true}}}"#,
        );
        let queue = CloneQueue::new(None, Some(0), true);
        let tasks = queue
            .discover(dir.path(), 0, &top(dir.path(), Some("top.git")))
            .unwrap();
        assert_eq!(names(&tasks), ["pages"]);
        assert!(queue.outcome().cycles.is_empty());
    }

    #[test]
    fn an_undeclared_meta_repo_that_repeats_one_above_it_is_cloned_once_and_not_entered() {
        // ws lists kid, kid lists top (ws's own repo); neither says meta: true
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        write_meta(ws, r#"{"projects": {"kid": "kid.git"}}"#);
        let queue = CloneQueue::new(None, None, false);
        let tasks = queue.discover(ws, 0, &top(ws, Some("top.git"))).unwrap();
        assert_eq!(names(&tasks), ["kid"]);
        // as the clones leave them
        std::fs::create_dir(&tasks[0].target_path).unwrap();
        write_meta(&tasks[0].target_path, r#"{"projects": {"top": "top.git"}}"#);
        let nested = queue.mark_completed(&tasks[0]);
        assert_eq!(names(&nested), ["top"]);
        assert!(queue.outcome().cycles.is_empty());
        std::fs::create_dir(&nested[0].target_path).unwrap();
        write_meta(
            &nested[0].target_path,
            r#"{"projects": {"kid": "kid.git"}}"#,
        );
        assert!(queue.mark_completed(&nested[0]).is_empty());
        assert_eq!(queue.outcome().cycles, [vec!["ws", "kid", "top"]]);
    }

    #[cfg(windows)]
    #[test]
    fn a_file_url_with_a_drive_is_the_local_folder() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_string_lossy().into_owned();
        let url = GitUrl::from(format!("file:///{}", path.replace('\\', "/")));
        assert!(same_repository((&GitUrl::from(path), &url)));
    }

    #[cfg(unix)]
    #[test]
    fn a_project_whose_folder_leads_back_up_is_a_cycle() {
        // ws/kid leads out of the workspace, to a meta repo whose `back`
        // leads to ws again
        let dir = tempfile::tempdir().unwrap();
        let (ws, kid) = (dir.path().join("ws"), dir.path().join("kid"));
        for repo in [&ws, &kid] {
            std::fs::create_dir(repo).unwrap();
            meta_core::git::run(repo, ["init", "-q"]).unwrap();
        }
        std::os::unix::fs::symlink(&kid, ws.join("kid")).unwrap();
        std::os::unix::fs::symlink(&ws, kid.join("back")).unwrap();
        write_meta(&ws, r#"{"projects": {"kid": {"repo": "u", "meta": true}}}"#);
        write_meta(
            &kid,
            r#"{"projects": {"back": {"repo": "u2", "meta": true}, "lib": "u3"}}"#,
        );
        let queue = CloneQueue::new(None, None, false);
        let tasks = queue.discover(&ws, 0, &top(&ws, None)).unwrap();
        assert_eq!(names(&tasks), ["lib"]);
        assert_eq!(queue.outcome().cycles, [vec!["ws", "kid", "back"]]);
    }

    #[test]
    fn repositories_the_config_does_not_list_are_recorded() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["listed", "other", "plain"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        meta_core::git::run(&dir.path().join("listed"), ["init", "-q"]).unwrap();
        meta_core::git::run(&dir.path().join("other"), ["init", "-q"]).unwrap();
        write_meta(dir.path(), r#"{"projects": {"listed": "u"}}"#);
        let queue = CloneQueue::new(None, None, true);
        queue
            .discover(dir.path(), 0, &top(dir.path(), None))
            .unwrap();
        assert_eq!(queue.outcome().unlisted, [dir.path().join("other")]);
    }

    #[test]
    fn failed_tasks_are_reported_with_their_error() {
        let dir = tempfile::tempdir().unwrap();
        write_meta(dir.path(), r#"{"projects": {"missing": "u"}}"#);
        let queue = CloneQueue::new(None, None, true);
        let tasks = queue
            .discover(dir.path(), 0, &top(dir.path(), None))
            .unwrap();
        assert!(queue
            .mark_failed(&tasks[0], "fatal: repository not found".to_string())
            .is_empty());

        let failures = queue.outcome().failures;
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].name, "missing");
        assert_eq!(failures[0].work, Work::Clone("u".into()));
        assert_eq!(failures[0].error, "fatal: repository not found");
        assert_eq!(queue.get_counts(), (1, 1));
    }
}
