//! Shared configuration types and parsing for meta config files.
//!
//! This module provides the core types and functions for finding and parsing
//! meta's YAML configuration files.

use crate::display::{escape, escape_path, is_unsafe_char};
use crate::git::GitUrl;
use crate::gitdir;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// Represents a project entry in the .meta.yaml.
/// Can be either a simple git URL string or an extended object with additional fields.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ProjectEntry {
    /// Simple format: just a git URL string
    Simple(GitUrl),
    /// Extended format: object with repo, path, tags, and dependency info
    Extended {
        /// Git remote URL (optional for local-only projects).
        #[serde(default)]
        repo: Option<GitUrl>,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        provides: Vec<String>,
        #[serde(default)]
        depends_on: Vec<String>,
        /// If true, this directory contains a nested .meta.yaml
        #[serde(default)]
        meta: bool,
    },
}

/// Parsed project info after normalization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    pub path: String,
    /// Git remote URL (optional for local-only projects).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<GitUrl>,
    pub tags: Vec<String>,
    /// What this project provides (e.g., APIs, libraries)
    #[serde(default)]
    pub provides: Vec<String>,
    /// What this project depends on (other project names or provided items)
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// If true, this directory contains a nested .meta.yaml
    #[serde(default)]
    pub meta: bool,
}

/// Default settings that can be configured in .meta.yaml
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct MetaDefaults {
    /// Run commands in parallel by default (defaults to true)
    #[serde(default = "default_true")]
    pub parallel: bool,
}

fn default_true() -> bool {
    true
}

impl Default for MetaDefaults {
    fn default() -> Self {
        Self { parallel: true }
    }
}

/// The `worktree` section of .meta.yaml.
#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct WorktreeConfig {
    /// Shell commands run at worktree events (`post-create`, ...), by event.
    #[serde(default, deserialize_with = "commands")]
    pub hooks: BTreeMap<String, String>,
}

/// A map of commands, each a YAML string: YAML would read `false` or `123`
/// as the command "false" or "123".
fn commands<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    BTreeMap::<String, serde_yaml_ng::Value>::deserialize(deserializer)?
        .into_iter()
        .map(|(name, value)| match value {
            serde_yaml_ng::Value::String(command) => Ok((name, command)),
            _ => Err(serde::de::Error::custom(format!(
                "hook '{}' must be a command string",
                escape(&name)
            ))),
        })
        .collect()
}

/// The meta configuration file structure. A key it doesn't know is an
/// error, so a misspelled one isn't silently ignored (project entries may
/// carry keys of their own for other tools).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetaConfig {
    /// Projects in this meta repo. Values can be:
    /// - A git URL string: `project: git@github.com:org/repo.git`
    /// - An extended object: `project: { repo: ..., path: ..., meta: true }`
    #[serde(default, deserialize_with = "unique_projects")]
    pub projects: HashMap<String, ProjectEntry>,
    #[serde(default)]
    pub defaults: MetaDefaults,
    /// Custom directory for worktrees (overrides default .worktrees/)
    #[serde(default)]
    pub worktrees_dir: Option<String>,
    #[serde(default)]
    pub worktree: WorktreeConfig,
}

/// A project map in which no key appears twice: serde would keep the last.
fn unique_projects<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<HashMap<String, ProjectEntry>, D::Error> {
    struct ProjectsVisitor;
    impl<'de> serde::de::Visitor<'de> for ProjectsVisitor {
        type Value = HashMap<String, ProjectEntry>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a map of unique project keys")
        }

        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut projects = HashMap::new();
            while let Some(key) = map.next_key::<String>()? {
                if projects.contains_key(&key) {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate project key '{}'",
                        escape(&key)
                    )));
                }
                projects.insert(key, map.next_value()?);
            }
            Ok(projects)
        }
    }
    deserializer.deserialize_map(ProjectsVisitor)
}

/// The config filename meta looks for.
pub const META_CONFIG_NAME: &str = ".meta.yaml";

/// `path` if it is a file; one that can't be examined or read is an error.
fn config_file(path: PathBuf) -> anyhow::Result<Option<PathBuf>> {
    let unreadable = |e| anyhow::anyhow!("Cannot read {}: {e}", escape_path(&path));
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unreadable(e)),
        Ok(_) => {}
    }
    if !std::fs::metadata(&path).map_err(unreadable)?.is_file() {
        return Ok(None);
    }
    std::fs::File::open(&path).with_context(|| format!("Cannot read {}", escape_path(&path)))?;
    Ok(Some(path))
}

/// The [`META_CONFIG_NAME`] file directly in `dir`. A candidate that cannot
/// be examined is an error. A `.meta` directory (for plugins and skills) is
/// never a config.
pub fn find_meta_config_in(dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    match std::fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("Cannot read {}", escape_path(dir))),
    }
    config_file(dir.join(META_CONFIG_NAME))
}

/// Find the nearest config, walking from `start_dir` to the filesystem root.
/// Uses [`META_CONFIG_NAME`], refusing unreadable configs.
/// An explicit `config_name` selects only that filename instead.
pub fn find_meta_config(
    start_dir: &Path,
    config_name: Option<&PathBuf>,
) -> anyhow::Result<Option<PathBuf>> {
    for dir in start_dir.ancestors() {
        let found = meta_config_at(dir, config_name)?;
        if found.is_some() {
            return Ok(found);
        }
    }
    Ok(None)
}

/// The config in `dir` itself: `config_name` if given, else the default.
pub fn meta_config_at(
    dir: &Path,
    config_name: Option<&PathBuf>,
) -> anyhow::Result<Option<PathBuf>> {
    let Some(name) = config_name else {
        return find_meta_config_in(dir);
    };
    config_file(dir.join(name))
}

