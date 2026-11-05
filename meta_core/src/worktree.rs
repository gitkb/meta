//! Worktree context detection and discovery for meta.
//!
//! Provides filesystem-based detection of worktree contexts and
//! discovery of repos within a worktree set. Used by the meta binary for
//! automatic worktree-scoped command dispatch, and by meta_git_cli (worktree)
//! for worktree management commands.

use anyhow::{Context, Result};
use meta_plugin::display::{escape, escape_lines, escape_path};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::gitdir;

/// Discovered information about a repo within a worktree set.
#[derive(Debug, Clone, Serialize)]
pub struct WorktreeRepoInfo {
    pub alias: String,
    pub branch: String,
    pub path: PathBuf,
    pub source_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_branch: Option<bool>,
}

/// The worktree set `cwd` is in: its name and folder, or `None`.
///
/// A set is a folder meta knows by its place, whatever made it and whatever
/// is in it: the nearest folder at or above `cwd` with an owner file beside
/// it ([`meta_plugin::sets::owner`]), else the `<name>` folder after a
/// `.worktrees` part of the path (a set made before owner files, or by
/// hand). Membership is checked by the caller ([`set_members`]).
pub fn detect_worktree_location(cwd: &Path) -> Result<Option<(String, PathBuf)>> {
    use std::path::Component;
    // the path as typed, then as it really is (a set reached through a link)
    let real = cwd.canonicalize().ok();
    for path in std::iter::once(cwd).chain(real.as_deref()) {
        for dir in path.ancestors() {
            let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if meta_plugin::sets::owner(dir)?.is_some() {
                return Ok(Some((name.to_string(), dir.to_path_buf())));
            }
        }
    }
    let components: Vec<_> = cwd.components().collect();
    for (i, comp) in components.iter().enumerate() {
        if let Component::Normal(name) = comp {
            if name.to_str() == Some(".worktrees") {
                let task_name = match components.get(i + 1) {
                    Some(Component::Normal(n)) => match n.to_str() {
                        Some(n) => n.to_string(),
                        None => return Ok(None),
                    },
                    _ => return Ok(None),
                };
                let task_dir: PathBuf = components.iter().take(i + 2).collect();
                return Ok(Some((task_name, task_dir)));
            }
        }
    }
    Ok(None)
}

/// The workspace the set at `set_dir` was made from, as its owner file says.
fn set_origin(set_dir: &Path) -> Result<Option<PathBuf>> {
    Ok(meta_plugin::sets::owner(set_dir)?.map(|owner| owner.workspace))
}

/// The config that names the members of the set at `set_dir`: the set's own
/// (its root checkout's), else the config of the workspace its owner file
/// says it was made from, else (a set without one) the nearest above it.
pub fn set_config(set_dir: &Path, config_name: Option<&PathBuf>) -> Result<Option<PathBuf>> {
    use crate::config;
    if let Some(found) = config::meta_config_at(set_dir, config_name)? {
        return Ok(Some(found));
    }
    match set_origin(set_dir)? {
        Some(source) => config::meta_config_at(&source, config_name),
        None => config::find_meta_config(set_dir, config_name),
    }
}

/// Where `dir`, inside a worktree set, is in the workspace the set was made
/// from (and from a set made inside another set, on to that one's
/// workspace), by the sets' owner files. None outside a set, and for a set
/// without an owner file.
pub fn primary_dir(dir: &Path) -> Result<Option<PathBuf>> {
    let mut place = dir.to_path_buf();
    let mut seen = Vec::new();
    while let Some((_, set_dir)) = detect_worktree_location(&place)? {
        let origin = set_origin(&set_dir).map_err(|e| {
            anyhow::anyhow!(
                "--primary can't tell which workspace worktree set {} was made from: {e:#}. \
                 Run meta from that workspace's directory instead.",
                escape_path(&set_dir)
            )
        })?;
        let Some(origin) = origin else {
            break;
        };
        let inside = place.strip_prefix(&set_dir).unwrap_or(Path::new(""));
        place = origin.join(inside);
        // owner files that lead in a circle lead nowhere
        if seen.contains(&place) {
            break;
        }
        seen.push(place.clone());
    }
    Ok((!seen.is_empty()).then_some(place))
}

