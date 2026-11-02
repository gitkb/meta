//! Worktree context detection and discovery for meta.
//!
//! Provides filesystem-based detection of worktree contexts and
//! discovery of repos within a worktree set. Used by meta_cli for
//! automatic worktree-scoped command dispatch, and by meta_git_lib
//! for worktree management commands.

use anyhow::{Context, Result};
use meta_core::display::{escape, escape_lines, escape_path};
use serde::Serialize;
use std::path::{Path, PathBuf};

use meta_core::gitdir;

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

/// Detect if cwd is inside a `.worktrees/<name>/` directory.
/// Returns (task_name, task_dir, repo_paths) if inside a worktree, None otherwise.
/// Filesystem-based detection — no store dependency.
///
/// Walks the path components looking for a `.worktrees` segment followed by a task name.
/// Works whether cwd is the task dir itself, a repo within it, or deeper inside a repo.
pub fn detect_worktree_context(cwd: &Path) -> Result<Option<(String, PathBuf, Vec<PathBuf>)>> {
    let Some((task_name, task_dir)) = detect_worktree_location(cwd)? else {
        return Ok(None);
    };
    let Ok(repos) = discover_worktree_members(&task_dir) else {
        return Ok(None);
    };
    if repos.is_empty() {
        return Ok(None);
    }
    let paths = repos.into_iter().map(|r| r.path).collect();
    Ok(Some((task_name, task_dir, paths)))
}

/// Locate the enclosing `.worktrees/<name>` set, or the set meta's record
/// names for the checkout around `cwd`, without scanning its contents.
/// This is a location hint, not validation. Callers must validate membership and
/// report errors rather than silently falling back to a different set on failure.
/// A record that can't be read is an error.
pub fn detect_worktree_location(cwd: &Path) -> Result<Option<(String, PathBuf)>> {
    use std::path::Component;

    let components: Vec<_> = cwd.components().collect();

    // Find the `.worktrees` component and extract the task name (next component)
    for (i, comp) in components.iter().enumerate() {
        if let Component::Normal(name) = comp {
            if name.to_str() == Some(".worktrees") {
                // Next component after .worktrees is the task name
                let task_name = match components.get(i + 1) {
                    Some(Component::Normal(n)) => match n.to_str() {
                        Some(n) => n.to_string(),
                        None => return Ok(None),
                    },
                    _ => return Ok(None), // .worktrees/ with nothing after it
                };

                // Reconstruct the task directory: everything up to and including .worktrees/<task>
                let task_dir: PathBuf = components.iter().take(i + 2).collect();
                return Ok(Some((task_name, task_dir)));
            }
        }
    }

    // A set elsewhere (`worktrees_dir` outside, or `.worktrees` a link, so
    // the real path has no such part): the set meta recorded for the
    // checkout around `cwd`
    let Ok(real) = cwd.canonicalize() else {
        return Ok(None);
    };
    for checkout in real.ancestors().filter(|dir| dir.join(".git").is_file()) {
        let Some(set) = recorded_set(checkout)? else {
            continue;
        };
        if let Some(name) = set.file_name().and_then(|n| n.to_str()) {
            if real.starts_with(&set) {
                return Ok(Some((name.to_string(), set)));
            }
        }
    }
    Ok(None)
}

/// The set, resolved, that meta's record says the checkout at `dir` was
/// made for; `None` without a record, or when the set is gone.
fn recorded_set(dir: &Path) -> Result<Option<PathBuf>> {
    Ok(meta_core::set_record::member(dir)?.and_then(|(_, record)| record.set.canonicalize().ok()))
}

/// The config that names the members of the set at `set_dir`: the set's own
/// (its root checkout's), else the config of the workspace meta recorded the
/// set was made from. Only a set without meta's records looks above itself.
pub fn set_config(
    set_dir: &Path,
    members: &[PathBuf],
    config_name: Option<&PathBuf>,
) -> Result<Option<PathBuf>> {
    use meta_core::config;
    if let Some(found) = config::meta_config_at(set_dir, config_name)? {
        return Ok(Some(found));
    }
    let origin = set_origin(set_dir, members)
        .map_err(|e| anyhow::anyhow!("{e:#}. Run meta from the workspace's directory instead."))?;
    match origin {
        Some(source) => config::meta_config_at(&source, config_name),
        None => config::find_meta_config(set_dir, config_name),
    }
}