/// The meta directories of the workspace `meta_dir` belongs to, nearest
/// first and its root last: `meta_dir`, then each meta directory above it
/// whose own project list names the one before. A meta directory its parent
/// doesn't list (or whose parent's config can't be read) is a workspace of
/// its own. In a worktree set meta made, the set's own meta directories come
/// first, then those of the checkout it stands for ([`set_source`]) and its
/// workspace.
pub fn workspace_meta_dirs(meta_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = listed_chain(meta_dir);
    // A checkout in a set meta made goes on with the checkout it stands for
    let source = dirs.iter().find_map(|dir| {
        set_source(dir).filter(|source| matches!(find_meta_config_in(source), Ok(Some(_))))
    });
    for dir in source.as_deref().map(listed_chain).unwrap_or_default() {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// `meta_dir`, then each meta directory above it whose own project list
/// names the one before.
fn listed_chain(meta_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![meta_dir.to_path_buf()];
    for parent in meta_dir.ancestors().skip(1) {
        match find_meta_config_in(parent) {
            Ok(None) => continue,
            Ok(Some(config)) if dirs.last().is_some_and(|dir| lists(&config, dir)) => {
                dirs.push(parent.to_path_buf())
            }
            _ => break,
        }
    }
    dirs
}

/// The root of the workspace `meta_dir` belongs to (see [`workspace_meta_dirs`]).
pub fn workspace_root(meta_dir: &Path) -> PathBuf {
    workspace_meta_dirs(meta_dir)
        .pop()
        .unwrap_or_else(|| meta_dir.to_path_buf())
}

/// Whether the project list in `config` names the directory `dir`.
fn lists(config: &Path, dir: &Path) -> bool {
    let (Some(parent), Ok(workspace), Ok(dir)) =
        (config.parent(), Workspace::load(config), dir.canonicalize())
    else {
        return false;
    };
    workspace.projects.iter().any(|p| {
        parent
            .join(&p.path)
            .canonicalize()
            .is_ok_and(|path| path == dir)
    })
}

/// The file in a linked worktree's git admin directory
/// (`<repo>/.git/worktrees/<id>/`) where meta records the worktree set it
/// made the worktree for.
pub const WORKTREE_RECORD_FILE: &str = "meta-worktree.json";

/// The checkout that `dir`, a checkout in a worktree set meta made, stands
/// for: the `project` the set was made from, joined with `dir`'s place in
/// the set (meta lays a set out as the workspace). The project comes from
/// meta's record in the checkout's git admin directory
/// (`<repo>/.git/worktrees/<id>/`, which its `.git` file names). A set made
/// from inside another set leads on to that set's source. A worktree made by
/// hand has no record and stands for itself, as a second workspace can.
/// Git's shared repository doesn't say which checkout made a set, so only
/// the record is used.
pub fn set_source(dir: &Path) -> Option<PathBuf> {
    let mut source = recorded_source(dir)?;
    let mut seen = vec![dir.to_path_buf()];
    while let Some(next) = recorded_source(&source) {
        if seen.contains(&next) {
            break;
        }
        seen.push(std::mem::replace(&mut source, next));
    }
    Some(source)
}

/// The meta directory meta's record says the set holding the checkout at
/// `dir` was made from.
pub fn recorded_project(dir: &Path) -> Option<PathBuf> {
    let record = worktree_record(dir)?;
    Some(PathBuf::from(record.get("project")?.as_str()?))
}

/// The set meta's record says the checkout at `dir` was made for, resolved.
pub fn recorded_set(dir: &Path) -> Option<PathBuf> {
    let record = worktree_record(dir)?;
    Path::new(record.get("set")?.as_str()?).canonicalize().ok()
}

/// Meta's record of the linked worktree at `dir`, from git's admin directory
/// for it (which its `.git` file names).
fn worktree_record(dir: &Path) -> Option<serde_json::Value> {
    let record = std::fs::read_to_string(record_path(dir)?).ok()?;
    serde_json::from_str(&record).ok()
}

/// Where meta's record of the linked worktree at `dir` goes: in git's admin
/// directory for it, which its `.git` file names.
fn record_path(dir: &Path) -> Option<PathBuf> {
    Some(
        gitdir::gitfile_target(dir)
            .ok()??
            .join(WORKTREE_RECORD_FILE),
    )
}

/// One step of [`set_source`].
fn recorded_source(dir: &Path) -> Option<PathBuf> {
    let project = recorded_project(dir)?;
    let set = recorded_set(dir)?;
    let place = dir.canonicalize().ok()?;
    let place = place.strip_prefix(set).ok()?;
    // meta puts each repo of a set where it is in the workspace
    let source = project.join(place);
    // The set's root stands for the workspace itself. Another checkout
    // stands for a project of that workspace with the same repository: the
    // one at its place, else (a link that differs between the root's
    // branches put it elsewhere) the only one there is. Its place alone
    // doesn't say: a second workspace's `git worktree add` shares the
    // repository but is no project.
    let Some(own) = common_git_dir(dir) else {
        return Some(source);
    };
    if place.as_os_str().is_empty() {
        return Some(source);
    }
    let mut matches = Vec::new();
    projects_with_git_dir(&project, &own, 0, &mut matches);
    let at_place = source.canonicalize().ok();
    match matches.as_slice() {
        [only] => Some(only.clone()),
        // no project says (one no longer listed, say): its place, if that
        // is its repository
        [] => (common_git_dir(&source).as_ref() == Some(&own)).then_some(source),
        _ => matches.into_iter().find(|path| {
            path.canonicalize()
                .ok()
                .is_some_and(|real| Some(real) == at_place)
        }),
    }
}

/// The projects of the meta repo at `meta_dir`, and of the meta repos nested
/// in it, whose repository's git directory is `git_dir`. A config that can't
/// be read is passed over: one broken sibling doesn't hide a project.
fn projects_with_git_dir(meta_dir: &Path, git_dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth > MAX_LINK_HOPS {
        return;
    }
    let Ok(Some(workspace)) = Workspace::in_dir(meta_dir) else {
        return;
    };
    for project in &workspace.projects {
        let path = meta_dir.join(&project.path);
        if common_git_dir(&path).as_deref() == Some(git_dir) {
            found.push(path.clone());
        }
        projects_with_git_dir(&path, git_dir, depth + 1, found);
    }
}

/// The git directory the checkout at `dir` shares with its linked worktrees,
/// resolved.
fn common_git_dir(dir: &Path) -> Option<PathBuf> {
    gitdir::common_dir(dir).ok()??.canonicalize().ok()
}

/// Nearest directory holding a config file: where `--local` plugins go.
/// Plugin scope follows cwd, independently of a command's explicit `--config`.
pub fn nearest_meta_dir(start_dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    Ok(find_meta_config(start_dir, None)?.and_then(|path| path.parent().map(Path::to_path_buf)))
}

/// A `.meta.yaml`, read and checked once: the workspace it describes.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// The directory of the config, where project paths start.
    pub dir: PathBuf,
    pub config_path: PathBuf,
    /// Checked and normalized, in name order.
    pub projects: Vec<ProjectInfo>,
    pub defaults: MetaDefaults,
    /// `worktrees_dir` as configured, checked and in normal form relative
    /// to `dir` (to the checkout a set stands for, inside a set).
    pub worktrees_dir: Option<String>,
    pub worktree: WorktreeConfig,
}

impl Workspace {
    /// The workspace of the nearest config at or above `start` (see
    /// [`find_meta_config`]).
    pub fn find(start: &Path, config_name: Option<&PathBuf>) -> anyhow::Result<Option<Self>> {
        find_meta_config(start, config_name)?
            .map(|path| Self::load(&path))
            .transpose()
    }

    /// The workspace whose config is directly in `dir`, if there is one.
    pub fn in_dir(dir: &Path) -> anyhow::Result<Option<Self>> {
        find_meta_config_in(dir)?
            .map(|path| Self::load(&path))
            .transpose()
    }

    /// The workspace at the top of the tree this one is nested in (see
    /// [`find_root_meta_dir`]); this one at the top.
    pub fn root(self) -> anyhow::Result<Self> {
        let root = find_root_meta_dir(&self.dir)?;
        if root == self.dir {
            return Ok(self);
        }
        Ok(Self::in_dir(&root)?.unwrap_or(self))
    }

    /// Read and check the config at `meta_path`.
    ///
    /// Every project is checked by [`validate_project`] and no two projects
    /// may share or nest paths ([`check_no_overlap`]); a bad entry fails the
    /// load with the file and project named.
    pub fn load(meta_path: &Path) -> anyhow::Result<Self> {
        let config_str = std::fs::read_to_string(meta_path).with_context(|| {
            format!(
                "Failed to read meta config file: '{}'",
                escape_path(meta_path)
            )
        })?;

        let config: MetaConfig = serde_yaml_ng::from_str(&config_str).with_context(|| {
            format!(
                "Failed to parse YAML config file: {}",
                escape_path(meta_path)
            )
        })?;

        // Convert project entries to normalized, validated ProjectInfo
        // in name order, so the first bad entry reported is always the same one
        let base_dir = meta_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        // Where worktrees live (as written, and where that really is); no
        // project may go there. A bad worktrees_dir is an error like a bad entry.
        let worktrees = match config.worktrees_dir.as_deref() {
            Some(dir) => relative_dir(base_dir, dir)
                .or_else(|why| {
                    // A set's checkout carries the config of the checkout it
                    // stands for, whose absolute worktrees_dir is inside that one
                    match set_source(base_dir) {
                        Some(source) if Path::new(dir).is_absolute() => relative_dir(&source, dir),
                        _ => Err(why),
                    }
                })
                .map_err(|why| {
                    anyhow::anyhow!("Invalid worktrees_dir in {}: {why}", escape_path(meta_path))
                })?,
            None => ".worktrees".to_string(),
        };
        let worktrees_at = resolve_inside(base_dir, &worktrees)
            .ok()
            .map(|at| PathBuf::from(at.to_string_lossy().to_lowercase()));
        let mut entries: Vec<_> = config.projects.into_iter().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let checked: Vec<(ProjectInfo, PathBuf)> = entries
            .into_iter()
            .map(|(name, entry)| {
                let (repo, path, tags, provides, depends_on, meta) = match entry {
                    // Simple string -> git URL
                    ProjectEntry::Simple(url) => {
                        (Some(url), name.clone(), vec![], vec![], vec![], false)
                    }
                    // Extended object -> repo with additional fields
                    // meta: true indicates this project is also a meta-repo (has its own .meta.yaml)
                    ProjectEntry::Extended {
                        repo,
                        path,
                        tags,
                        provides,
                        depends_on,
                        meta,
                    } => {
                        let resolved_path = path.unwrap_or_else(|| name.clone());
                        (repo, resolved_path, tags, provides, depends_on, meta)
                    }
                };
                let mut info = ProjectInfo {
                    name,
                    path,
                    repo,
                    tags,
                    provides,
                    depends_on,
                    meta,
                };
                let location =
                    validate_project(base_dir, &worktrees, worktrees_at.as_deref(), &mut info)
                        .map_err(|reason| {
                            anyhow::anyhow!(
                                "Invalid project '{}' in {}: {reason}",
                                escape(&info.name),
                                escape_path(meta_path)
                            )
                        })?;
                Ok((info, location))
            })
            .collect::<anyhow::Result<_>>()?;
        check_no_overlap(&checked)
            .map_err(|e| anyhow::anyhow!("Invalid projects in {}: {e}", escape_path(meta_path)))?;
        // Nor may the worktree directory sit inside a project's checkout
        if let Some(at) = &worktrees_at {
            for (info, location) in &checked {
                if at.starts_with(PathBuf::from(location.to_string_lossy().to_lowercase())) {
                    anyhow::bail!(
                        "Invalid worktrees_dir in {}: '{}' is inside project '{}'",
                        escape_path(meta_path),
                        escape(&worktrees),
                        escape(&info.name)
                    );
                }
            }
        }
        let projects = checked.into_iter().map(|(info, _)| info).collect();
        Ok(Self {
            dir: base_dir.to_path_buf(),
            config_path: meta_path.to_path_buf(),
            projects,
            defaults: config.defaults,
            worktrees_dir: config.worktrees_dir.map(|_| worktrees),
            worktree: config.worktree,
        })
    }
}

/// Check one project entry and normalize its path, or say what is wrong.
///
/// These rules are checked once, when a config is parsed, so every caller can
/// join `path` onto the directory of the `.meta.yaml` without checking again:
/// - No value (name, path, repo, tags, provides, depends_on) is empty or holds
///   a control or bidirectional-control character ([`is_unsafe_char`]).
/// - `name` is a relative path in normal form (usually one component; a
///   nested alias like `vendor/lib` names a nested repo), held to
///   the same rules as `path`.
/// - The name and every path component pass [`component_problem`]: ASCII
///   letters, digits, space and `._-+@` only, no trailing dot or space, no
///   Windows device name, not `.git`. The first component is not `.worktrees`.
/// - `path` (the name when omitted) is relative and stays inside the config's
///   directory: `\` reads as `/`, empty and `.` components are dropped, `..`
///   is rejected, something must remain, and no symlink along it may lead
///   outside the directory.
/// - `repo` does not start with `-`, so git cannot read it as an option.
/// - Tags hold no whitespace and no `,`, which the `--tag` filter splits on.
///
/// Returns where the project lives on disk (see [`resolve_inside`]).
fn validate_project(
    base_dir: &Path,
    worktrees: &str,
    worktrees_at: Option<&Path>,
    info: &mut ProjectInfo,
) -> Result<PathBuf, String> {
    let lists = [
        ("tag", &info.tags),
        ("provides entry", &info.provides),
        ("depends_on entry", &info.depends_on),
    ];
    for (what, value) in [("name", &info.name), ("path", &info.path)]
        .into_iter()
        .chain(
            lists
                .into_iter()
                .flat_map(|(what, l)| l.iter().map(move |v| (what, v))),
        )
    {
        if value.is_empty() {
            return Err(format!("{what} is empty"));
        }
        if value.chars().any(is_unsafe_char) {
            return Err(format!(
                "{what} '{}' contains control characters",
                escape(value)
            ));
        }
    }
    // A name may be a relative path, such as `vendor/lib` for a nested
    // repo, and names a project in commands. So it follows the path rules,
    // and must already be in their normal form.
    let normal = normalize_project_path(&info.name).map_err(|why| format!("name: {why}"))?;
    if normal != info.name {
        return Err(format!(
            "name must be a plain relative path such as 'api' or 'vendor/lib', not '{}'",
            info.name
        ));
    }
    if let Some(repo) = &info.repo {
        if repo.is_empty() {
            return Err("repo is empty".into());
        }
        // printed as a GitUrl prints: escaped, without its credentials
        if repo.raw().chars().any(is_unsafe_char) {
            return Err(format!("repo '{repo}' contains control characters"));
        }
        if repo.raw().starts_with('-') {
            return Err("repo must not start with '-'".into());
        }
    }
    if let Some(tag) = info
        .tags
        .iter()
        .find(|t| t.contains(|c: char| c.is_whitespace() || c == ','))
    {
        return Err(format!("tag '{tag}' contains whitespace or ','"));
    }
    info.path = normalize_project_path(&info.path)?;
    // The configured worktree directory, and the default one always
    let in_worktrees = |value: &str| {
        let value = value.to_ascii_lowercase();
        [worktrees.to_ascii_lowercase(), ".worktrees".to_string()]
            .iter()
            .any(|dir| value == *dir || value.starts_with(&format!("{dir}/")))
    };
    for value in [&info.name, &info.path] {
        if in_worktrees(value) {
            return Err(format!(
                "'{value}' is inside {worktrees}, where worktrees live"
            ));
        }
    }
    let location = resolve_inside(base_dir, &info.path)?;
    // Where it really goes must not be reserved either (`alias -> .git`)
    let base = base_dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", escape_path(base_dir)))?;
    if let Ok(inside) = location.strip_prefix(&base) {
        let parts: Vec<String> = inside
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let lowered = PathBuf::from(location.to_string_lossy().to_lowercase());
        if parts.iter().any(|p| p.eq_ignore_ascii_case(".git"))
            || in_worktrees(&parts.join("/"))
            || worktrees_at.is_some_and(|at| lowered.starts_with(at))
        {
            return Err(format!(
                "path '{}' leads into .git or {worktrees} through a symlink",
                info.path
            ));
        }
    }
    Ok(location)
}

/// Check a directory a config names relative to its own directory, such as
/// `worktrees_dir`, by the rules for project paths, and return its normal
/// form. (An absolute or outside location is the user's to choose, through
/// an environment variable such as `META_WORKTREES`, not the config's.)
pub fn relative_dir(base_dir: &Path, raw: &str) -> Result<String, String> {
    // An absolute path is fine when it lies inside base_dir
    let raw_path = Path::new(raw);
    let relative = if raw_path.is_absolute() {
        // the real path of the nearest part that exists, then the rest
        let real = |p: &Path| {
            let existing = p.ancestors().find(|a| a.exists())?;
            let rest = p.strip_prefix(existing).ok()?;
            Some(existing.canonicalize().ok()?.join(rest))
        };
        real(base_dir)
            .zip(real(raw_path))
            .and_then(|(base, path)| {
                path.strip_prefix(base)
                    .ok()
                    .map(|rest| rest.to_string_lossy().into_owned())
            })
            .ok_or_else(|| format!("'{}' is outside {}", escape(raw), escape_path(base_dir)))?
    } else {
        raw.to_string()
    };
    let normal = normalize_project_path(&relative)?;
    let location = resolve_inside(base_dir, &normal)?;
    // Nor may it lead into git's own data
    let base = base_dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", escape_path(base_dir)))?;
    if location.strip_prefix(&base).is_ok_and(|inside| {
        inside
            .components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case(".git"))
    }) {
        return Err(format!("'{}' leads into .git", escape(raw)));
    }
    Ok(normal)
}

