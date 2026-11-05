use crate::here::Listing;
use log::debug;
use meta_core::gitdir::Checkout;
use meta_plugin::RepoUrl;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// What a task does to its project's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Work {
    /// Clone the repository from this URL into the empty or missing folder.
    Clone(RepoUrl),
    /// Fast-forward the repository there (`git pull --ff-only`).
    Pull,
}

/// A meta repo on the way down to a task: its name, its folder (resolved),
/// and the URL it comes from, if known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaRepo {
    pub name: String,
    pub dir: PathBuf,
    pub url: Option<RepoUrl>,
}

impl MetaRepo {
    /// The meta repo `name` at `dir`, from its checkout's origin if it has
    /// one, else from `url` (what .meta.yaml lists, or the URL given).
    pub fn at(name: &str, dir: &Path, url: Option<RepoUrl>) -> Self {
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
    pub url: Option<RepoUrl>,
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
    /// The project's folder
    pub path: PathBuf,
    pub work: Work,
    pub error: String,
}

/// What became of the repositories a clone or update found.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// How many repositories were pulled, and the folders cloned into
    pub pulled: usize,
    pub cloned: Vec<PathBuf>,
    /// Failed clones and pulls, in the order they failed
    pub failures: Vec<CloneFailure>,
    /// Repositories not pulled because there is nothing to pull from: the
    /// name, and why
    pub not_pulled: Vec<(String, String)>,
    /// Projects not cloned because their folder holds something that is no
    /// repository of its own: the name, the folder, and why
    pub refused: Vec<(String, PathBuf, String)>,
    /// Projects not entered because they repeat a meta repo above them:
    /// the chain of names, the top one first
    pub cycles: Vec<Vec<String>>,
    /// Nested `.meta.yaml` files that can't be read or used, each with its repo
    pub unreadable: Vec<String>,
    /// Repositories in a meta repo's folder that no `.meta.yaml` read lists,
    /// and that hold no project one lists
    pub unlisted: Vec<PathBuf>,
    /// Projects not cloned because their URL holds credentials and was read
    /// back from meta, which hides them: the name, the folder, and whether
    /// the config listing it changed in this run (another run gets the URL
    /// as configured, in its request)
    pub deferred: Vec<(String, PathBuf, bool)>,
}

/// The repositories a clone or update finds, and what became of them.
///
/// Reading a meta repo's `.meta.yaml` hands back the tasks for its
/// projects; each folder is handed out once.
pub struct CloneQueue {
    /// Folders a task was handed out for, or that were entered as cloned
    claimed: Mutex<HashSet<PathBuf>>,
    /// The folder of every project a `.meta.yaml` read lists (resolved)
    listed: Mutex<Vec<PathBuf>>,
    outcome: Mutex<Outcome>,
    /// Tasks handed out (for progress display)
    total_discovered: AtomicUsize,
    /// How many tasks the run is expected to hand out (for progress display)
    expected: AtomicUsize,
    /// Tasks finished, failed ones included (for progress display)
    total_completed: AtomicUsize,
    /// Git depth argument (if any)
    git_depth: Option<String>,
    /// Max meta depth for recursion (None = unlimited)
    meta_depth: Option<usize>,
    /// Whether projects already cloned are pulled
    pull: bool,
    /// The folders a selection chose: only their projects are cloned, and
    /// only their meta repos are searched for unlisted repositories
    only: Option<HashSet<PathBuf>>,
    /// The config meta's `--config` named, and the folder it lists for
    config: Option<(PathBuf, PathBuf)>,
    /// Each project's URL as configured, by its folder (resolved): from
    /// the request, for a URL whose credentials meta hides when it lists it
    raw: HashMap<PathBuf, RepoUrl>,
    /// The folders cloned or pulled in this run: their configs may have changed
    changed: Mutex<HashSet<PathBuf>>,
}

impl CloneQueue {
    /// A queue that clones with `git_depth`, reads `.meta.yaml` files down
    /// to level `meta_depth` (all of them with `None`), and pulls the
    /// projects already cloned when `pull`.
    pub fn new(git_depth: Option<String>, meta_depth: Option<usize>, pull: bool) -> Self {
        Self {
            claimed: Mutex::new(HashSet::new()),
            listed: Mutex::new(Vec::new()),
            outcome: Mutex::new(Outcome::default()),
            total_discovered: AtomicUsize::new(0),
            expected: AtomicUsize::new(0),
            total_completed: AtomicUsize::new(0),
            git_depth,
            meta_depth,
            pull,
            only: None,
            config: None,
            raw: HashMap::new(),
            changed: Mutex::new(HashSet::new()),
        }
    }