/// Where `dir`, inside a worktree set meta made, is in the workspace the set
/// was made from (and from a set made inside another set, on to that one's
/// workspace), by meta's records in the set's checkouts. None outside a set,
/// and for a set without records (made by hand).
pub fn primary_dir(dir: &Path) -> Result<Option<PathBuf>> {
    let mut place = dir.to_path_buf();
    let mut seen = Vec::new();
    while let Some((_, set_dir)) = detect_worktree_location(&place)? {
        let members: Vec<PathBuf> = discover_worktree_members(&set_dir)?
            .into_iter()
            .map(|member| member.path)
            .collect();
        // set_origin's way out of its error is --primary itself
        let origin = set_origin(&set_dir, &members).map_err(|e| {
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
        // records that lead in a circle lead nowhere
        if seen.contains(&place) {
            break;
        }
        seen.push(place.clone());
    }
    Ok((!seen.is_empty()).then_some(place))
}

/// The workspace meta recorded the set at `set_dir` was made from, by the
/// members whose record names this set (another set's checkout moved in
/// doesn't count). Members that name different workspaces are an error.
fn set_origin(set_dir: &Path, members: &[PathBuf]) -> Result<Option<PathBuf>> {
    let set = set_dir
        .canonicalize()
        .unwrap_or_else(|_| set_dir.to_path_buf());
    let mut origin: Option<PathBuf> = None;
    for member in members {
        let Some((_, record)) = meta_core::set_record::member(member)? else {
            continue;
        };
        if record.set.canonicalize().ok().as_ref() != Some(&set) {
            continue;
        }
        let project = record.project;
        match &origin {
            Some(first) if *first != project => anyhow::bail!(
                "The checkouts of worktree set {} name different workspaces ({} and {}); \
                 meta can't tell which config describes them",
                escape_path(set_dir),
                escape_path(first),
                escape_path(&project)
            ),
            _ => origin = Some(project),
        }
    }
    Ok(origin)
}

/// The checkouts among `members` of the set at `set_dir` that `filters`
/// select (see [`crate::repo_filter::Scope::select`]). A set lays its repos out as
/// the workspace, so a checkout is named and tagged by the project at its
/// place in the set, nested meta repos included, as the set's `workspace`
/// and the nested configs tell (see [`set_tree`]); a checkout of no project,
/// such as the set's root, has neither name nor tags.
pub fn select_in_set(
    set_dir: &Path,
    members: &[PathBuf],
    workspace: Option<&meta_core::config::Workspace>,
    filters: &crate::repo_filter::Filters,
) -> Result<Vec<PathBuf>> {
    use crate::repo_filter::{Candidate, Scope};
    if filters.is_empty() {
        return Ok(members.to_vec());
    }
    if workspace.is_none() && !filters.tags.is_empty() {
        anyhow::bail!(
            "No meta config found for worktree set {}, so --tag cannot select its repos",
            escape_path(set_dir)
        );
    }
    // where each project's checkout is in the set (an outside project's at
    // its name)
    let projects: Vec<_> = workspace
        .map_or(&[][..], |w| &w.projects)
        .iter()
        .map(|project| meta_core::config::ProjectInfo {
            path: project.set_place().to_string(),
            ..project.clone()
        })
        .collect();
    let tree = set_tree(set_dir, members, &projects)?;
    // Compared as directories: a project at a link in the set is the
    // checkout where the link leads
    let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let infos: std::collections::HashMap<PathBuf, meta_core::config::ProjectInfo> =
        meta_core::config::build_project_map(&tree, set_dir, "")
            .into_iter()
            .map(|(place, (_, info))| (real(&set_dir.join(place)), info))
            .collect();
    let repos = members
        .iter()
        .map(|member| match infos.get(&real(member)) {
            Some(info) => Candidate::project(member.clone(), info),
            None => Candidate::root(member.clone()),
        })
        .collect();
    Scope {
        root: set_dir.to_path_buf(),
        config: workspace.map(|w| w.config_path.clone()),
        repos,
        // a tag of a project the set doesn't hold is no typo
        tags: infos.into_values().flat_map(|info| info.tags).collect(),
        nested: true,
    }
    .select(filters)
}

/// The meta tree that describes the checkouts of the set at `set_dir`, whose
/// config lists `projects`: the nested configs of the workspace the set was
/// made from, for nested meta repos the set doesn't hold (one skipped, say),
/// then the set's own, so a map built from it in order lets the set's win.
/// The workspace's part is best effort: what neither tells about stays
/// unselected.
fn set_tree(
    set_dir: &Path,
    members: &[PathBuf],
    projects: &[meta_core::config::ProjectInfo],
) -> Result<Vec<meta_core::config::MetaTreeNode>> {
    use meta_core::config;
    let own = config::walk_projects(set_dir, projects, None)?;
    let mut tree: Vec<_> = set_origin(set_dir, members)?
        .map(|dir| {
            let mut visited = std::collections::HashSet::new();
            not_held(readable_tree(&dir, projects, &mut visited), set_dir)
        })
        .unwrap_or_default();
    tree.extend(own);
    Ok(tree)
}

/// The meta tree of `projects` in `dir`, passing over each nested config
/// that can't be read: one broken config loses only its own projects.
fn readable_tree(
    dir: &Path,
    projects: &[meta_core::config::ProjectInfo],
    visited: &mut std::collections::HashSet<PathBuf>,
) -> Vec<meta_core::config::MetaTreeNode> {
    use meta_core::config;
    projects
        .iter()
        .map(|project| {
            let path = dir.join(&project.path);
            let nested = config::find_meta_config_in(&path).ok().flatten();
            // each meta repo once, as `walk_projects` does: links can loop
            let children = nested
                .as_ref()
                .filter(|_| visited.insert(path.canonicalize().unwrap_or_else(|_| path.clone())))
                .and_then(|config| config::Workspace::load(config).ok())
                .map(|nested| readable_tree(&path, &nested.projects, visited))
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

/// `nodes` without the projects of the nested meta repos whose config the
/// set at `dir` holds: those the set's own tree describes.
fn not_held(
    nodes: Vec<meta_core::config::MetaTreeNode>,
    dir: &Path,
) -> Vec<meta_core::config::MetaTreeNode> {
    nodes
        .into_iter()
        .map(|mut node| {
            let place = dir.join(&node.info.path);
            node.children = match meta_core::config::find_meta_config_in(&place) {
                Ok(Some(_)) => Vec::new(),
                _ => not_held(node.children, &place),
            };
            node
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
                if meta_core::set_record::member(entry.path())
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
                                    .join(meta_core::set_record::RECORD_FILE)
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

/// Discover repos within a worktree task directory by scanning for .git files.
/// Results are sorted by alias for deterministic output.
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
        branch: meta_core::git::current_branch(&member.path)
            .ok()
            .flatten()
            .unwrap_or_else(|| "HEAD".to_string()),
        alias: member.alias,
        path: member.path,
        source_path: member.source_path,
        created_branch: None,
    }
}

/// Legacy discovery boundaries without branch probes. Implicit command scoping
/// only needs repository paths; it must not start Git for each member.
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
/// A directory with a `.git` file is a worktree repo — record it and stop
/// recursing into it. A directory without `.git` is an intermediate directory
/// (e.g., `vendor/`) — recurse into it to find nested repos.
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

        let sub_git = sub_path.join(".git");
        if sub_git
            .symlink_metadata()
            .map(|m| m.is_file())
            .unwrap_or(false)
        {
            if !scanned.member(root, &sub_path)? {
                continue;
            }
            // This is a worktree repo — use relative path from root as alias
            let source = match source_repo(&sub_path) {
                Ok(s) => s,
                Err(e) => {
                    log::debug!(
                        "Skipping {}: {}",
                        escape_path(&sub_path),
                        escape_lines(&format!("{e:#}"))
                    );
                    continue;
                }
            };
            let alias = sub_path
                .strip_prefix(root)
                .unwrap_or(&sub_path)
                .to_string_lossy()
                .to_string();
            repos.push(WorktreeMemberInfo {
                alias,
                path: sub_path,
                source_path: source,
            });
        } else {
            // Not a repo — recurse into intermediate directory (an
            // unreadable one is skipped there; a record that can't be read
            // is an error)
            discover_repos_recursive(root, &sub_path, scanned, repos)?;
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
pub fn discover_worktree_repos_recursive(task_dir: &Path) -> Result<Vec<WorktreeRepoInfo>> {
    Ok(discover_worktree_members_recursive(task_dir)?
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

pub fn discover_worktree_members_recursive(task_dir: &Path) -> Result<Vec<WorktreeMemberInfo>> {
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

#[cfg(test)]
mod recursive_tests {
    use super::*;
    use meta_test_support::Sandbox;

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
        let projects = meta_core::config::Workspace::load(&workspace.join(".meta.yaml"))
            .unwrap()
            .projects;
        let tree = meta_core::config::walk_projects(&workspace, &projects, None).unwrap();

        // the set doesn't hold parent: the workspace's config tells about x
        std::fs::create_dir_all(&set).unwrap();
        assert_eq!(not_held(tree.clone(), &set)[0].children.len(), 1);
        // the set holds parent's config, which no longer lists x
        config(&set.join("parent"), "");
        assert!(not_held(tree, &set)[0].children.is_empty());
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
    /// Nested members keep sorted distinct aliases and default boundaries.
    fn recursive_members_preserve_legacy_boundaries_and_sorted_full_aliases() {
        let sandbox = meta_test_support::isolated!();
        let root = &sandbox.work().join("set");
        for alias in [".", "open-source", "open-source/atc", "other/atc"] {
            member(&sandbox, root, alias);
        }
        std::fs::create_dir_all(root.join("configured-but-absent")).unwrap();
        assert_eq!(
            aliases(discover_worktree_repos(root).unwrap()),
            [".", "open-source", "other/atc"]
        );
        assert_eq!(
            aliases(discover_worktree_repos_recursive(root).unwrap()),
            [".", "open-source", "open-source/atc", "other/atc"]
        );
    }

    #[test]
    fn implicit_context_keeps_legacy_scope_without_collecting_branch_facts() {
        let sandbox = meta_test_support::isolated!();
        let root = sandbox.work().join(".worktrees/example");
        for alias in [".", "parent", "parent/nested"] {
            member(&sandbox, &root, alias);
        }
        let (name, observed_root, paths) = detect_worktree_context(&root.join("parent/nested"))
            .unwrap()
            .unwrap();
        assert_eq!(name, "example");
        assert_eq!(observed_root, root);
        assert_eq!(paths, vec![root.clone(), root.join("parent")]);
        let metadata = discover_worktree_members_recursive(&root).unwrap();
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
        let sandbox = meta_test_support::isolated!();
        member(&sandbox, &sandbox.work(), "parent");
        let child = sandbox.work().join("parent/broken");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join(".git"), "invalid").unwrap();
        assert!(discover_worktree_repos_recursive(sandbox.work().as_path()).is_err());
        assert_eq!(
            aliases(discover_worktree_repos(sandbox.work().as_path()).unwrap()),
            ["parent"]
        );
    }

    #[test]
    /// Missing roots cannot be represented as complete empty inventories.
    fn recursive_missing_root_is_error() {
        let sandbox = meta_test_support::isolated!();
        assert!(discover_worktree_repos_recursive(&sandbox.work().join("missing")).is_err());
    }

    #[cfg(unix)]
    #[test]
    /// Under a hidden directory, a checkout meta recorded for this set is a
    /// member; others (an agent's own worktrees), a nested set and symlink
    /// cycles are not.
    fn recursive_finds_recorded_hidden_members_only() {
        let sandbox = meta_test_support::isolated!();
        let root = sandbox.work();
        member(&sandbox, &root, "visible");
        member(&sandbox, &root, ".deps/child");
        let gitfile = std::fs::read_to_string(root.join(".deps/child/.git")).unwrap();
        let admin = Path::new(gitfile.trim().strip_prefix("gitdir: ").unwrap());
        std::fs::write(
            admin.join(meta_core::set_record::RECORD_FILE),
            serde_json::json!({ "project": "/p", "set": root, "created_at": "2025-01-01T00:00:00Z" })
                .to_string(),
        )
        .unwrap();
        member(&sandbox, &root, ".claude/worktrees/agent");
        member(&sandbox, &root, ".worktrees/other");
        std::os::unix::fs::symlink(&root, root.join("visible/cycle")).unwrap();
        assert_eq!(
            aliases(discover_worktree_repos_recursive(&root).unwrap()),
            [".deps/child", "visible"]
        );
    }
    /// Relative Git links resolve against the checkout, independently of cwd.
    #[test]
    fn relative_gitdir_reports_absolute_source() {
        let sandbox = meta_test_support::isolated!();
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
            discover_worktree_repos_recursive(&root).unwrap()[0].source_path,
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
        let sandbox = meta_test_support::isolated!();
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
            assert!(
                discover_worktree_repos_recursive(&root).is_err(),
                "{missing}"
            );
        }
    }
}