/// Refuse two projects at one location, or one inside another's directory:
/// a clone of the outer repo could check out a symlink that the inner path
/// then follows out of the workspace. Locations are compared as resolved
/// through existing symlinks, and without case, which some filesystems
/// ignore. A repo inside another belongs in a nested `.meta.yaml` (`meta: true`).
fn check_no_overlap(projects: &[(ProjectInfo, PathBuf)]) -> Result<(), String> {
    // Paths, by where they resolve; and names, which must name one project
    // each in commands
    let by_location: Vec<_> = projects
        .iter()
        .map(|(info, location)| {
            (
                PathBuf::from(location.to_string_lossy().to_lowercase()),
                info,
            )
        })
        .collect();
    if let Some((outer, inner)) = nested(&by_location) {
        return Err(format!(
            "project '{}' (path '{}') is at or inside project '{}' (path '{}'); \
             list it in a nested .meta.yaml instead",
            escape(&inner.name),
            escape(&inner.path),
            escape(&outer.name),
            escape(&outer.path)
        ));
    }
    let by_name: Vec<_> = projects
        .iter()
        .map(|(info, _)| (PathBuf::from(info.name.to_ascii_lowercase()), info))
        .collect();
    if let Some((outer, inner)) = nested(&by_name) {
        return Err(format!(
            "project name '{}' is the same as or inside '{}'; names must not overlap",
            escape(&inner.name),
            escape(&outer.name)
        ));
    }
    Ok(())
}

/// The first pair whose keys are equal or nested, outer first.
fn nested<'a>(keyed: &[(PathBuf, &'a ProjectInfo)]) -> Option<(&'a ProjectInfo, &'a ProjectInfo)> {
    for (i, (outer_key, outer)) in keyed.iter().enumerate() {
        for (inner_key, inner) in keyed.iter().skip(i + 1) {
            if inner_key.starts_with(outer_key) {
                return Some((outer, inner));
            }
            if outer_key.starts_with(inner_key) {
                return Some((inner, outer));
            }
        }
    }
    None
}

/// What is wrong with a name or path component, if anything. The same
/// `.meta.yaml` serves every platform, so each must name one and the same
/// directory everywhere:
/// - only ASCII letters, digits, space and `._-+@`, which no filesystem
///   folds, normalizes or treats specially (Unicode names can alias each
///   other on macOS and Windows; `<>:"|?*` and `C:` mean something on Windows);
/// - no trailing dot or space, which Windows drops (`a.` is `a` there);
/// - not a Windows device name such as `NUL` or `COM1`;
/// - not `.git`, which would put a repo inside a git directory (hooks).
fn component_problem(part: &str) -> Option<&'static str> {
    if !part
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-' | '+' | '@'))
    {
        return Some("a character other than ASCII letters, digits, space and ._-+@");
    }
    if part.ends_with(['.', ' ']) {
        return Some("a trailing dot or space, which Windows drops");
    }
    let stem = part
        .split('.')
        .next()
        .unwrap_or(part)
        .trim_end()
        .to_ascii_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix)
            .is_some_and(|n| n.len() == 1 && n.chars().all(|c| c.is_ascii_digit()))
    };
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT")
    {
        return Some("a Windows device name");
    }
    if part.eq_ignore_ascii_case(".git") {
        return Some("the name .git");
    }
    None
}

/// Normalize a project path to `a/b` form, rejecting anything that is not a
/// plain relative path below the config's directory.
fn normalize_project_path(raw: &str) -> Result<String, String> {
    let unified = raw.replace('\\', "/");
    if unified.starts_with('/') {
        return Err(format!("path '{}' must be relative", escape(raw)));
    }
    let mut parts = Vec::new();
    for part in unified.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(format!("path '{}' must not contain '..'", escape(raw))),
            part => match component_problem(part) {
                Some(why) => {
                    return Err(format!("path '{}' has a component with {why}", escape(raw)))
                }
                None => parts.push(part),
            },
        }
    }
    if parts.is_empty() {
        return Err(format!(
            "path '{}' must name a directory below the .meta.yaml file",
            escape(raw)
        ));
    }
    Ok(parts.join("/"))
}

/// Resolve `rel` below `base_dir` through the symlinks that exist now, and
/// refuse it if one leads outside `base_dir` or if it ends at `base_dir`.
/// Returns the project's location: the real path of the part that exists,
/// followed by the part not created yet.
pub fn resolve_inside(base_dir: &Path, rel: &str) -> Result<PathBuf, String> {
    resolve_inside_after(base_dir, rel, 0)
}

/// Links followed for a missing target before giving up, as the OS does.
const MAX_LINK_HOPS: usize = 40;

/// [`resolve_inside`], `hops` links into following missing targets.
fn resolve_inside_after(base_dir: &Path, rel: &str, hops: usize) -> Result<PathBuf, String> {
    let base = base_dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", escape_path(base_dir)))?;
    if rel.starts_with('/') || rel.split('/').any(|part| matches!(part, "" | "." | "..")) {
        return Err(format!(
            "path '{}' must be a plain relative path",
            escape(rel)
        ));
    }
    let mut current = base.clone();
    let mut exists = true;
    for part in rel.split('/') {
        current.push(part);
        if !exists {
            continue;
        }
        match current.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => exists = false,
            // What can't be examined (no permission, say) may hold a
            // symlink out, so it is refused rather than taken as missing
            Err(e) => {
                return Err(format!(
                    "path '{}' cannot be checked at '{}': {e}",
                    escape(rel),
                    escape_path(&current)
                ))
            }
            Ok(meta) if meta.file_type().is_symlink() => match current.canonicalize() {
                Ok(target) if target.starts_with(&base) => current = target,
                // A link to what isn't there yet (a repo a checkout links
                // to, before its own checkout): checked as the path it names
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    current = dangling_target(&base, &current, rel, hops + 1)?;
                    // the rest is checked too: part of it may exist
                    exists = current.symlink_metadata().is_ok();
                }
                Ok(_) => {
                    return Err(format!(
                        "path '{}' leads outside {} through the symlink '{}'",
                        escape(rel),
                        escape_path(&base),
                        escape_path(&current)
                    ))
                }
                // Its target may not exist yet, but where it would lead
                // cannot be checked, so it is refused
                Err(e) => {
                    return Err(format!(
                        "path '{}' goes through the symlink '{}', which cannot be resolved: {e}",
                        escape(rel),
                        escape_path(&current)
                    ))
                }
            },
            Ok(_) => {}
        }
    }
    if current == base {
        return Err(format!(
            "path '{}' leads to the directory of the .meta.yaml file itself",
            escape(rel)
        ));
    }
    Ok(current)
}

/// Where the symlink `link` inside `base` leads when its target is missing:
/// the target's path, with each link on the way followed before a `..`
/// after it, then checked with [`resolve_inside`].
fn dangling_target(base: &Path, link: &Path, rel: &str, hops: usize) -> Result<PathBuf, String> {
    let unresolved = |at: &Path, e: std::io::Error| {
        format!(
            "path '{}' goes through the symlink '{}', which cannot be resolved: {e}",
            escape(rel),
            escape_path(at)
        )
    };
    if hops > MAX_LINK_HOPS {
        return Err(unresolved(
            link,
            std::io::Error::other("too many levels of symbolic links"),
        ));
    }
    let target = std::fs::read_link(link).map_err(|e| unresolved(link, e))?;
    let mut path = link.parent().unwrap_or(base).to_path_buf();
    for part in target.components() {
        match part {
            // `..` leaves a directory only if it is there, as the OS has it
            std::path::Component::ParentDir => {
                if !path.is_dir() {
                    return Err(unresolved(
                        link,
                        std::io::Error::from(std::io::ErrorKind::NotFound),
                    ));
                }
                path.pop();
            }
            std::path::Component::CurDir => {}
            // What exists is taken as the OS resolves it (links followed,
            // and in the base's form: `\\?\` on Windows, which an absolute
            // target doesn't carry); a link to what is missing is followed here
            std::path::Component::Normal(_) => {
                path.push(part);
                match path.canonicalize() {
                    Ok(real) => path = real,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        if path
                            .symlink_metadata()
                            .is_ok_and(|meta| meta.file_type().is_symlink())
                        {
                            path = dangling_target(base, &path, rel, hops + 1)?;
                        }
                    }
                    Err(e) => return Err(unresolved(&path, e)),
                }
            }
            // The root of an absolute target
            std::path::Component::Prefix(_) | std::path::Component::RootDir => path.push(part),
        }
    }
    let outside = || {
        format!(
            "path '{}' leads outside {} through the symlink '{}'",
            escape(rel),
            escape_path(base),
            escape_path(link)
        )
    };
    let place = path.strip_prefix(base).map_err(|_| outside())?;
    let place: Vec<_> = place
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    if place.is_empty() {
        return Err(outside());
    }
    resolve_inside_after(base, &place.join("/"), hops)
}

// ============================================================================
// Tree Walking
// ============================================================================

/// A node in the meta project tree, representing a project and its nested children.
#[derive(Debug, Clone, Serialize)]
pub struct MetaTreeNode {
    pub info: ProjectInfo,
    pub is_meta: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<MetaTreeNode>,
}

/// Walk a meta repository tree, discovering nested .meta.yaml repos.
///
/// Parses the .meta.yaml at `start_dir` and for each project checks
/// if it has its own .meta.yaml file. Recursively expands children up to `max_depth`.
/// Uses cycle detection via path canonicalization.
///
/// `max_depth` of `None` means unlimited recursion.
/// `max_depth` of `Some(0)` means no recursion (only top-level projects).
pub fn walk_meta_tree(
    start_dir: &Path,
    max_depth: Option<usize>,
) -> anyhow::Result<Vec<MetaTreeNode>> {
    let workspace = Workspace::find(start_dir, None)?
        .ok_or_else(|| anyhow::anyhow!("No .meta.yaml found in {}", escape_path(start_dir)))?;
    walk_projects(&workspace.dir, &workspace.projects, max_depth)
}