    /// The queue, cloning each of `projects` (meta's members of the
    /// workspace) from its URL as configured.
    pub fn raw(mut self, projects: &[meta_plugin::Project]) -> Self {
        self.raw = projects
            .iter()
            .filter_map(|p| Some((resolved(&p.abs), RepoUrl::from(p.repo.clone()?))))
            .collect();
        self
    }

    /// The queue, reading the projects of the folder `dir` from `config`
    /// (meta's `--config`) instead of its default config.
    pub fn config(mut self, dir: &Path, config: &Path) -> Self {
        self.config = Some((resolved(dir), config.to_path_buf()));
        self
    }

    /// What the config in `dir` lists (`--config`'s for its folder), as
    /// meta reads it; `None` without one.
    fn listing(&self, dir: &Path) -> anyhow::Result<Option<Listing>> {
        let config = self
            .config
            .as_ref()
            .filter(|(at, _)| *at == resolved(dir))
            .map(|(_, config)| config.as_path());
        Listing::of(dir, config)
    }

    /// The queue, keeping to the folders `dirs` (a selection's).
    pub fn only(mut self, dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        self.only = Some(dirs.into_iter().map(|dir| resolved(&dir)).collect());
        self
    }

    /// Whether the folder at `dir` is one the queue keeps to.
    fn selected(&self, dir: &Path) -> bool {
        self.only
            .as_ref()
            .is_none_or(|only| only.contains(&resolved(dir)))
    }

    /// Show at least `tasks` as the total in progress, until more are found.
    pub fn expect(&self, tasks: usize) {
        self.expected.store(tasks, Ordering::SeqCst);
    }