/// The members of the set at `set_dir` that `filters` select (see
/// [`crate::repo_filter::Scope::select`]): the projects of `workspace` (the
/// set's config, [`set_config`]), nested meta repos' included, whose folder
/// is in the set at the project's place, and the set's root when it holds
/// the workspace's config. What a folder holds (a git checkout, a jj
/// workspace) is its provider's business: meta looks for the folder only.
/// A checkout is named and tagged by the project at its place, as the set's
/// config and the nested configs tell (see [`set_tree`]).
pub fn set_members(
    set_dir: &Path,
    workspace: Option<&crate::config::Workspace>,
    filters: &crate::repo_filter::Filters,
) -> Result<Vec<crate::repo_filter::Candidate>> {
    use crate::repo_filter::{Candidate, Scope};
    let Some(workspace) = workspace else {
        if !filters.tags.is_empty() {
            anyhow::bail!(
                "No meta config found for worktree set {}, so --tag cannot select its repos",
                escape_path(set_dir)
            );
        }
        return Ok(Vec::new());
    };
    // where each project's checkout is in the set (an outside project's at
    // its name)
    let projects: Vec<_> = workspace
        .projects
        .iter()
        .map(|project| crate::config::ProjectInfo {
            path: project.set_place().to_string(),
            ..project.clone()
        })
        .collect();
    // without a filter, a nested config that can't be read only costs its
    // projects: the command runs in the others, as in the workspace
    let tree = match set_tree(set_dir, &projects) {
        Ok(tree) => tree,
        Err(_) if filters.is_empty() => projects
            .iter()
            .map(|info| crate::config::MetaTreeNode {
                info: info.clone(),
                is_meta: false,
                error: None,
                children: Vec::new(),
            })
            .collect(),
        Err(e) => return Err(e),
    };
    let mut places: Vec<(String, crate::config::ProjectInfo)> =
        crate::config::build_project_map(&tree, set_dir, "")
            .into_iter()
            .map(|(place, (_, info))| (place, info))
            .collect();
    places.sort_by(|a, b| a.0.cmp(&b.0));
    let tags = places
        .iter()
        .flat_map(|(_, info)| info.tags.clone())
        .collect();
    let config_name = workspace.config_path.file_name();
    let mut repos: Vec<Candidate> = config_name
        .filter(|name| set_dir.join(name).is_file())
        .map(|_| Candidate::root(set_dir.to_path_buf()))
        .into_iter()
        .collect();
    let held: Vec<PathBuf> = places
        .iter()
        .map(|(place, _)| set_dir.join(place))
        .filter(|dir| dir.is_dir())
        .collect();
    repos.extend(
        places
            .iter()
            .filter(|(place, _)| {
                let dir = set_dir.join(place);
                held.contains(&dir) && !only_leads_on(&dir, &held)
            })
            .map(|(place, info)| Candidate::project(set_dir.join(place), info)),
    );
    Scope {
        root: set_dir.to_path_buf(),
        config: Some(workspace.config_path.clone()),
        repos,
        // a tag of a project the set doesn't hold is no typo
        tags,
        nested: true,
    }
    .select_repos(filters)
}

/// Whether the folder `dir` holds nothing but the way to other members'
/// folders (`held`): the folder a nested project's checkout sits in, not a
/// checkout itself. A folder that can't be read counts as a member.
fn only_leads_on(dir: &Path, held: &[PathBuf]) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let mut any = false;
    for entry in entries {
        let Ok(entry) = entry else {
            return false;
        };
        let path = entry.path();
        let real_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        let leads = real_dir
            && held
                .iter()
                .any(|member| member != dir && member.starts_with(&path));
        if !leads {
            return false;
        }
        any = true;
    }
    any
}

/// The meta tree that describes the checkouts of the set at `set_dir`, whose
/// config lists `projects`: the tree of the workspace the set was made from
/// (its owner file says which; what `create` planned from, the projects its
/// config on disk lists, which the set's checked-out config may not), for
/// what the set's own configs don't hold, then the set's own, so a map built
/// from it in order lets the set's win. The workspace's part is best effort:
/// what neither tells about stays unselected.
fn set_tree(
    set_dir: &Path,
    projects: &[crate::config::ProjectInfo],
) -> Result<Vec<crate::config::MetaTreeNode>> {
    use crate::config;
    let own = config::walk_projects(set_dir, projects, None)?;
    let mut tree: Vec<_> = set_origin(set_dir)?
        .map(|dir| {
            // where each project of the workspace sits in the set
            let origin: Vec<config::ProjectInfo> = config::Workspace::in_dir(&dir)
                .ok()
                .flatten()
                .map(|w| {
                    w.projects
                        .iter()
                        .map(|project| config::ProjectInfo {
                            path: project.set_place().to_string(),
                            ..project.clone()
                        })
                        .collect()
                })
                .unwrap_or_else(|| projects.to_vec());
            let mut visited = std::collections::HashSet::new();
            readable_tree(&dir, set_dir, &origin, &mut visited)
        })
        .unwrap_or_default();
    tree.extend(own);
    Ok(tree)
}