/// The meta tree whose top level is `projects`, the parsed config in
/// `meta_dir` (such as one chosen with `--config`), walked like
/// [`walk_meta_tree`]. A nested config that fails to parse is an error.
pub fn walk_projects(
    meta_dir: &Path,
    projects: &[ProjectInfo],
    max_depth: Option<usize>,
) -> anyhow::Result<Vec<MetaTreeNode>> {
    let mut visited = std::collections::HashSet::new();
    visited.insert(meta_dir.canonicalize().unwrap_or(meta_dir.to_path_buf()));

    let depth = max_depth.unwrap_or(usize::MAX);
    walk_inner(meta_dir, projects, depth, 0, &mut visited)
}

/// Flatten a meta tree into fully-qualified path strings.
///
/// For nested children, paths are joined with their parent
/// (e.g., a child with path "grandchild" under parent "child" becomes "child/grandchild").
pub fn flatten_meta_tree(nodes: &[MetaTreeNode]) -> Vec<String> {
    let mut paths = Vec::new();
    flatten_inner(nodes, "", &mut paths);
    paths
}

fn flatten_inner(nodes: &[MetaTreeNode], prefix: &str, paths: &mut Vec<String>) {
    for node in nodes {
        let full_path = if prefix.is_empty() {
            node.info.path.clone()
        } else {
            format!("{}/{}", prefix, node.info.path)
        };
        paths.push(full_path.clone());
        flatten_inner(&node.children, &full_path, paths);
    }
}

/// Build a map from full path to (resolved path, ProjectInfo) for nested lookups.
///
/// This is useful for looking up projects by their full nested path
/// (e.g., "vendor/nested-lib" -> resolved filesystem path and project info).
pub fn build_project_map(
    nodes: &[MetaTreeNode],
    base_dir: &Path,
    prefix: &str,
) -> std::collections::HashMap<String, (PathBuf, ProjectInfo)> {
    let mut map = std::collections::HashMap::new();
    for node in nodes {
        let full_path = if prefix.is_empty() {
            node.info.path.clone()
        } else {
            format!("{}/{}", prefix, node.info.path)
        };
        let resolved_path = base_dir.join(&full_path);
        map.insert(full_path.clone(), (resolved_path, node.info.clone()));
        // Recurse into children
        map.extend(build_project_map(&node.children, base_dir, &full_path));
    }
    map
}

// ============================================================================
// Orphan Detection
// ============================================================================

/// Information about an orphaned nested meta repo.
#[derive(Debug, Clone)]
pub struct OrphanWarning {
    /// Path to the current (orphaned) meta directory
    pub current: PathBuf,
    /// Path to the parent meta directory
    pub parent: PathBuf,
    /// Actual parent config path for the inclusion hint.
    pub parent_config: PathBuf,
    /// Suggested key to add to the parent's config
    pub suggested_key: String,
}

/// Find a parent .meta.yaml (if any) above the given meta directory.
///
/// Starts searching from the parent of `meta_dir` and walks up.
/// Returns None if this is the root meta repo (no parent .meta.yaml found).
pub fn find_parent_meta_config(meta_dir: &Path) -> anyhow::Result<Option<PathBuf>> {
    match meta_dir.parent() {
        Some(parent) => find_meta_config(parent, None),
        None => Ok(None),
    }
}

/// Find the root (topmost) meta directory by walking up through parent .meta.yaml files.
///
/// Starting from `meta_dir`, repeatedly calls `find_parent_meta_config` until
/// no more parents are found. Returns the topmost meta directory in the hierarchy.
/// If `meta_dir` is already the root, returns it unchanged.
pub fn find_root_meta_dir(meta_dir: &Path) -> anyhow::Result<PathBuf> {
    let mut current = meta_dir.to_path_buf();
    loop {
        match find_parent_meta_config(&current)? {
            Some(parent_config_path) => {
                current = parent_config_path
                    .parent()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Config path has no parent: {}",
                            escape_path(&parent_config_path)
                        )
                    })?
                    .to_path_buf();
            }
            None => break Ok(current),
        }
    }
}

/// Check if a meta directory is tracked by its parent meta config.
///
/// Returns `Some(OrphanWarning)` if there's a parent .meta.yaml that doesn't include
/// this directory in its project list (directly or transitively).
/// Returns `None` if tracked or if there's no parent meta.
pub fn check_orphan_status(meta_dir: &Path) -> Option<OrphanWarning> {
    let parent_config = find_parent_meta_config(meta_dir).ok()??;
    let parent_meta_dir = parent_config.parent()?;

    // Walk the parent's project tree to see what's tracked
    let tree = walk_meta_tree(parent_meta_dir, None).ok()?;
    let flat_paths = flatten_meta_tree(&tree);

    // Get the relative path from parent to current
    let relative = meta_dir.strip_prefix(parent_meta_dir).ok()?;
    let relative_str = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");

    // Check if this path is in the flattened tree
    if flat_paths.iter().any(|p| p == &*relative_str) {
        None // Tracked, not orphan
    } else {
        // Extract the first path component as the suggested key
        let suggested_key = relative
            .components()
            .next()?
            .as_os_str()
            .to_string_lossy()
            .to_string();

        Some(OrphanWarning {
            current: meta_dir.to_path_buf(),
            parent: parent_meta_dir.to_path_buf(),
            parent_config: parent_config.clone(),
            suggested_key,
        })
    }
}

