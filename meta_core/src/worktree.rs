//! Worktree context detection for meta: the set a command runs in, the
//! config naming its members, and where a workspace keeps its sets. Used
//! by the meta binary for worktree-scoped command dispatch; which checkouts
//! a set holds is its provider's to say (meta-git scans its own).

use anyhow::Result;
use meta_plugin::display::escape_path;
use std::path::{Path, PathBuf};

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

/// The workspace the set at `set_dir` belongs to: the one its owner file
/// names while the file was made for this very folder (a set made with
/// `META_WORKTREES` elsewhere); else the workspace whose worktrees folder
/// holds it, nearest above it (in a copied or moved workspace, the copy:
/// an owner file made elsewhere is stale, never followed). None when no
/// workspace's sets are there (a set that stands for itself).
fn set_origin(set_dir: &Path) -> Result<Option<PathBuf>> {
    let real = |dir: &Path| dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if let Some(owner) = meta_plugin::sets::owner(set_dir)? {
        if owner.set == real(set_dir) && owner.workspace.is_dir() {
            return Ok(Some(owner.workspace));
        }
    }
    let Some(sets) = set_dir.parent() else {
        return Ok(None);
    };
    for dir in sets.ancestors().skip(1) {
        let Ok(Some(workspace)) = crate::config::Workspace::in_dir(dir) else {
            continue;
        };
        let theirs = resolve_worktree_root(Some(&workspace), dir).ok();
        if theirs.as_deref().map(real) == Some(real(sets)) {
            return Ok(Some(dir.to_path_buf()));
        }
    }
    Ok(None)
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
    let unknown = |place: &Path, e: anyhow::Error| {
        anyhow::anyhow!(
            "--primary can't tell which workspace worktree set {} was made from: {e:#}. \
             Run meta from that workspace's directory instead.",
            escape_path(place)
        )
    };
    while let Some((_, set_dir)) =
        detect_worktree_location(&place).map_err(|e| unknown(&place, e))?
    {
        let origin = set_origin(&set_dir).map_err(|e| unknown(&set_dir, e))?;
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
/// from), each nested meta repo's projects those its config lists in the
/// set at `set` and those the workspace's lists, the set's first (its entry
/// wins in a map built in order), passing over each nested config that
/// can't be read: one broken config loses only its own projects. A
/// checkout in the set is a member whichever of the two lists it.
fn readable_tree(
    dir: &Path,
    set: &Path,
    projects: &[crate::config::ProjectInfo],
    visited: &mut std::collections::HashSet<PathBuf>,
) -> Vec<crate::config::MetaTreeNode> {
    use crate::config;
    let listed = |folder: &Path| {
        config::find_meta_config_in(folder)
            .ok()
            .flatten()
            .and_then(|path| config::Workspace::load(&path).ok())
            .map(|w| w.projects)
    };
    projects
        .iter()
        .map(|project| {
            let (path, place) = (dir.join(&project.path), set.join(&project.path));
            // each meta repo once, as `walk_projects` does: links can loop
            let first = visited.insert(path.canonicalize().unwrap_or_else(|_| path.clone()));
            let (own, theirs) = if first {
                (listed(&place), listed(&path))
            } else {
                (None, None)
            };
            let is_meta = own.is_some() || theirs.is_some();
            let mut nested: Vec<config::ProjectInfo> = own.unwrap_or_default();
            for project in theirs.unwrap_or_default() {
                if !nested.iter().any(|p| p.path == project.path) {
                    nested.push(project);
                }
            }
            config::MetaTreeNode {
                info: project.clone(),
                is_meta,
                error: None,
                children: readable_tree(&path, &place, &nested, visited),
            }
        })
        .collect()
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
mod tests {
    use super::*;
    use meta_plugin::testing::Sandbox;

    #[test]
    fn a_nested_repos_projects_are_those_the_set_or_the_workspace_lists() {
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
        // the set holds parent's config, which lists y and not x: both are
        // in the tree (a checkout of either in the set is a member), the
        // set's first
        config(&set.join("parent"), r#""y": "y""#);
        let names: Vec<String> = tree(&set)[0]
            .children
            .iter()
            .map(|c| c.info.name.clone())
            .collect();
        assert_eq!(names, ["y", "x"]);
    }
}