    /// How many folders were handed out or entered: the repositories a run
    /// would clone or pull, as far as discovery went.
    pub fn found(&self) -> usize {
        self.claimed.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// The task for the meta repo at `dir` itself, which is `name` and comes
    /// from `url`: its projects are found once it is done.
    pub fn root(&self, name: &str, dir: &Path, url: Option<RepoUrl>, work: Work) -> CloneTask {
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
        self.total_discovered.fetch_add(1, Ordering::SeqCst);
        task
    }

    /// Change the outcome.
    fn record(&self, change: impl FnOnce(&mut Outcome)) {
        change(&mut self.outcome.lock().unwrap_or_else(|e| e.into_inner()));
    }

    /// What became of the repositories so far.
    pub fn outcome(&self) -> Outcome {
        let mut outcome = self
            .outcome
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        // A repository another .meta.yaml lists, or that holds a listed
        // project, is no leftover
        let listed = self.listed.lock().unwrap_or_else(|e| e.into_inner());
        outcome
            .unlisted
            .retain(|path| !listed.iter().any(|dir| dir.starts_with(resolved(path))));
        outcome
    }

    /// Claim the folder at `path`: false if it was claimed before, however
    /// spelled.
    fn claim(&self, path: &Path) -> bool {
        self.claimed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(resolved(path))
    }

    /// The tasks for the projects of the `.meta.yaml` in `dir`, at `level`,
    /// below the meta repos `above` (the one at `dir` last).
    #[cfg(test)]
    pub fn discover(
        &self,
        dir: &Path,
        level: usize,
        above: &Arc<[MetaRepo]>,
    ) -> anyhow::Result<Vec<CloneTask>> {
        if !self.reads(level) {
            debug!("Skipping nested discovery at depth {level}");
            return Ok(Vec::new());
        }
        match self.listing(dir)? {
            Some(listing) => self.discover_in(&listing, level, above),
            None => {
                debug!(
                    "No .meta.yaml found in {}",
                    meta_plugin::display::escape_path(dir)
                );
                Ok(Vec::new())
            }
        }
    }

    /// [`Self::discover`], for a workspace already loaded.
    pub fn discover_in(
        &self,
        workspace: &Listing,
        level: usize,
        above: &Arc<[MetaRepo]>,
    ) -> anyhow::Result<Vec<CloneTask>> {
        let base_dir = workspace.dir.as_path();
        self.note_unlisted(workspace);
        let mut tasks = Vec::new();
        for project in &workspace.projects {
            let target_path = base_dir.join(&project.path);
            // a URL meta read back hides its credentials: the one configured
            // is in the request, or in the request of another run
            let mut project = project.clone();
            if workspace.asked && project.repo.as_ref().is_some_and(RepoUrl::has_credentials) {
                match self.raw.get(&resolved(&target_path)) {
                    Some(url) => project.repo = Some(url.clone()),
                    None => {
                        let absent = matches!(Checkout::of(&target_path), Checkout::Missing)
                            || is_empty_dir(&target_path);
                        if absent && self.selected(&target_path) {
                            let new = self
                                .changed
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .contains(&resolved(base_dir));
                            self.record(|o| {
                                o.deferred
                                    .push((project.name.clone(), target_path.clone(), new))
                            });
                            continue;
                        }
                        // cloned already: only its folder counts
                        project.repo = None;
                    }
                }
            }
            let project = &project;
            self.listed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(resolved(&target_path));

            let work = match Checkout::of(&target_path) {
                Checkout::Repo { .. } if self.pull => Work::Pull,
                // Cloned: still look in it for nested discovery
                Checkout::Repo { .. } => {
                    if self.claim(&target_path) {
                        let this = MetaRepo::at(&project.name, &target_path, project.repo.clone());
                        tasks.extend(self.enter(this, &target_path, level + 1, above)?.0);
                    }
                    continue;
                }
                // Outside a selection, nothing is cloned
                Checkout::Missing | Checkout::NotARepo(_) if !self.selected(&target_path) => {
                    continue
                }
                // git clones into an empty folder, and refuses any other
                Checkout::NotARepo(why) if !is_empty_dir(&target_path) => {
                    self.record(|o| {
                        o.refused
                            .push((project.name.clone(), target_path.clone(), why))
                    });
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
                self.total_discovered.fetch_add(1, Ordering::SeqCst);
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

    /// Record the repositories in `workspace`'s folder that it doesn't list
    /// (outside a selection's meta repos, none). A repo the folder's own
    /// .meta.yaml lists counts as listed too, so `--config` with a smaller
    /// list never makes a listed project unlisted; when that .meta.yaml
    /// can't be read, none is recorded.
    fn note_unlisted(&self, workspace: &Listing) {
        if !self.selected(&workspace.dir) {
            return;
        }
        let named = self
            .config
            .as_ref()
            .is_some_and(|(at, _)| *at == resolved(&workspace.dir));
        let default = match named {
            true => match Listing::of(&workspace.dir, None) {
                Ok(default) => default,
                Err(_) => return,
            },
            false => None,
        };
        let listed: HashSet<&str> = workspace
            .projects
            .iter()
            .chain(default.iter().flat_map(|d| &d.projects))
            .map(|p| p.path.as_str())
            .collect();
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

    /// Tasks finished, and tasks handed out (or expected, if more)
    pub fn get_counts(&self) -> (usize, usize) {
        (
            self.total_completed.load(Ordering::SeqCst),
            self.total_discovered
                .load(Ordering::SeqCst)
                .max(self.expected.load(Ordering::SeqCst)),
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
            Work::Clone(_) => o.cloned.push(task.target_path.clone()),
        });
        self.changed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(resolved(&task.target_path));
        let (found, listed) = self.read_nested(task);

        // Warn if the config declared meta: true but no nested .meta.yaml was found
        if task.is_meta && task.level > 0 && self.reads(task.level) && !listed {
            meta_plugin::out::warn(format!(
                "'{}' is declared with `meta: true` but no .meta.yaml was found inside it",
                meta_plugin::display::escape(&task.name)
            ));
        }
        found
    }

    /// The tasks for the projects of the `.meta.yaml` in `task`'s folder; a
    /// config that can't be read is recorded.
    fn nested(&self, task: &CloneTask) -> Vec<CloneTask> {
        self.read_nested(task).0
    }

    /// [`Self::nested`], and whether the folder has a config (one that
    /// can't be read has one).
    fn read_nested(&self, task: &CloneTask) -> (Vec<CloneTask>, bool) {
        let this = MetaRepo::at(&task.name, &task.target_path, task.url.clone());
        let found = self
            .enter(this, &task.target_path, task.level, &task.above)
            .unwrap_or_else(|e| {
                self.record(|o| o.unreadable.push(format!("{}: {e:#}", task.name)));
                (Vec::new(), true)
            });
        debug!("{}: {} nested tasks discovered", task.name, found.0.len());
        found
    }

    /// The tasks for the projects of the `.meta.yaml` of `this`, the meta
    /// repo at `dir`, at `level` below `above`; none if `this` has one and
    /// is one of `above`, which is recorded as a cycle. And whether it has
    /// one (as far as it was read: not past the depth).
    fn enter(
        &self,
        this: MetaRepo,
        dir: &Path,
        level: usize,
        above: &[MetaRepo],
    ) -> anyhow::Result<(Vec<CloneTask>, bool)> {
        if !self.reads(level) {
            return Ok((Vec::new(), true));
        }
        let Some(listing) = self.listing(dir)? else {
            return Ok((Vec::new(), false));
        };
        if self.repeats(&this, above) {
            return Ok((Vec::new(), true));
        }
        Ok((
            self.discover_in(&listing, level, &below(above, this))?,
            true,
        ))
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
                path: task.target_path.clone(),
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
fn origin(dir: &Path) -> Option<RepoUrl> {
    match Checkout::of(dir) {
        Checkout::Repo { .. } => meta_core::git::remote_url(dir).ok().flatten(),
        _ => None,
    }
}

/// Whether two URLs name the same repository, local ones by the folder they
/// lead to.
fn same_repository((a, b): (&RepoUrl, &RepoUrl)) -> bool {
    let local = |url: &RepoUrl| {
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

    /// The top of a test's tree. The configs are read by meta: the one
    /// built beside this test (`cargo build --workspace --bins` first).
    fn top(dir: &Path, url: Option<&str>) -> Arc<[MetaRepo]> {
        crate::here::tests::use_built_meta();
        Arc::from([MetaRepo::at("ws", dir, url.map(RepoUrl::from))])
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
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        std::fs::create_dir(dir.join("child")).unwrap();
        sandbox.git(&dir.join("child"), &["init", "-q"]);
        write_meta(
            &dir,
            r#"{"projects": {"child": {"repo": "u", "meta": true}, "sib": "u2"}}"#,
        );
        std::os::unix::fs::symlink("missing.yaml", dir.join("child/.meta.yaml")).unwrap();
        let queue = CloneQueue::new(None, Some(0), false);
        let tasks = queue.discover(&dir, 0, &top(&dir, None)).unwrap();
        assert_eq!(names(&tasks), ["sib"]);
    }

    #[cfg(unix)]
    #[test]
    fn discover_fails_on_a_nested_config_it_cannot_read() {
        // child/.meta.yaml is a link to nowhere: its repos are unknown, which is
        // an error, not "no nested repos"
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        std::fs::create_dir(dir.join("child")).unwrap();
        sandbox.git(&dir.join("child"), &["init", "-q"]);
        write_meta(
            &dir,
            r#"{"projects": {"child": {"repo": "u", "meta": true}}}"#,
        );
        std::os::unix::fs::symlink("missing.yaml", dir.join("child/.meta.yaml")).unwrap();
        let queue = CloneQueue::new(None, None, false);
        let err = queue.discover(&dir, 0, &top(&dir, None)).unwrap_err();
        assert!(format!("{err:#}").contains("Cannot read"), "{err:#}");
    }

    #[test]
    fn discover_clones_only_where_no_repository_is_and_pulls_the_rest() {
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        // a clone, an empty folder git can clone into, and a plain folder
        for name in ["cloned", "empty", "plain"] {
            std::fs::create_dir(dir.join(name)).unwrap();
        }
        sandbox.git(&dir.join("cloned"), &["init", "-q"]);
        std::fs::write(dir.join("plain/notes.txt"), "mine").unwrap();
        write_meta(
            &dir,
            r#"{"projects": {"cloned": "u1", "empty": "u2", "plain": "u3", "missing": "u4",
                "no-repo": {"path": "no-repo"}}}"#,
        );

        let queue = CloneQueue::new(None, None, true);
        let tasks = queue.discover(&dir, 0, &top(&dir, None)).unwrap();
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
            [(
                "plain".to_string(),
                dir.join("plain"),
                "folder exists, no .git".to_string()
            )]
        );
        // a folder is handed out once
        assert!(queue
            .discover(&dir, 0, &top(&dir, None))
            .unwrap()
            .is_empty());
        assert_eq!(queue.get_counts(), (0, 3));
    }

    #[test]
    fn a_nested_meta_repo_is_read_once_its_task_is_done() {
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        write_meta(
            &dir,
            r#"{"projects": {"child": {"repo": "u", "meta": true}}}"#,
        );

        let queue = CloneQueue::new(None, None, false);
        let tasks = queue.discover(&dir, 0, &top(&dir, None)).unwrap();
        assert_eq!(names(&tasks), ["child"]);
        // as the clone leaves it
        let child = dir.join("child");
        std::fs::create_dir(&child).unwrap();
        write_meta(&child, r#"{"projects": {"grandchild": "u2"}}"#);
        assert!(tasks[0].is_meta);
        let nested = queue.mark_completed(&tasks[0]);
        assert_eq!(names(&nested), ["grandchild"]);
        assert_eq!(queue.outcome().pulled, 0);
        assert_eq!(queue.outcome().cloned, [child]);

        // with depth limit 0, the clone's .meta.yaml isn't read
        let queue = CloneQueue::new(None, Some(0), false);
        assert!(queue.mark_completed(&tasks[0]).is_empty());
    }

    #[test]
    fn a_meta_repo_that_lists_one_above_it_is_refused_by_name() {
        // ws lists kid, whose .meta.yaml lists ws's own URL
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        write_meta(
            &dir,
            r#"{"projects": {"kid": {"repo": "kid.git", "meta": true}}}"#,
        );
        let queue = CloneQueue::new(None, None, false);
        let tasks = queue
            .discover(&dir, 0, &top(&dir, Some("top.git")))
            .unwrap();
        std::fs::create_dir(dir.join("kid")).unwrap();
        write_meta(
            &dir.join("kid"),
            r#"{"projects": {"top": {"repo": "top.git", "meta": true}, "lib": "lib.git"}}"#,
        );
        let nested = queue.mark_completed(&tasks[0]);
        assert_eq!(names(&nested), ["lib"]);
        assert_eq!(queue.outcome().cycles, [vec!["ws", "kid", "top"]]);
    }

    #[test]
    fn two_meta_repos_listing_one_repo_is_no_cycle_and_local_urls_match_by_folder() {
        // b and c both list d; d lists the workspace by another spelling
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        let ws = &dir;
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
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        write_meta(
            &dir,
            r#"{"projects": {"pages": {"repo": "top.git", "meta": true}}}"#,
        );
        let queue = CloneQueue::new(None, Some(0), true);
        let tasks = queue
            .discover(&dir, 0, &top(&dir, Some("top.git")))
            .unwrap();
        assert_eq!(names(&tasks), ["pages"]);
        assert!(queue.outcome().cycles.is_empty());
    }

    #[test]
    fn an_undeclared_meta_repo_that_repeats_one_above_it_is_cloned_once_and_not_entered() {
        // ws lists kid, kid lists top (ws's own repo); neither says meta: true
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        let ws = &dir;
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
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        let path = dir.to_string_lossy().into_owned();
        let url = RepoUrl::from(format!("file:///{}", path.replace('\\', "/")));
        assert!(same_repository((&RepoUrl::from(path), &url)));
    }

    #[cfg(unix)]
    #[test]
    fn a_project_whose_folder_leads_back_up_is_a_cycle() {
        // ws/kid leads out of the workspace, to a meta repo whose `back`
        // leads to ws again
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        let (ws, kid) = (dir.join("ws"), dir.join("kid"));
        for repo in [&ws, &kid] {
            std::fs::create_dir(repo).unwrap();
            sandbox.git(repo, &["init", "-q"]);
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
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        for name in ["listed", "other", "plain"] {
            std::fs::create_dir(dir.join(name)).unwrap();
        }
        sandbox.git(&dir.join("listed"), &["init", "-q"]);
        sandbox.git(&dir.join("other"), &["init", "-q"]);
        write_meta(&dir, r#"{"projects": {"listed": "u"}}"#);
        let queue = CloneQueue::new(None, None, true);
        queue.discover(&dir, 0, &top(&dir, None)).unwrap();
        assert_eq!(queue.outcome().unlisted, [dir.join("other")]);
    }

    #[test]
    fn a_repository_that_holds_a_listed_project_is_no_leftover() {
        // libs is a repo of its own, unlisted, but the project libs/x is in it
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        for name in ["libs", "libs/x", "stray"] {
            std::fs::create_dir(dir.join(name)).unwrap();
            sandbox.git(&dir.join(name), &["init", "-q"]);
        }
        write_meta(
            &dir,
            r#"{"projects": {"x": {"repo": "u", "path": "libs/x"}}}"#,
        );
        let queue = CloneQueue::new(None, None, false);
        queue.discover(&dir, 0, &top(&dir, None)).unwrap();
        assert_eq!(queue.outcome().unlisted, [dir.join("stray")]);
    }

    #[test]
    fn failed_tasks_are_reported_with_their_error() {
        let sandbox = meta_plugin::testing::isolated!();
        let dir = sandbox.work();
        write_meta(&dir, r#"{"projects": {"missing": "u"}}"#);
        let queue = CloneQueue::new(None, None, true);
        let tasks = queue.discover(&dir, 0, &top(&dir, None)).unwrap();
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