fn walk_inner(
    base_dir: &Path,
    projects: &[ProjectInfo],
    max_depth: usize,
    current_depth: usize,
    visited: &mut std::collections::HashSet<PathBuf>,
) -> anyhow::Result<Vec<MetaTreeNode>> {
    let mut nodes = Vec::new();

    for project in projects {
        let project_dir = base_dir.join(&project.path);

        // Check if this project has its own .meta.yaml file directly in its directory
        // Past the depth limit it is never read, so a config that can't be
        // examined only stops a walk that would recurse into it
        let nested_config = if current_depth < max_depth {
            find_meta_config_in(&project_dir)?
        } else {
            find_meta_config_in(&project_dir).ok().flatten()
        };
        let has_meta = nested_config.is_some();

        // Recurse into children if within depth limit and this is a meta repo.
        // A nested config that fails to parse or validate is an error, not an
        // empty subtree: skipping it would silently drop its repos.
        let mut children = vec![];
        if let Some(nested_config_path) = nested_config {
            let canonical = project_dir.canonicalize().unwrap_or(project_dir.clone());
            // `visited` stops cycles through symlinks
            if current_depth < max_depth && visited.insert(canonical) {
                let nested = Workspace::load(&nested_config_path)?;
                children = walk_inner(
                    &project_dir,
                    &nested.projects,
                    max_depth,
                    current_depth + 1,
                    visited,
                )?;
            }
        }

        nodes.push(MetaTreeNode {
            info: project.clone(),
            is_meta: has_meta,
            children,
        });
    }

    nodes.sort_by(|a, b| a.info.name.cmp(&b.info.name));
    Ok(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_walk_meta_tree_no_config() {
        let dir = tempfile::tempdir().unwrap();
        let result = walk_meta_tree(dir.path(), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_walk_meta_tree_empty_projects() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();
        let tree = walk_meta_tree(dir.path(), None).unwrap();
        assert!(tree.is_empty());
    }

    #[test]
    fn test_walk_meta_tree_multiple_projects_sorted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("zebra")).unwrap();
        std::fs::create_dir(dir.path().join("alpha")).unwrap();
        std::fs::create_dir(dir.path().join("middle")).unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {
                "zebra": "git@github.com:org/zebra.git",
                "alpha": "git@github.com:org/alpha.git",
                "middle": "git@github.com:org/middle.git"
            }}"#,
        )
        .unwrap();

        let tree = walk_meta_tree(dir.path(), None).unwrap();
        assert_eq!(tree.len(), 3);
        assert_eq!(tree[0].info.name, "alpha");
        assert_eq!(tree[1].info.name, "middle");
        assert_eq!(tree[2].info.name, "zebra");
    }

    #[test]
    fn test_walk_meta_tree_is_meta_flag() {
        let dir = tempfile::tempdir().unwrap();
        let has_meta = dir.path().join("has_meta");
        let no_meta = dir.path().join("no_meta");
        std::fs::create_dir(&has_meta).unwrap();
        std::fs::create_dir(&no_meta).unwrap();
        std::fs::write(has_meta.join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {
                "has_meta": "git@github.com:org/has_meta.git",
                "no_meta": "git@github.com:org/no_meta.git"
            }}"#,
        )
        .unwrap();

        let tree = walk_meta_tree(dir.path(), None).unwrap();
        let has = tree.iter().find(|n| n.info.name == "has_meta").unwrap();
        let no = tree.iter().find(|n| n.info.name == "no_meta").unwrap();
        assert!(has.is_meta);
        assert!(!no.is_meta);
    }

    #[test]
    fn test_walk_meta_tree_depth_zero_no_recursion() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child");
        let grandchild = child.join("grandchild");
        std::fs::create_dir_all(&grandchild).unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"child": "git@github.com:org/child.git"}}"#,
        )
        .unwrap();
        std::fs::write(
            child.join(".meta.yaml"),
            r#"{"projects": {"grandchild": "git@github.com:org/grandchild.git"}}"#,
        )
        .unwrap();

        let tree = walk_meta_tree(dir.path(), Some(0)).unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].info.name, "child");
        assert!(tree[0].is_meta);
        assert!(tree[0].children.is_empty()); // No recursion
    }

    #[test]
    fn test_walk_meta_tree_rejects_symlink_cycles() {
        // A nested project that links back to its own meta dir (or any
        // ancestor) is refused, so the walk cannot loop
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child");
        std::fs::create_dir(&child).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&child, child.join("loop")).unwrap();

        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"child": "git@github.com:org/child.git"}}"#,
        )
        .unwrap();
        std::fs::write(
            child.join(".meta.yaml"),
            r#"{"projects": {"loop": "git@github.com:org/loop.git"}}"#,
        )
        .unwrap();

        #[cfg(unix)]
        {
            let err = walk_meta_tree(dir.path(), None).unwrap_err().to_string();
            assert!(
                err.contains("leads to the directory of the .meta.yaml file itself"),
                "{err}"
            );
        }
    }

    #[test]
    fn test_flatten_meta_tree_empty() {
        let paths = flatten_meta_tree(&[]);
        assert!(paths.is_empty());
    }

    #[test]
    fn test_flatten_meta_tree_deeply_nested() {
        let dir = tempfile::tempdir().unwrap();
        let l1 = dir.path().join("l1");
        let l2 = l1.join("l2");
        let l3 = l2.join("l3");
        std::fs::create_dir_all(&l3).unwrap();

        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"l1": "git@github.com:org/l1.git"}}"#,
        )
        .unwrap();
        std::fs::write(
            l1.join(".meta.yaml"),
            r#"{"projects": {"l2": "git@github.com:org/l2.git"}}"#,
        )
        .unwrap();
        std::fs::write(
            l2.join(".meta.yaml"),
            r#"{"projects": {"l3": "git@github.com:org/l3.git"}}"#,
        )
        .unwrap();

        let tree = walk_meta_tree(dir.path(), None).unwrap();
        let paths = flatten_meta_tree(&tree);

        assert_eq!(paths.len(), 3);
        assert_eq!(paths[0], "l1");
        assert_eq!(paths[1], "l1/l2");
        assert_eq!(paths[2], "l1/l2/l3");
    }

    #[test]
    fn test_walk_meta_tree_nonexistent_project_dir() {
        let dir = tempfile::tempdir().unwrap();
        // Project listed in .meta.yaml but directory doesn't exist
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"missing": "git@github.com:org/missing.git"}}"#,
        )
        .unwrap();

        let tree = walk_meta_tree(dir.path(), None).unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].info.name, "missing");
        assert!(!tree[0].is_meta);
        assert!(tree[0].children.is_empty());
    }

    #[test]
    fn test_walk_meta_tree_extended_format() {
        let dir = tempfile::tempdir().unwrap();
        let custom_path = dir.path().join("custom/path");
        std::fs::create_dir_all(&custom_path).unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {
                "myproject": {
                    "repo": "git@github.com:org/myproject.git",
                    "path": "custom/path",
                    "tags": ["frontend", "react"]
                }
            }}"#,
        )
        .unwrap();

        let tree = walk_meta_tree(dir.path(), None).unwrap();
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].info.name, "myproject");
        assert_eq!(tree[0].info.path, "custom/path");
        assert_eq!(tree[0].info.tags, vec!["frontend", "react"]);
    }

    #[test]
    fn defaults_and_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".meta.yaml");
        let load = |yaml: &str| {
            std::fs::write(&config, yaml).unwrap();
            Workspace::load(&config)
        };
        assert!(load("projects: {}\n").unwrap().defaults.parallel);
        assert!(
            !load("defaults:\n  parallel: false\n")
                .unwrap()
                .defaults
                .parallel
        );
        let hooks = load("worktree:\n  hooks:\n    post-create: make\n").unwrap();
        assert_eq!(
            hooks.worktree.hooks.get("post-create").map(String::as_str),
            Some("make")
        );
        // a misspelled key is an error, at any level meta owns
        for typo in [
            "worktree_dir: x\n",
            // there is no ignore list: --exclude leaves repos out
            "ignore:\n  - node_modules\n",
            "defaults:\n  paralel: false\n",
            "worktree:\n  hook: {}\n",
            "worktree:\n  hooks:\n    post-create: false\n",
            "worktree:\n  hooks:\n    post-create: 123\n",
            "worktree:\n  hooks:\n    post-create:\n",
        ] {
            assert!(load(typo).is_err(), "{typo}");
        }
        // keys other tools put on a project are theirs
        assert!(load("projects:\n  a:\n    repo: x\n    branch: main\n").is_ok());
    }

    // ============================================================================
    // Nested meta repos (meta: true field)
    // ============================================================================

    #[test]
    fn test_parse_nested_meta_yaml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            "projects:\n  core: git@github.com:org/core.git\n  vendor:\n    repo: git@github.com:org/vendor.git\n    meta: true\n",
        )
        .unwrap();

        let projects = Workspace::load(&dir.path().join(".meta.yaml"))
            .unwrap()
            .projects;
        assert_eq!(projects.len(), 2);

        let vendor = projects.iter().find(|p| p.name == "vendor").unwrap();
        assert_eq!(vendor.repo, Some("git@github.com:org/vendor.git".into()));
    }

    #[test]
    fn test_parse_nested_meta_with_custom_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{
                "projects": {
                    "vendor": {
                        "repo": "git@github.com:org/vendor.git",
                        "path": "third_party/vendor",
                        "meta": true
                    }
                }
            }"#,
        )
        .unwrap();

        let projects = Workspace::load(&dir.path().join(".meta.yaml"))
            .unwrap()
            .projects;
        assert_eq!(projects.len(), 1);

        let vendor = &projects[0];
        assert_eq!(vendor.repo, Some("git@github.com:org/vendor.git".into()));
        assert_eq!(vendor.name, "vendor");
        assert_eq!(vendor.path, "third_party/vendor");
    }

    #[test]
    fn test_walk_meta_tree_nested_meta_with_children() {
        let dir = tempfile::tempdir().unwrap();
        let vendor = dir.path().join("vendor");
        let nested = vendor.join("tree-sitter-markdown");
        std::fs::create_dir_all(&nested).unwrap();

        // Root .meta.yaml with nested meta project (has repo URL + meta: true)
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}}}"#,
        )
        .unwrap();

        // Nested .meta.yaml inside vendor (simulates what exists after cloning)
        std::fs::write(
            vendor.join(".meta.yaml"),
            r#"{"projects": {"tree-sitter-markdown": "git@github.com:org/tsm.git"}}"#,
        )
        .unwrap();

        let tree = walk_meta_tree(dir.path(), None).unwrap();
        assert_eq!(tree.len(), 1);

        let vendor_node = &tree[0];
        assert_eq!(vendor_node.info.name, "vendor");
        assert_eq!(
            vendor_node.info.repo,
            Some("git@github.com:org/vendor.git".into())
        );
        assert!(vendor_node.is_meta);
        assert_eq!(vendor_node.children.len(), 1);

        let nested_node = &vendor_node.children[0];
        assert_eq!(nested_node.info.name, "tree-sitter-markdown");
        assert!(nested_node.info.repo.is_some());
    }

    // ============================================================================
    // Orphan detection tests
    // ============================================================================

    #[test]
    fn test_find_parent_meta_config_none_at_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();

        // No parent above this directory
        let result = find_parent_meta_config(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_find_parent_meta_config_finds_parent() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("vendor");
        std::fs::create_dir(&nested).unwrap();

        // Parent .meta.yaml
        std::fs::write(dir.path().join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();
        // Nested .meta.yaml
        std::fs::write(nested.join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();

        let result = find_parent_meta_config(&nested).unwrap();
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(path, dir.path().join(".meta.yaml"));
    }

    #[test]
    fn test_check_orphan_status_not_orphan_when_tracked() {
        let dir = tempfile::tempdir().unwrap();
        let vendor = dir.path().join("vendor");
        std::fs::create_dir(&vendor).unwrap();

        // Parent .meta.yaml tracks vendor with repo URL and meta: true
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}}}"#,
        )
        .unwrap();
        // Nested .meta.yaml (simulates what exists after cloning vendor)
        std::fs::write(vendor.join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();

        let result = check_orphan_status(&vendor);
        assert!(result.is_none(), "vendor should not be orphan when tracked");
    }

    #[test]
    fn test_check_orphan_status_is_orphan_when_not_tracked() {
        let dir = tempfile::tempdir().unwrap();
        let vendor = dir.path().join("vendor");
        std::fs::create_dir(&vendor).unwrap();

        // Parent .meta.yaml does NOT track vendor
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"other": "git@github.com:org/other.git"}}"#,
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("other")).unwrap();
        // Nested .meta.yaml
        std::fs::write(vendor.join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();

        let result = check_orphan_status(&vendor);
        assert!(result.is_some(), "vendor should be orphan when not tracked");

        let warning = result.unwrap();
        assert_eq!(warning.current, vendor);
        assert_eq!(warning.parent, dir.path());
        assert_eq!(warning.suggested_key, "vendor");
    }

    #[test]
    fn test_check_orphan_status_no_parent_means_not_orphan() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();

        // No parent .meta.yaml above this one
        let result = check_orphan_status(dir.path());
        assert!(result.is_none(), "should not be orphan if no parent exists");
    }

    #[test]
    fn test_check_orphan_status_deeply_nested_tracked() {
        // Test 3 levels deep: root -> vendor -> sub-vendor -> deep-lib
        let dir = tempfile::tempdir().unwrap();
        let vendor = dir.path().join("vendor");
        let sub_vendor = vendor.join("sub-vendor");
        let deep_lib = sub_vendor.join("deep-lib");
        std::fs::create_dir_all(&deep_lib).unwrap();

        // Root tracks vendor as nested meta
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}}}"#,
        )
        .unwrap();

        // Vendor tracks sub-vendor as nested meta
        std::fs::write(
            vendor.join(".meta.yaml"),
            r#"{"projects": {"sub-vendor": {"repo": "git@github.com:org/sub-vendor.git", "meta": true}}}"#,
        )
        .unwrap();

        // Sub-vendor tracks deep-lib
        std::fs::write(
            sub_vendor.join(".meta.yaml"),
            r#"{"projects": {"deep-lib": "git@github.com:org/deep-lib.git"}}"#,
        )
        .unwrap();

        // Check from sub-vendor's perspective (should not be orphan - tracked by vendor)
        let result = check_orphan_status(&sub_vendor);
        assert!(
            result.is_none(),
            "sub-vendor should not be orphan when tracked by vendor"
        );
    }

    #[test]
    fn test_check_orphan_status_deeply_nested_orphan() {
        // Test orphan detection at 2 levels deep
        let dir = tempfile::tempdir().unwrap();
        let vendor = dir.path().join("vendor");
        let orphan_dir = vendor.join("orphan-project");
        std::fs::create_dir_all(&orphan_dir).unwrap();
        std::fs::create_dir(dir.path().join("backend")).unwrap();

        // Root tracks vendor and backend
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}, "backend": "git@github.com:org/backend.git"}}"#,
        )
        .unwrap();

        // Vendor does NOT track orphan-project
        std::fs::write(
            vendor.join(".meta.yaml"),
            r#"{"projects": {"lib": "git@github.com:org/lib.git"}}"#,
        )
        .unwrap();
        std::fs::create_dir(vendor.join("lib")).unwrap();

        // Orphan has its own .meta.yaml
        std::fs::write(orphan_dir.join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();

        // Check from orphan's perspective
        let result = check_orphan_status(&orphan_dir);
        assert!(
            result.is_some(),
            "orphan-project should be orphan when not tracked by vendor"
        );

        let warning = result.unwrap();
        assert_eq!(warning.suggested_key, "orphan-project");
        assert_eq!(warning.parent, vendor);
    }

    #[test]
    fn test_walk_meta_tree_fails_on_malformed_nested_meta() {
        // A broken nested .meta.yaml must surface, not silently drop its repos
        let dir = tempfile::tempdir().unwrap();
        let vendor = dir.path().join("vendor");
        std::fs::create_dir(&vendor).unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}}}"#,
        )
        .unwrap();
        std::fs::write(
            vendor.join(".meta.yaml"),
            r#"{"projects": {"x": {"path": "../../escape"}}}"#,
        )
        .unwrap();

        let err = walk_meta_tree(dir.path(), None).unwrap_err().to_string();
        assert!(err.contains("Invalid project 'x'"), "{err}");
        assert!(err.contains("vendor"), "{err}");
    }

    fn parse_err_in(dir: &Path, json: &str) -> String {
        std::fs::write(dir.join(".meta.yaml"), json).unwrap();
        Workspace::load(&dir.join(".meta.yaml"))
            .unwrap_err()
            .to_string()
    }

    fn parse_err(json: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        parse_err_in(dir.path(), json)
    }

    #[test]
    fn test_parse_rejects_paths_outside_the_workspace() {
        for path in [
            "../../dropped",
            "a/../../b",
            "/abs/dir",
            "C:/x",
            "C:x",
            "\\\\host\\s",
            ".",
            "./",
        ] {
            let err = parse_err(&format!(
                r#"{{"projects": {{"p": {{"repo": "u", "path": {path:?}}}}}}}"#
            ));
            assert!(err.contains("Invalid project 'p' in "), "{path}: {err}");
            assert!(err.contains(".meta.yaml: path"), "{path}: {err}");
        }
    }

    #[test]
    fn test_parse_rejects_bad_names_tags_and_repos() {
        for (json, what) in [
            (
                r#"{"projects": {"../../../../escaped-wt": {"repo": "u", "path": "a"}}}"#,
                "name: path '../../../../escaped-wt' must not contain '..'",
            ),
            (
                r#"{"projects": {"/abs": "u"}}"#,
                "name: path '/abs' must be relative",
            ),
            (r#"{"projects": {"a//b": "u"}}"#, "plain relative path"),
            (
                r#"{"projects": {"..": {"repo": "u", "path": "a"}}}"#,
                "must not contain '..'",
            ),
            (
                r#"{"projects": {"x\u001b]0;T\u0007": "u"}}"#,
                "control characters",
            ),
            (
                r#"{"projects": {"a": {"repo": "u", "tags": ["n\nfake"]}}}"#,
                "control characters",
            ),
            (
                r#"{"projects": {"a": {"repo": "u", "tags": ["a,b"]}}}"#,
                "whitespace or ','",
            ),
            (
                r#"{"projects": {"a": {"repo": "u", "tags": [""]}}}"#,
                "tag is empty",
            ),
            (
                r#"{"projects": {"a": "--upload-pack=touch x"}}"#,
                "must not start with '-'",
            ),
            (
                r#"{"projects": {"a": {"repo": "u", "path": "t\u001b[2J"}}}"#,
                "control characters",
            ),
            (
                r#"{"projects": {"a": "https://ghp_SECRET@h/r\n"}}"#,
                "repo 'https://***@h/r\\n' contains control characters",
            ),
        ] {
            let err = parse_err(json);
            assert!(err.contains(what), "{json}: {err}");
            // The offending value is escaped in the message itself, and a
            // URL's credentials aren't in it
            assert!(!err.contains('\u{1b}'), "{json}: {err}");
            assert!(!err.contains("ghp_SECRET"), "{json}: {err}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn an_absolute_link_to_what_is_not_there_yet_stays_inside() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        // the target as a user writes it, without `\\?\`
        let plain = dir.path().join("real");
        // creating links needs a privilege or developer mode
        if std::os::windows::fs::symlink_dir(&plain, base.join("link")).is_err() {
            eprintln!("skipped: this runner can't create symbolic links");
            return;
        }
        assert_eq!(resolve_inside(&base, "link"), Ok(base.join("real")));
        let outside = tempfile::tempdir().unwrap();
        std::os::windows::fs::symlink_dir(outside.path().join("x"), base.join("out")).unwrap();
        let err = resolve_inside(&base, "out").unwrap_err();
        assert!(err.contains("leads outside"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_what_is_not_there_yet_leads_where_it_names() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        std::os::unix::fs::symlink("real", base.join("link")).unwrap();
        assert_eq!(resolve_inside(&base, "link"), Ok(base.join("real")));
        assert_eq!(resolve_inside(&base, "link/x"), Ok(base.join("real/x")));

        std::os::unix::fs::symlink("../elsewhere", base.join("out")).unwrap();
        let err = resolve_inside(&base, "out").unwrap_err();
        assert!(err.contains("leads outside"), "{err}");
        // through a link that does exist, out of the base
        std::os::unix::fs::symlink("/", base.join("root")).unwrap();
        std::os::unix::fs::symlink("root/nowhere-meta-test", base.join("via")).unwrap();
        let err = resolve_inside(&base, "via").unwrap_err();
        assert!(err.contains("leads outside"), "{err}");

        // `..` climbs from where the link before it leads
        std::fs::create_dir_all(base.join("real/base")).unwrap();
        std::os::unix::fs::symlink("real/base", base.join("hop")).unwrap();
        std::os::unix::fs::symlink("hop/../app", base.join("back")).unwrap();
        assert_eq!(resolve_inside(&base, "back"), Ok(base.join("real/app")));

        // `..` out of a directory that isn't there is no way through, and a
        // link after a followed one is still checked
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(base.join("b")).unwrap();
        std::os::unix::fs::symlink(outside.path(), base.join("b/x")).unwrap();
        std::os::unix::fs::symlink("missing/../b", base.join("skip")).unwrap();
        assert!(resolve_inside(&base, "skip/x").is_err());
        std::os::unix::fs::symlink("b", base.join("tob")).unwrap();
        std::os::unix::fs::symlink("nothing/../tob/x/y", base.join("deep")).unwrap();
        assert!(resolve_inside(&base, "deep").is_err());
        std::os::unix::fs::symlink("missing/../self", base.join("self")).unwrap();
        assert!(resolve_inside(&base, "self").is_err());
        // names from the filesystem are escaped in the error
        std::os::unix::fs::symlink(outside.path(), base.join("bad\nFORGED")).unwrap();
        std::os::unix::fs::symlink("bad\nFORGED/missing", base.join("forge")).unwrap();
        let err = resolve_inside(&base, "forge").unwrap_err();
        assert!(!err.contains('\n'), "{err}");
        // a chain of links to missing targets ends
        std::os::unix::fs::symlink("ping/z", base.join("pong")).unwrap();
        std::os::unix::fs::symlink("pong/z", base.join("ping")).unwrap();
        assert!(resolve_inside(&base, "ping").is_err());
    }

    /// Whether a directory with mode 000 still can't be entered here (as
    /// root it can, and permission tests have nothing to check).
    #[cfg(unix)]
    fn locked_dir(path: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
        std::fs::read_dir(path).is_err()
    }

    #[cfg(unix)]
    fn unlock_dir(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_rejects_a_path_through_a_directory_it_cannot_examine() {
        // `locked` may hold a symlink out; not being able to look is no
        // proof that the rest of the path does not exist yet
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        if !locked_dir(&locked) {
            return;
        }
        let err = parse_err_in(
            dir.path(),
            r#"{"projects": {"p": {"repo": "u", "path": "locked/x"}}}"#,
        );
        unlock_dir(&locked);
        assert!(err.contains("cannot be checked at"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn test_walk_fails_on_a_nested_config_it_cannot_read() {
        // group/.meta.yaml links into a directory that can't be entered: the walk
        // must fail rather than leave group's repos out
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("group/core")).unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"group": {"repo": "u", "meta": true}}}"#,
        )
        .unwrap();
        let private = dir.path().join("private");
        std::fs::create_dir(&private).unwrap();
        std::fs::write(
            private.join("config.json"),
            r#"{"projects": {"core": "u"}}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink(
            "../private/config.json",
            dir.path().join("group/.meta.yaml"),
        )
        .unwrap();
        assert!(walk_meta_tree(dir.path(), None).is_ok());
        if !locked_dir(&private) {
            return;
        }
        let result = walk_meta_tree(dir.path(), None);
        unlock_dir(&private);
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Cannot read") && err.contains(".meta.yaml"),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_rejects_overlap_through_symlink_alias() {
        // `alias` points at `real`, so `alias/outer/link/x` sits inside
        // `real/outer`, whose clone could plant `link` pointing anywhere.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("alias")).unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"outer": {"repo": "u", "path": "real/outer"}, "inner": {"repo": "u", "path": "alias/outer/link/x"}}}"#,
        )
        .unwrap();
        let err = Workspace::load(&dir.path().join(".meta.yaml"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(
                "project 'inner' (path 'alias/outer/link/x') is at or inside project 'outer'"
            ),
            "{err}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_rejects_reserved_dirs_through_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::os::unix::fs::symlink(dir.path().join(".git"), dir.path().join("alias")).unwrap();
        let err = parse_err_in(
            dir.path(),
            r#"{"projects": {"p": {"repo": "u", "path": "alias/hooks"}}}"#,
        );
        assert!(err.contains("leads into .git or .worktrees"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_allows_symlink_alias_of_the_workspace_itself() {
        // `here -> .` is harmless as a step on the way; only ending at the
        // config's own directory is refused
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("here")).unwrap();
        let meta = dir.path().join(".meta.yaml");
        std::fs::write(
            &meta,
            r#"{"projects": {"svc": {"repo": "u", "path": "here/service"}}}"#,
        )
        .unwrap();
        assert!(Workspace::load(&meta).is_ok());

        std::fs::write(
            &meta,
            r#"{"projects": {"svc": {"repo": "u", "path": "here"}}}"#,
        )
        .unwrap();
        let err = Workspace::load(&meta).unwrap_err().to_string();
        assert!(
            err.contains("leads to the directory of the .meta.yaml file itself"),
            "{err}"
        );
    }

    #[cfg(unix)] // Windows refuses control characters in file names
    #[test]
    fn test_config_errors_escape_the_path() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws\nfake line\u{1b}[2J");
        std::fs::create_dir(&ws).unwrap();
        std::fs::write(ws.join(".meta.yaml"), "{not json").unwrap();
        let err = format!("{:#}", Workspace::load(&ws.join(".meta.yaml")).unwrap_err());
        assert!(!err.contains('\n') && !err.contains('\u{1b}'), "{err}");
        assert!(err.contains("ws\\nfake line\\u{1b}[2J"), "{err}");
    }

    #[test]
    fn test_parse_rejects_drive_prefixes_anywhere() {
        for json in [
            r#"{"projects": {"p": {"repo": "u", "path": "./C:/outside"}}}"#,
            r#"{"projects": {"p": {"repo": "u", "path": "a/C:b"}}}"#,
            r#"{"projects": {"C:outside": {"repo": "u", "path": "safe"}}}"#,
        ] {
            let err = parse_err(json);
            assert!(err.contains("ASCII letters"), "{json}: {err}");
        }
    }

    #[test]
    fn test_parse_rejects_names_and_paths_that_alias_or_mislead() {
        for (json, what) in [
            (
                r#"{"projects": {"p": {"repo": "u", "path": "a./x"}}}"#,
                "trailing dot",
            ),
            (
                r#"{"projects": {"p": {"repo": "u", "path": "a /x"}}}"#,
                "trailing dot or space",
            ),
            (
                r#"{"projects": {"p": {"repo": "u", "path": "x/nul"}}}"#,
                "device name",
            ),
            (
                r#"{"projects": {"p": {"repo": "u", "path": "con.txt"}}}"#,
                "device name",
            ),
            (
                r#"{"projects": {"p": {"repo": "u", "path": "x/com1"}}}"#,
                "device name",
            ),
            (
                r#"{"projects": {"p": {"repo": "u", "path": "C:x"}}}"#,
                "ASCII letters",
            ),
            (
                r#"{"projects": {"p": {"repo": "u", "path": "ab:c"}}}"#,
                "ASCII letters",
            ),
            (
                r#"{"projects": {"p": {"repo": "u", "path": "PROGRA~1/x"}}}"#,
                "ASCII letters",
            ),
            (
                r#"{"projects": {"p": {"repo": "u", "path": "caf\u00e9"}}}"#,
                "ASCII letters",
            ),
            (
                r#"{"projects": {"p": {"repo": "u", "path": "cafe\u0301"}}}"#,
                "ASCII letters",
            ),
            (
                r#"{"projects": {"p": {"repo": "u", "path": "COM\u00b9"}}}"#,
                "ASCII letters",
            ),
            (
                r#"{"projects": {"p": {"repo": "u", "path": ".git/hooks"}}}"#,
                "the name .git",
            ),
            (
                r#"{"projects": {"p": {"repo": "u", "path": "x/.GIT/hooks"}}}"#,
                "the name .git",
            ),
            (
                r#"{"projects": {"p": {"repo": "u", "path": ".worktrees/t"}}}"#,
                "inside .worktrees",
            ),
            (
                r#"{"projects": {"p": {"repo": "u", "path": "sets/x"}}, "worktrees_dir": "sets"}"#,
                "inside sets",
            ),
            (
                r#"{"projects": {".worktrees/b": {"repo": "u", "path": "b"}}, "worktrees_dir": "sets"}"#,
                "inside sets",
            ),
            (r#"{"projects": {"x|y": "u"}}"#, "ASCII letters"),
            (
                r#"{"projects": {"a.": {"repo": "u", "path": "b"}}}"#,
                "trailing dot",
            ),
        ] {
            let err = parse_err(json);
            assert!(err.contains(what), "{json}: {err}");
        }
        // Ordinary names that only look similar are fine
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"console": "u", "a.b": "u", "com10": "u", "x y+z@w": "u", ".github": "u"}}"#,
        )
        .unwrap();
        assert!(Workspace::load(&dir.path().join(".meta.yaml")).is_ok());
    }

    #[test]
    fn test_parse_rejects_overlapping_paths() {
        let err = parse_err(
            r#"{"projects": {"outer": {"repo": "u", "path": "a"}, "inner": {"repo": "u", "path": "a/link/x"}}}"#,
        );
        assert!(
            err.contains("project 'inner' (path 'a/link/x') is at or inside project 'outer'"),
            "{err}"
        );

        let err = parse_err(
            r#"{"projects": {"one": {"repo": "u", "path": "x"}, "two": {"repo": "u", "path": "./x/"}}}"#,
        );
        assert!(
            err.contains("project 'two' (path 'x') is at or inside project 'one'"),
            "{err}"
        );

        // Case differences don't separate paths (some filesystems ignore case)
        let err = parse_err(
            r#"{"projects": {"one": {"repo": "u", "path": "Lib"}, "two": {"repo": "u", "path": "lib/x"}}}"#,
        );
        assert!(err.contains("is at or inside project 'one'"), "{err}");

        // A shared name prefix is not nesting
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"a": "u", "a-b": "u", "ab": {"repo": "u", "path": "a.b/c"}}}"#,
        )
        .unwrap();
        assert!(Workspace::load(&dir.path().join(".meta.yaml")).is_ok());
    }

    #[test]
    fn test_parse_accepts_nested_alias_names() {
        // as gitkb's .meta.yaml lists `vendor/tree-sitter-markdown`
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            "projects:\n  core: u\n  vendor/tree-sitter-markdown: u\n",
        )
        .unwrap();
        let projects = Workspace::load(&dir.path().join(".meta.yaml"))
            .unwrap()
            .projects;
        assert_eq!(projects[1].name, "vendor/tree-sitter-markdown");
        assert_eq!(projects[1].path, "vendor/tree-sitter-markdown");
    }

    #[test]
    fn test_relative_dir_stays_inside() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(relative_dir(dir.path(), "wt").unwrap(), "wt");
        let inside = dir.path().join("custom").to_string_lossy().into_owned();
        assert_eq!(relative_dir(dir.path(), &inside).unwrap(), "custom");
        let deeper = dir
            .path()
            .join("build/worktrees")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            relative_dir(dir.path(), &deeper).unwrap(),
            "build/worktrees"
        );
        assert!(relative_dir(dir.path(), "../out").is_err());
        #[cfg(unix)]
        {
            std::fs::create_dir(dir.path().join(".git")).unwrap();
            std::os::unix::fs::symlink(dir.path().join(".git"), dir.path().join("storage"))
                .unwrap();
            assert!(relative_dir(dir.path(), "storage")
                .unwrap_err()
                .contains("leads into .git"));
        }
        // a value from the config is escaped in the message
        let err = relative_dir(dir.path(), "../bad\nFORGED").unwrap_err();
        assert!(!err.contains('\n') && err.contains("bad\\nFORGED"), "{err}");
        assert!(relative_dir(dir.path(), "/").is_err());
    }

    #[test]
    fn test_parse_rejects_nesting_or_colliding_names() {
        let err = parse_err(
            r#"{"projects": {"a": {"repo": "u", "path": "x"}, "a/b": {"repo": "u", "path": "y"}}}"#,
        );
        assert!(
            err.contains("project name 'a/b' is the same as or inside 'a'"),
            "{err}"
        );
        let err = parse_err(
            r#"{"projects": {"API": {"repo": "u", "path": "x"}, "api": {"repo": "u", "path": "y"}}}"#,
        );
        assert!(err.contains("is the same as or inside"), "{err}");
        let err = parse_err(
            r#"{"projects": {"app": {"repo": "u", "path": "app"}}, "worktrees_dir": "app/wt"}"#,
        );
        assert!(err.contains("is inside project 'app'"), "{err}");
        let err = parse_err(r#"{"projects": {}, "worktrees_dir": "../out"}"#);
        assert!(err.contains("Invalid worktrees_dir"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_rejects_projects_where_the_worktree_root_resolves() {
        // .worktrees is a symlink to storage/, so storage/review is in it
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("storage")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("storage"), dir.path().join(".worktrees"))
            .unwrap();
        let err = parse_err_in(
            dir.path(),
            r#"{"projects": {"app": {"repo": "u", "path": "storage/review"}}}"#,
        );
        assert!(err.contains("through a symlink"), "{err}");
    }

    #[test]
    fn test_parse_normalizes_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"a": {"repo": "u", "path": "./third_party//vendor/"}, "b": {"repo": "u", "path": "x\\y"}}}"#,
        )
        .unwrap();
        let projects = Workspace::load(&dir.path().join(".meta.yaml"))
            .unwrap()
            .projects;
        assert_eq!(projects[0].path, "third_party/vendor");
        assert_eq!(projects[1].path, "x/y");
    }

    #[cfg(unix)]
    #[test]
    fn test_parse_rejects_symlink_escape() {
        let outside = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("inside")).unwrap();
        let meta = dir.path().join(".meta.yaml");

        for path in ["link", "link/sub"] {
            std::fs::write(
                &meta,
                format!(r#"{{"projects": {{"p": {{"repo": "u", "path": "{path}"}}}}}}"#),
            )
            .unwrap();
            let err = Workspace::load(&meta).unwrap_err().to_string();
            assert!(err.contains("through the symlink"), "{path}: {err}");
        }

        // A symlink that stays inside the workspace is fine
        std::fs::write(
            &meta,
            r#"{"projects": {"p": {"repo": "u", "path": "inside/sub"}}}"#,
        )
        .unwrap();
        assert!(Workspace::load(&meta).is_ok());
    }

    #[test]
    fn test_build_project_map_empty_tree() {
        let dir = tempfile::tempdir().unwrap();
        let tree: Vec<MetaTreeNode> = vec![];
        let map = build_project_map(&tree, dir.path(), "");
        assert!(map.is_empty());
    }

    #[test]
    fn test_build_project_map_with_custom_path() {
        // Test that custom paths are handled correctly in the map
        let dir = tempfile::tempdir().unwrap();
        let custom_path = dir.path().join("custom/path/to/project");
        std::fs::create_dir_all(&custom_path).unwrap();

        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"my-project": {"repo": "git@github.com:org/my-project.git", "path": "custom/path/to/project"}}}"#,
        )
        .unwrap();

        let tree = walk_meta_tree(dir.path(), None).unwrap();
        let map = build_project_map(&tree, dir.path(), "");

        // The key should be the path, not the name
        assert!(map.contains_key("custom/path/to/project"));
        let (path, info) = map.get("custom/path/to/project").unwrap();
        assert_eq!(info.name, "my-project");
        assert_eq!(*path, custom_path);
    }

    // ============================================================================
    // find_root_meta_dir tests
    // ============================================================================

    #[test]
    fn test_find_root_meta_dir_no_parent() {
        // Single-level meta dir (no parent) → returns self
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();

        let root = find_root_meta_dir(dir.path()).unwrap();
        assert_eq!(root, dir.path());
    }

    #[test]
    fn test_find_root_meta_dir_two_levels() {
        // root → child: from child, should return root
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child");
        std::fs::create_dir(&child).unwrap();

        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"child": {"repo": "git@github.com:org/child.git", "meta": true}}}"#,
        )
        .unwrap();
        std::fs::write(child.join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();

        let root = find_root_meta_dir(&child).unwrap();
        assert_eq!(root, dir.path());
    }

    #[test]
    fn test_find_root_meta_dir_three_levels() {
        // root → child → grandchild: from grandchild, should return root
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child");
        let grandchild = child.join("grandchild");
        std::fs::create_dir_all(&grandchild).unwrap();

        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"child": {"repo": "git@github.com:org/child.git", "meta": true}}}"#,
        )
        .unwrap();
        std::fs::write(
            child.join(".meta.yaml"),
            r#"{"projects": {"grandchild": {"repo": "git@github.com:org/gc.git", "meta": true}}}"#,
        )
        .unwrap();
        std::fs::write(grandchild.join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();

        let root = find_root_meta_dir(&grandchild).unwrap();
        assert_eq!(root, dir.path());
    }

    // ============================================================================
    // A `.meta/` directory (meta's local folder) is never config
    // ============================================================================

    fn meta_yaml(dir: &Path, projects: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join(".meta.yaml"),
            format!(r#"{{"projects": {{{projects}}}}}"#),
        )
        .unwrap();
    }

    /// A repository's git directory at `repo/.git`, as much of one as meta
    /// reads: git calls a directory without these no repository.
    fn git_dir(repo: &Path) {
        let git = repo.join(".git");
        for dir in ["objects", "refs"] {
            std::fs::create_dir_all(git.join(dir)).unwrap();
        }
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    }

    #[test]
    fn the_root_climbs_only_through_parents_that_list_the_child() {
        let tmp = tempfile::tempdir().unwrap();
        let top = tmp.path().canonicalize().unwrap();
        meta_yaml(&top, r#""inner": {"repo": "x", "meta": true}"#);
        meta_yaml(&top.join("inner"), r#""deep": {"repo": "x", "meta": true}"#);
        meta_yaml(&top.join("inner/deep"), "");
        assert_eq!(workspace_root(&top.join("inner/deep")), top);
        assert_eq!(workspace_root(&top.join("inner")), top);

        // A workspace inside another that doesn't list it is its own
        let unlisted = top.join("inner/elsewhere");
        meta_yaml(&unlisted, "");
        assert_eq!(workspace_root(&unlisted), unlisted);
        // and so is one under a directory without a config
        let below = top.join("plain/sub");
        meta_yaml(&below, "");
        assert_eq!(workspace_root(&below), below);
    }

    #[test]
    fn a_worktree_meta_made_belongs_to_its_sources_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let top = tmp.path().canonicalize().unwrap();
        meta_yaml(&top, r#""a": "x""#);
        // A worktree set's root checkout, as git lays it out
        let admin = top.join(".git/worktrees/s1");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        let set = top.join(".worktrees/s1");
        meta_yaml(&set, r#""a": "x""#);
        std::fs::write(set.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        // Made by hand: a workspace of its own
        assert_eq!(workspace_meta_dirs(&set), vec![set.clone()]);
        // Made by meta
        let record = serde_json::json!({ "project": top, "set": set });
        std::fs::write(admin.join(WORKTREE_RECORD_FILE), record.to_string()).unwrap();
        assert_eq!(workspace_meta_dirs(&set), vec![set.clone(), top.clone()]);
    }

    #[test]
    fn a_nested_checkout_in_a_set_climbs_the_set_then_its_source() {
        let tmp = tempfile::tempdir().unwrap();
        let top = tmp.path().canonicalize().unwrap();
        let listing_child = r#""child": {"repo": "x", "meta": true}"#;
        meta_yaml(&top, listing_child);
        meta_yaml(&top.join("child"), "");
        // The set's root and nested checkouts, each with meta's record
        let set = top.join(".worktrees/ctx");
        meta_yaml(&set, listing_child);
        meta_yaml(&set.join("child"), "");
        let record = serde_json::json!({ "project": top, "set": set }).to_string();
        for (checkout, repo) in [(&set, top.clone()), (&set.join("child"), top.join("child"))] {
            git_dir(&repo);
            let admin = repo.join(".git/worktrees/ctx");
            std::fs::create_dir_all(&admin).unwrap();
            std::fs::write(admin.join("commondir"), "../..\n").unwrap();
            std::fs::write(admin.join(WORKTREE_RECORD_FILE), &record).unwrap();
            std::fs::write(
                checkout.join(".git"),
                format!("gitdir: {}\n", admin.display()),
            )
            .unwrap();
        }
        // The set's own meta dirs, then the checkout it stands for and up
        assert_eq!(
            workspace_meta_dirs(&set.join("child")),
            vec![
                set.join("child"),
                set.clone(),
                top.join("child"),
                top.clone()
            ]
        );
    }

    #[test]
    fn a_set_checkout_stands_for_the_repo_at_its_place() {
        // alpha lives at beta, and beta at gamma: a checkout at <set>/beta is
        // alpha's, not beta's
        let tmp = tempfile::tempdir().unwrap();
        let top = tmp.path().canonicalize().unwrap();
        meta_yaml(
            &top,
            r#""alpha": {"repo": "x", "path": "beta"}, "beta": {"repo": "x", "path": "gamma"}"#,
        );
        let set = top.join(".worktrees/s");
        let member = set.join("beta");
        std::fs::create_dir_all(&member).unwrap();
        git_dir(&top.join("beta"));
        let admin = top.join("beta/.git/worktrees/s");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        let record = serde_json::json!({ "project": top, "set": set }).to_string();
        std::fs::write(admin.join(WORKTREE_RECORD_FILE), record).unwrap();
        std::fs::write(
            member.join(".git"),
            format!("gitdir: {}\n", admin.display()),
        )
        .unwrap();
        assert_eq!(set_source(&member), Some(top.join("beta")));
    }

    #[test]
    fn a_set_checkout_a_branch_link_put_elsewhere_stands_for_its_own_repo() {
        // app is at `link`; the root's branch in the set links it to
        // `alternate`, which in the workspace is another repository
        let tmp = tempfile::tempdir().unwrap();
        let top = tmp.path().canonicalize().unwrap();
        meta_yaml(
            &top,
            r#""app": {"repo": "x", "path": "real"}, "alt": {"repo": "x", "path": "alternate"}"#,
        );
        let set = top.join(".worktrees/s");
        let member = set.join("alternate");
        std::fs::create_dir_all(&member).unwrap();
        git_dir(&top.join("alternate"));
        git_dir(&top.join("real"));
        let admin = top.join("real/.git/worktrees/s");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        let record = serde_json::json!({ "project": top, "set": set }).to_string();
        std::fs::write(admin.join(WORKTREE_RECORD_FILE), record).unwrap();
        std::fs::write(
            member.join(".git"),
            format!("gitdir: {}\n", admin.display()),
        )
        .unwrap();
        assert_eq!(set_source(&member), Some(top.join("real")));

        // so does a place that holds no repository, or nothing, in the workspace
        std::fs::remove_dir_all(top.join("alternate/.git")).unwrap();
        assert_eq!(set_source(&member), Some(top.join("real")));
        std::fs::remove_dir_all(top.join("alternate")).unwrap();
        assert_eq!(set_source(&member), Some(top.join("real")));

        // a broken config of another nested repo doesn't hide it
        meta_yaml(
            &top,
            r#""app": {"repo": "x", "path": "real"}, "alt": {"repo": "x", "path": "alternate"}, "other": {"repo": "x", "meta": true}"#,
        );
        std::fs::create_dir_all(top.join("other")).unwrap();
        std::fs::write(top.join("other/.meta.yaml"), "{broken").unwrap();
        assert_eq!(set_source(&member), Some(top.join("real")));

        // a repository that is no project of the workspace stands for nothing
        meta_yaml(&top, r#""alt": {"repo": "x", "path": "alternate"}"#);
        assert_eq!(set_source(&member), None);
    }

    #[test]
    fn a_second_checkout_of_a_projects_repo_is_not_its_source() {
        // app is at `real`; `alternate` is a hand-made `git worktree add` of
        // the same repository, no project; a branch link put the set's
        // checkout of app at `<set>/alternate`
        let tmp = tempfile::tempdir().unwrap();
        let top = tmp.path().canonicalize().unwrap();
        meta_yaml(&top, r#""app": {"repo": "x", "path": "real"}"#);
        git_dir(&top.join("real"));
        let linked = |checkout: &Path, id: &str| {
            let admin = top.join("real/.git/worktrees").join(id);
            std::fs::create_dir_all(&admin).unwrap();
            std::fs::write(admin.join("commondir"), "../..\n").unwrap();
            std::fs::create_dir_all(checkout).unwrap();
            std::fs::write(
                checkout.join(".git"),
                format!("gitdir: {}\n", admin.display()),
            )
            .unwrap();
            admin
        };
        linked(&top.join("alternate"), "alternate");
        let set = top.join(".worktrees/s");
        let member = set.join("alternate");
        let admin = linked(&member, "s");
        let record = serde_json::json!({ "project": top, "set": set }).to_string();
        std::fs::write(admin.join(WORKTREE_RECORD_FILE), record).unwrap();
        assert_eq!(set_source(&member), Some(top.join("real")));
    }

    #[test]
    fn a_separate_git_dir_workspace_is_its_set_roots_source() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let (top, git) = (base.join("ws"), base.join("git"));
        meta_yaml(&top, "");
        std::fs::write(top.join(".git"), format!("gitdir: {}\n", git.display())).unwrap();
        let set = top.join(".worktrees/s");
        std::fs::create_dir_all(&set).unwrap();
        let admin = git.join("worktrees/s");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        let record = serde_json::json!({ "project": top, "set": set }).to_string();
        std::fs::write(admin.join(WORKTREE_RECORD_FILE), record).unwrap();
        std::fs::write(set.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        assert_eq!(set_source(&set), Some(top));
    }

    #[test]
    fn a_broken_sibling_config_does_not_split_the_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let top = tmp.path().canonicalize().unwrap();
        meta_yaml(
            &top,
            r#""inner": {"repo": "x", "meta": true}, "other": {"repo": "x", "meta": true}"#,
        );
        meta_yaml(&top.join("inner"), "");
        std::fs::create_dir_all(top.join("other")).unwrap();
        std::fs::write(top.join("other/.meta.yaml"), "{not json").unwrap();
        assert_eq!(
            workspace_meta_dirs(&top.join("inner")),
            vec![top.join("inner"), top.clone()]
        );
    }

    #[test]
    fn nearest_meta_dir_uses_nearest_config_not_bare_meta_directory() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("inner");
        let cwd = inner.join("child");
        std::fs::create_dir_all(cwd.join(".meta/plugins")).unwrap();
        std::fs::write(dir.path().join(".meta.yaml"), "projects: {}").unwrap();
        assert_eq!(
            nearest_meta_dir(&cwd).unwrap(),
            Some(dir.path().to_path_buf())
        );
        std::fs::write(inner.join(".meta.yaml"), "{}").unwrap();
        assert_eq!(nearest_meta_dir(&cwd).unwrap(), Some(inner));
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_config_is_an_error_in_both_discovery_paths() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".meta.yaml");
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&path).is_err() {
            assert!(find_meta_config_in(dir.path()).is_err());
            assert!(find_meta_config(dir.path(), None).is_err());
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn any_config_file_is_read_as_yaml() {
        // whatever its name, as `--config alt.json` reads it
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("alt.json");
        std::fs::write(&path, "projects:\n  app: git@x:o/app.git\n").unwrap();
        let projects = Workspace::load(&path).unwrap().projects;
        assert_eq!(projects[0].name, "app");
    }

    #[test]
    fn test_meta_directory_is_not_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".meta/skills")).unwrap();
        assert!(find_meta_config_in(dir.path()).unwrap().is_none());

        std::fs::write(dir.path().join(".meta.yaml"), "projects: {}\n").unwrap();
        let expected = dir.path().join(".meta.yaml");
        assert_eq!(find_meta_config_in(dir.path()).unwrap().unwrap(), expected);
        assert_eq!(
            find_meta_config(dir.path(), None).unwrap().unwrap(),
            expected
        );
    }

    #[test]
    fn test_find_meta_config_walks_past_meta_directory() {
        let dir = tempfile::tempdir().unwrap();
        let child = dir.path().join("child");
        std::fs::create_dir_all(child.join(".meta")).unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"child": "x"}}"#,
        )
        .unwrap();

        let path = find_meta_config(&child, None).unwrap().unwrap();
        assert_eq!(path, dir.path().join(".meta.yaml"));

        let tree = walk_meta_tree(dir.path(), None).unwrap();
        assert!(!tree[0].is_meta);
    }
}

#[cfg(test)]
mod duplicate_project_tests {
    #[test]
    fn duplicate_project_keys_name_file_and_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".meta.yaml");
        for content in [
            "projects:\n  dup: first\n  dup: second\n",
            r#"{"projects":{"dup":"first","dup":"second"}}"#,
        ] {
            std::fs::write(&path, content).unwrap();
            let err = format!("{:#}", super::Workspace::load(&path).unwrap_err());
            assert!(err.contains(".meta.yaml"), "{err}");
            assert!(err.contains("duplicate project key 'dup'"), "{err}");
        }
    }
}