/// The meta tree of `projects` listed in `dir` (the workspace a set was made
/// from), each nested meta repo's projects read from its checkout in the
/// set at `set` when that holds its config, else from the workspace's,
/// passing over each nested config that can't be read: one broken config
/// loses only its own projects.
fn readable_tree(
    dir: &Path,
    set: &Path,
    projects: &[crate::config::ProjectInfo],
    visited: &mut std::collections::HashSet<PathBuf>,
) -> Vec<crate::config::MetaTreeNode> {
    use crate::config;
    projects
        .iter()
        .map(|project| {
            let (path, place) = (dir.join(&project.path), set.join(&project.path));
            let nested = match config::find_meta_config_in(&place).ok().flatten() {
                Some(own) => Some(own),
                None => config::find_meta_config_in(&path).ok().flatten(),
            };
            // each meta repo once, as `walk_projects` does: links can loop
            let children = nested
                .as_ref()
                .filter(|_| visited.insert(path.canonicalize().unwrap_or_else(|_| path.clone())))
                .and_then(|config| config::Workspace::load(config).ok())
                .map(|nested| readable_tree(&path, &place, &nested.projects, visited))
                .unwrap_or_default();
            config::MetaTreeNode {
                info: project.clone(),
                is_meta: nested.is_some(),
                error: None,
                children,
            }
        })
        .collect()
}

/// Whether a checkout meta made for a set (a linked worktree with meta's
/// record, readable or not, or one whose git directory can't be found) may
/// be under `dir`, as in a set moved by hand; a part of `dir` that can't be
/// read may hold one too. Other tools' worktrees
/// have no record; plain clones (a `.git` directory, or a link to one) are
/// not looked inside.
pub fn holds_meta_checkout(dir: &Path) -> bool {
    let canonical_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    // Where discovery looks: not in `.git`, nor in the sets of a workspace
    // that is itself under `.worktrees` (its own nested `.worktrees`)
    let mut walk = walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || ScannedDirs::searched(entry.path()));
    while let Some(entry) = walk.next() {
        // What can't be read may hold one: can't tell is no plain clone
        let Ok(entry) = entry else { return true };
        if !entry.file_type().is_dir() {
            continue;
        }
        match std::fs::metadata(entry.path().join(".git")) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return true,
            Ok(meta) if meta.is_dir() => walk.skip_current_dir(),
            // a checkout of a set kept inside `dir` by a workspace inside
            // `dir` (one there with sets of its own, wherever its
            // worktrees_dir puts them) is no sign that `dir` is a set
            // (a record that can't be read says nothing, and is checked below)
            Ok(_)
                if crate::set_record::member(entry.path())
                    .ok()
                    .flatten()
                    .is_some_and(|(_, record)| {
                        let inside = |path: &Path| {
                            path.canonicalize()
                                .is_ok_and(|path| path.starts_with(&canonical_dir))
                        };
                        inside(&record.set)
                            && record.set.canonicalize().ok() != Some(canonical_dir.clone())
                            && inside(&record.project)
                    }) => {}
            // even one meta can no longer read: it was meta's. A checkout
            // whose git directory can't be found (moved, with relative paths)
            // may be meta's too: it is not taken for a plain clone
            Ok(_) => {
                let meta_made = gitdir::gitfile_target(entry.path())
                    .ok()
                    .flatten()
                    .is_none_or(|admin| {
                        // only a record that is surely absent says "not meta's"
                        !admin.is_dir()
                            || !matches!(
                                admin
                                    .join(crate::set_record::RECORD_FILE)
                                    .symlink_metadata(),
                                Err(e) if e.kind() == std::io::ErrorKind::NotFound
                            )
                    });
                if meta_made {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// [`discover_worktree_members`] with each one's branch.
pub fn discover_worktree_repos(task_dir: &Path) -> Result<Vec<WorktreeRepoInfo>> {
    Ok(discover_worktree_members(task_dir)?
        .into_iter()
        .map(with_branch)
        .collect())
}

fn with_branch(member: WorktreeMemberInfo) -> WorktreeRepoInfo {
    WorktreeRepoInfo {
        // a branch git can't read is shown as by git for a detached HEAD;
        // status and list say when a checkout can't be read
        branch: crate::git::current_branch(&member.path)
            .ok()
            .flatten()
            .unwrap_or_else(|| "HEAD".to_string()),
        alias: member.alias,
        path: member.path,
        source_path: member.source_path,
        created_branch: None,
    }
}

/// Every member of the set at `task_dir`, nested ones (`clients/desktop`
/// inside `clients`) too, without branch probes (command scoping needs only
/// their paths, and starts no git for each): the one member list, as a
/// listing of the set shows it. What can't be read is skipped; a checkout
/// found inside a member counts only if git's linked-worktree layout holds
/// (a submodule doesn't).
pub fn discover_worktree_members(task_dir: &Path) -> Result<Vec<WorktreeMemberInfo>> {
    let mut repos = Vec::new();

    // Check if the task dir itself is a worktree (the "." alias)
    let dot_git = task_dir.join(".git");
    if dot_git
        .symlink_metadata()
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        let source = source_repo(task_dir)?;
        repos.push(WorktreeMemberInfo {
            alias: ".".to_string(),
            path: task_dir.to_path_buf(),
            source_path: source,
        });
    }

    // Recursively scan subdirectories for worktrees.
    // Repos may be nested (e.g., "vendor/tree-sitter-markdown") when
    // dependencies have paths with intermediate directories.
    if task_dir.is_dir() {
        let scanned = ScannedDirs::of(task_dir);
        discover_repos_recursive(task_dir, task_dir, &scanned, &mut repos)?;
    }

    // Sort by alias for deterministic output ("." sorts first)
    repos.sort_by(|a, b| a.alias.cmp(&b.alias));

    Ok(repos)
}

/// Recursively scan `dir` for git worktree repos, recording aliases as
/// relative paths from `root` (the worktree task directory).
///
/// A directory with a `.git` file is a worktree repo: recorded, with the
/// repos nested in it ([`add_member`]). A directory without `.git` is an
/// intermediate directory (e.g., `vendor/`): recursed into.
fn discover_repos_recursive(
    root: &Path,
    dir: &Path,
    scanned: &ScannedDirs,
    repos: &mut Vec<WorktreeMemberInfo>,
) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            log::debug!(
                "Skipping unreadable directory {}: {}",
                escape_path(dir),
                escape_lines(&format!("{e:#}"))
            );
            return Ok(());
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let sub_path = entry.path();
        if !entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            continue;
        }
        if !ScannedDirs::searched(&sub_path) {
            continue;
        }
        match is_checkout(&sub_path) {
            true => add_member(root, &sub_path, scanned, false, repos)?,
            // an unreadable one is skipped there; a record that can't be
            // read is an error
            false => discover_repos_recursive(root, &sub_path, scanned, repos)?,
        }
    }
    Ok(())
}

/// Whether `dir` has a `.git` file: a worktree's (or a submodule's).
fn is_checkout(dir: &Path) -> bool {
    dir.join(".git")
        .symlink_metadata()
        .is_ok_and(|m| m.is_file())
}

/// Record the checkout at `dir` as a member when it is one, then the
/// members nested in it. A set lays its repos out as the workspace does, so
/// those sit where its own `.meta.yaml` puts its projects: only there is
/// looked, not through its whole tree (a build folder can hold thousands of
/// folders), and a worktree made by hand inside it is no member. `nested`:
/// `dir` is one of those, where only a checkout in git's linked-worktree
/// layout counts (a submodule doesn't).
fn add_member(
    root: &Path,
    dir: &Path,
    scanned: &ScannedDirs,
    nested: bool,
    repos: &mut Vec<WorktreeMemberInfo>,
) -> Result<()> {
    if nested && gitdir::linked(dir).is_err() {
        return Ok(());
    }
    if !scanned.member(root, dir)? {
        return Ok(());
    }
    let source = match source_repo(dir) {
        Ok(s) => s,
        Err(e) => {
            log::debug!(
                "Skipping {}: {}",
                escape_path(dir),
                escape_lines(&format!("{e:#}"))
            );
            return Ok(());
        }
    };
    repos.push(WorktreeMemberInfo {
        alias: dir
            .strip_prefix(root)
            .unwrap_or(dir)
            .to_string_lossy()
            .to_string(),
        path: dir.to_path_buf(),
        source_path: source.clone(),
    });
    // the projects of its .meta.yaml as checked out, and of its source's as
    // it is on disk, which create planned from (a config that can't be read
    // lists none: the strict scan says why)
    let listed = |checkout: &Path| {
        crate::config::Workspace::in_dir(checkout)
            .ok()
            .flatten()
            .map(|workspace| workspace.projects)
            .unwrap_or_default()
    };
    let mut projects = listed(dir);
    projects.extend(listed(&source));
    projects.sort_by(|a, b| a.path.cmp(&b.path));
    projects.dedup_by(|a, b| a.path == b.path);
    for project in projects {
        let place = Path::new(&project.path);
        // a project outside the checkout is no repo nested in it
        let inside = place
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)));
        let nested = dir.join(place);
        if inside && is_checkout(&nested) {
            add_member(root, &nested, scanned, true, repos)?;
        }
    }
    Ok(())
}

/// Discover all realized worktrees, including repos nested beneath other repos.
/// Unlike legacy discovery, traversal errors are fatal. Under a hidden
/// directory only checkouts meta recorded in this set count (see
/// `ScannedDirs`); directory symlinks are not followed, so traversal never
/// leaves the set.
/// A finite entry budget prevents unbounded scans of generated directory trees.
pub fn discover_worktree_repos_strict(task_dir: &Path) -> Result<Vec<WorktreeRepoInfo>> {
    Ok(discover_worktree_members_strict(task_dir)?
        .into_iter()
        .map(with_branch)
        .collect())
}

/// Realized membership without branch, dirty-state or ahead/behind probes.
/// This uses the same strict bounded traversal and Git backlink validation as
/// recursive status, but never launches Git. Consumers can observe Git once.
#[derive(Debug, Clone, Serialize)]
pub struct WorktreeMemberInfo {
    pub alias: String,
    pub path: PathBuf,
    pub source_path: PathBuf,
}

/// The set, resolved, that meta-git's record says the checkout at `dir` was
/// made for; `None` without a record, or when the set is gone.
fn recorded_set(dir: &Path) -> Result<Option<PathBuf>> {
    Ok(crate::set_record::member(dir)?.and_then(|(_, record)| record.set.canonicalize().ok()))
}

/// Which checkouts in a set are its members. `.git` and a nested
/// `.worktrees` hold none. Under another hidden directory, only a checkout
/// meta's record says it made for this set counts: a project may live at
/// `.deps/x`, but a tool's own worktrees (`.claude/worktrees/x`) are not the
/// set's.
struct ScannedDirs {
    set: PathBuf,
}

impl ScannedDirs {
    fn of(task_dir: &Path) -> Self {
        ScannedDirs {
            set: task_dir
                .canonicalize()
                .unwrap_or_else(|_| task_dir.to_path_buf()),
        }
    }

    /// Whether to look for members inside `dir`.
    fn searched(dir: &Path) -> bool {
        dir.file_name()
            .is_none_or(|name| name != ".git" && name != ".worktrees")
    }

    /// Whether the checkout at `dir`, inside the set at `root`, is a member.
    fn member(&self, root: &Path, dir: &Path) -> Result<bool> {
        let hidden = dir.strip_prefix(root).is_ok_and(|place| {
            place
                .components()
                .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        });
        Ok(!hidden || recorded_set(dir)?.as_ref() == Some(&self.set))
    }
}

pub fn discover_worktree_members_strict(task_dir: &Path) -> Result<Vec<WorktreeMemberInfo>> {
    let mut repos = Vec::new();
    let scanned = ScannedDirs::of(task_dir);
    let entries = walkdir::WalkDir::new(task_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || ScannedDirs::searched(entry.path()));
    for (count, entry) in entries.enumerate() {
        anyhow::ensure!(
            count < 2_000_000,
            "Recursive worktree discovery exceeded 2000000 entries"
        );
        // walkdir's error names the path it failed on: kept on one line
        let entry = entry.map_err(|e| {
            anyhow::anyhow!(
                "Cannot scan worktree {}: {}",
                escape_path(task_dir),
                escape(&e.to_string())
            )
        })?;
        if !entry.file_type().is_dir() {
            continue;
        }
        let path = entry.path();
        let git_file = path.join(".git");
        let metadata = match git_file.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Cannot inspect {}", escape_path(&git_file)))
            }
        };
        if !metadata.is_file() || !scanned.member(task_dir, path)? {
            continue;
        }
        gitdir::linked(path)?;
        let source_path = source_repo(path)?;
        let alias = if path == task_dir {
            ".".to_string()
        } else {
            path.strip_prefix(task_dir)?.to_string_lossy().to_string()
        };
        repos.push(WorktreeMemberInfo {
            alias,
            path: path.to_path_buf(),
            source_path,
        });
    }
    repos.sort_by(|a, b| a.alias.cmp(&b.alias));
    Ok(repos)
}

/// The checkout the linked worktree at `checkout` was made from: the
/// directory above the git directory that holds its admin directory
/// (`<repo>/.git/worktrees/<id>`). An absolute `gitdir:` is taken as written.
fn source_repo(checkout: &Path) -> Result<PathBuf> {
    let target = gitdir::read_gitfile(checkout)?
        .with_context(|| format!("{} has no .git file", escape_path(checkout)))?;
    let admin = if target.is_absolute() {
        target
    } else {
        checkout
            .join(&target)
            .canonicalize()
            .with_context(|| format!("Cannot resolve gitdir of {}", escape_path(checkout)))?
    };
    admin
        .ancestors()
        .nth(3)
        .map(Path::to_path_buf)
        .with_context(|| {
            format!(
                "Cannot derive source repo from gitdir: {}",
                escape_path(&admin)
            )
        })
}

/// Refuse a set name that isn't one folder name of ASCII letters, digits,
/// `-` and `_` (not starting with `.`).
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("Worktree name cannot be empty");
    }
    let shown = escape(name);
    if name.starts_with('.') {
        anyhow::bail!("Invalid worktree name '{shown}': cannot start with '.'");
    }
    if name.contains('/') || name.contains('\\') {
        anyhow::bail!("Invalid worktree name '{shown}': cannot contain path separators");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!("Invalid worktree name '{shown}': only ASCII alphanumeric characters, hyphens, and underscores allowed");
    }
    Ok(())
}

/// The configs a worktree command reads, each loaded once: `here`, the
/// nearest at or above where it runs, and `owner`, whose sets and settings
/// (`worktrees_dir`, hooks) these are: inside a set, the checkout the set
/// stands for, which outlives it; else `here`.
#[derive(Debug, Clone)]
pub struct Configs {
    pub here: crate::config::Workspace,
    pub owner: crate::config::Workspace,
}

impl Configs {
    /// The configs of the workspace at or above `start`; an error outside one.
    pub fn find(start: &Path) -> Result<Self> {
        Self::find_optional(start)?
            .ok_or_else(|| anyhow::anyhow!("Not inside a meta project (no .meta.yaml found)"))
    }

    /// Like [`Configs::find`], `None` outside a workspace.
    pub fn find_optional(start: &Path) -> Result<Option<Self>> {
        crate::config::Workspace::find(start, None)?
            .map(Self::of)
            .transpose()
    }

    /// The configs for `here`. A source checkout without a config is still
    /// the owner, with no settings of its own.
    pub fn of(here: crate::config::Workspace) -> Result<Self> {
        let Some(source) = crate::config::set_source(&here.dir)?.filter(|s| s.is_dir()) else {
            return Ok(Self {
                owner: here.clone(),
                here,
            });
        };
        let owner = crate::config::Workspace::in_dir(&source)?.unwrap_or_else(|| {
            crate::config::Workspace {
                config_path: source.join(crate::config::META_CONFIG_NAME),
                dir: source,
                projects: Vec::new(),
                defaults: Default::default(),
                worktrees_dir: None,
                worktree: Default::default(),
            }
        });
        Ok(Self { here, owner })
    }
}

/// Where worktree sets go: `META_WORKTREES`, else the `worktrees_dir` of
/// `owner`'s config (anywhere; checked when it was loaded), else its
/// `.worktrees`; outside a workspace, `.worktrees` in `cwd`.
pub fn resolve_worktree_root(
    owner: Option<&crate::config::Workspace>,
    cwd: &Path,
) -> Result<PathBuf> {
    if let Ok(env_path) = std::env::var("META_WORKTREES") {
        return Ok(PathBuf::from(env_path));
    }
    let Some(owner) = owner else {
        return Ok(cwd.join(".worktrees"));
    };
    // A link there is followed, wherever it leads (removal checks what is in
    // a set, not where it is), but not into git's data; a configured
    // worktrees_dir was checked when it was loaded
    let Some(configured) = &owner.worktrees_dir else {
        crate::config::check_location(&owner.dir, ".worktrees", false)
            .map_err(|why| anyhow::anyhow!("Refusing to keep worktrees in .worktrees: {why}"))?;
        return Ok(owner.dir.join(".worktrees"));
    };
    Ok(owner.dir.join(configured))
}

#[cfg(test)]
mod recursive_tests {
    use super::*;
    use meta_plugin::testing::Sandbox;

    #[test]
    fn the_sets_own_nested_config_wins_over_the_workspaces() {
        let sandbox = Sandbox::new();
        let (workspace, set) = (sandbox.work().join("ws"), sandbox.work().join("set"));
        let config = |dir: &Path, projects: &str| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(
                dir.join(".meta.yaml"),
                format!(r#"{{"projects": {{{projects}}}}}"#),
            )
            .unwrap();
        };
        config(&workspace, r#""parent": {"repo": "x", "meta": true}"#);
        config(&workspace.join("parent"), r#""x": "x""#);
        let projects = crate::config::Workspace::load(&workspace.join(".meta.yaml"))
            .unwrap()
            .projects;
        let tree = |set: &Path| {
            readable_tree(
                &workspace,
                set,
                &projects,
                &mut std::collections::HashSet::new(),
            )
        };

        // the set doesn't hold parent: the workspace's config tells about x
        std::fs::create_dir_all(&set).unwrap();
        assert_eq!(tree(&set)[0].children.len(), 1);
        // the set holds parent's config, which no longer lists x
        config(&set.join("parent"), "");
        assert!(tree(&set)[0].children.is_empty());
    }

    /// Create a real committed repository and linked worktree in the fixture.
    fn member(sandbox: &Sandbox, root: &Path, alias: &str) {
        let sources = if alias == "." {
            root.parent().unwrap().join(".sources")
        } else {
            root.join(".sources")
        };
        std::fs::create_dir_all(&sources).unwrap();
        let source = sources.join(format!(
            "repo{}",
            std::fs::read_dir(&sources).unwrap().count()
        ));
        sandbox.repo(&source);
        sandbox.git(
            &source,
            &[
                "worktree",
                "add",
                "-qb",
                "topic",
                root.join(alias).to_str().unwrap(),
            ],
        );
    }

    /// Normalize separators for platform-independent alias assertions.
    fn aliases(repos: Vec<WorktreeRepoInfo>) -> Vec<String> {
        repos
            .into_iter()
            .map(|repo| repo.alias.replace('\\', "/"))
            .collect()
    }

    #[test]
    /// Nested members have sorted, distinct, full aliases.
    fn members_include_nested_ones_with_sorted_full_aliases() {
        let sandbox = meta_plugin::testing::isolated!();
        let root = &sandbox.work().join("set");
        for alias in [".", "open-source", "open-source/atc", "other/atc"] {
            member(&sandbox, root, alias);
        }
        // a repo nested in a member is one of its .meta.yaml's projects
        std::fs::write(
            root.join("open-source/.meta.yaml"),
            "projects:\n  atc: u\n  absent: u\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("configured-but-absent")).unwrap();
        // the strict scan finds the same members
        for repos in [
            discover_worktree_repos(root),
            discover_worktree_repos_strict(root),
        ] {
            assert_eq!(
                aliases(repos.unwrap()),
                [".", "open-source", "open-source/atc", "other/atc"]
            );
        }
    }

    #[test]
    fn implicit_context_reaches_nested_members_without_collecting_branch_facts() {
        let sandbox = meta_plugin::testing::isolated!();
        let root = sandbox.work().join(".worktrees/example");
        for alias in [".", "parent", "parent/nested"] {
            member(&sandbox, &root, alias);
        }
        // listed by the source's .meta.yaml on disk (create plans from it),
        // though not by the one checked out in the set
        let source = source_repo(&root.join("parent")).unwrap();
        std::fs::write(source.join(".meta.yaml"), "projects:\n  nested: u\n").unwrap();
        let (name, observed_root) = detect_worktree_location(&root.join("parent/nested"))
            .unwrap()
            .unwrap();
        assert_eq!(name, "example");
        assert_eq!(observed_root, root);
        let paths: Vec<PathBuf> = discover_worktree_members(&root)
            .unwrap()
            .into_iter()
            .map(|member| member.path)
            .collect();
        assert_eq!(
            paths,
            vec![
                root.clone(),
                root.join("parent"),
                root.join("parent/nested")
            ]
        );
        let metadata = discover_worktree_members_strict(&root).unwrap();
        assert_eq!(metadata.len(), 3);
        for row in metadata {
            let json = serde_json::to_value(row).unwrap();
            assert!(json.get("branch").is_none());
            assert!(json.get("dirty").is_none());
        }
    }

    #[test]
    /// Malformed nested Git files fail instead of silently omitting members.
    fn recursive_malformed_member_is_error_instead_of_partial_inventory() {
        let sandbox = meta_plugin::testing::isolated!();
        member(&sandbox, &sandbox.work(), "parent");
        let child = sandbox.work().join("parent/broken");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join(".git"), "invalid").unwrap();
        assert!(discover_worktree_repos_strict(sandbox.work().as_path()).is_err());
        assert_eq!(
            aliases(discover_worktree_repos(sandbox.work().as_path()).unwrap()),
            ["parent"]
        );
    }

    #[test]
    /// Missing roots cannot be represented as complete empty inventories.
    fn recursive_missing_root_is_error() {
        let sandbox = meta_plugin::testing::isolated!();
        assert!(discover_worktree_repos_strict(&sandbox.work().join("missing")).is_err());
    }

    #[cfg(unix)]
    #[test]
    /// Under a hidden directory, a checkout meta recorded for this set is a
    /// member; others (an agent's own worktrees), a nested set and symlink
    /// cycles are not.
    fn recursive_finds_recorded_hidden_members_only() {
        let sandbox = meta_plugin::testing::isolated!();
        let root = sandbox.work();
        member(&sandbox, &root, "visible");
        member(&sandbox, &root, ".deps/child");
        let gitfile = std::fs::read_to_string(root.join(".deps/child/.git")).unwrap();
        let admin = Path::new(gitfile.trim().strip_prefix("gitdir: ").unwrap());
        std::fs::write(
            admin.join(crate::set_record::RECORD_FILE),
            serde_json::json!({ "project": "/p", "set": root, "created_at": "2025-01-01T00:00:00Z" })
                .to_string(),
        )
        .unwrap();
        member(&sandbox, &root, ".claude/worktrees/agent");
        member(&sandbox, &root, ".worktrees/other");
        std::os::unix::fs::symlink(&root, root.join("visible/cycle")).unwrap();
        assert_eq!(
            aliases(discover_worktree_repos_strict(&root).unwrap()),
            [".deps/child", "visible"]
        );
    }
    /// Relative Git links resolve against the checkout, independently of cwd.
    #[test]
    fn relative_gitdir_reports_absolute_source() {
        let sandbox = meta_plugin::testing::isolated!();
        let root = sandbox.work().join("set");
        member(&sandbox, &root, ".");
        let source = sandbox
            .work()
            .join(".sources/repo0")
            .canonicalize()
            .unwrap();
        let git_file = root.join(".git");
        let content = std::fs::read_to_string(&git_file).unwrap();
        let admin = Path::new(content.trim().strip_prefix("gitdir: ").unwrap());
        let name = admin.file_name().unwrap().to_str().unwrap();
        std::fs::write(
            &git_file,
            format!("gitdir: ../.sources/repo0/.git/worktrees/{name}\n"),
        )
        .unwrap();
        assert_eq!(source_repo(&root).unwrap(), source);
        assert_eq!(
            discover_worktree_repos_strict(&root).unwrap()[0].source_path,
            source
        );
        assert_eq!(
            discover_worktree_repos(&root).unwrap()[0].source_path,
            source
        );
    }

    /// Dangling and incomplete admin links must not produce successful members.
    #[test]
    fn recursive_rejects_missing_worktree_metadata() {
        let sandbox = meta_plugin::testing::isolated!();
        for missing in ["HEAD", "commondir", "gitdir", "entire-admin"] {
            let root = sandbox.work().join(missing);
            member(&sandbox, &root, "child");
            let content = std::fs::read_to_string(root.join("child/.git")).unwrap();
            let admin = Path::new(content.trim().strip_prefix("gitdir: ").unwrap());
            if missing == "entire-admin" {
                std::fs::remove_dir_all(admin).unwrap();
            } else {
                std::fs::remove_file(admin.join(missing)).unwrap();
            }
            assert!(discover_worktree_repos_strict(&root).is_err(), "{missing}");
        }
    }
}
