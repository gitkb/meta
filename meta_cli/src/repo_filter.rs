//! `--include` / `--exclude`: which repos a command runs in.

use anyhow::{bail, Result};
use meta_core::config::{MetaTreeNode, ProjectInfo};
use std::path::{Component, Path, PathBuf};

/// (`.meta.yaml` name, path from `meta_dir`) for `projects`, and for every project
/// nested under `tree` (the result of walking the same meta root). Paths are
/// in the same form as [`select`] gives the repos it chooses among.
pub fn repo_names(
    meta_dir: &Path,
    projects: &[ProjectInfo],
    tree: &[MetaTreeNode],
) -> Vec<(String, String)> {
    fn walk(nodes: &[MetaTreeNode], dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        for node in nodes {
            let path = dir.join(&node.info.path);
            out.push((node.info.name.clone(), repo_path(&path, root)));
            walk(&node.children, &path, root, out);
        }
    }
    let mut names: Vec<(String, String)> = projects
        .iter()
        .map(|p| (p.name.clone(), repo_path(&meta_dir.join(&p.path), meta_dir)))
        .collect();
    walk(tree, meta_dir, meta_dir, &mut names);
    names
}

/// The repos among `dirs` that `include` and `exclude` select, in order.
///
/// A value names a repo by its `.meta.yaml` name (from `names`, see
/// [`repo_names`]) or by its whole path from `root` (`.` for the root; a
/// trailing `/` is ignored), never by a part of either. A value starting with
/// `./` is a path only. Repos are compared as directories, so excluding one
/// excludes every spelling of it (`api`, `./api` and `api/`). A value that names
/// none of `dirs` is an error, so a typo never runs a command in a repo it was
/// meant to leave out; so is a value that names several repos, and a selection
/// that leaves no repo.
pub fn select(
    include: &[String],
    exclude: &[String],
    dirs: &[String],
    root: &Path,
    names: &[(String, String)],
) -> Result<Vec<String>> {
    if include.is_empty() && exclude.is_empty() {
        return Ok(dirs.to_vec());
    }
    let repos: Vec<Repo> = dirs
        .iter()
        .map(|dir| Repo {
            dir,
            path: repo_path(Path::new(dir), root),
            id: std::fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir)),
        })
        .collect();
    // A path is the repo's, or leads there through a link
    let names_dir = |path: &str, repo: &Repo| {
        *path == repo.path || std::fs::canonicalize(root.join(path)).is_ok_and(|id| id == repo.id)
    };
    let named = |flag: &str, value: &str| -> Result<Vec<&Path>> {
        let wanted = path_value(value);
        let by_path = distinct(
            repos
                .iter()
                .filter(|r| wanted.as_deref().is_some_and(|path| names_dir(path, r))),
        );
        let by_name = if value.starts_with("./") {
            vec![]
        } else {
            distinct(repos.iter().filter(|r| {
                names
                    .iter()
                    .any(|(name, p)| name == value && names_dir(p, r))
            }))
        };
        let listed = |repos: &[&Repo]| -> String {
            repos
                .iter()
                .map(|r| meta_core::display::escape(&r.path))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let value = meta_core::display::escape(value);
        if by_name.len() > 1 {
            bail!(
                "{flag} '{value}' is the .meta.yaml name of the repos at {}; name one by its path, \
                 as ./<path>",
                listed(&by_name)
            );
        }
        let ids =
            |repos: &[&Repo]| -> Vec<PathBuf> { repos.iter().map(|r| r.id.clone()).collect() };
        if !by_path.is_empty() && !by_name.is_empty() && ids(&by_path) != ids(&by_name) {
            bail!(
                "{flag} '{value}' names the repo at {} by path and the repo at {} by .meta.yaml \
                 name; name the one you mean by its path, as ./<path>",
                listed(&by_path),
                listed(&by_name)
            );
        }
        let hits = if by_path.is_empty() { by_name } else { by_path };
        if hits.is_empty() {
            bail!(
                "{flag} '{value}' names none of the repos this command runs in. Use a repo's \
                 name from .meta.yaml or its path from the workspace root (`.` for the root), as \
                 `meta project list` shows it; in a worktree set, the same, from the set's root. --tag and \
                 --recursive change which repos the command runs in."
            );
        }
        Ok(hits.iter().map(|r| r.id.as_path()).collect())
    };
    let mut included = Vec::new();
    for value in include {
        included.extend(named("--include", value)?);
    }
    let mut excluded = Vec::new();
    for value in exclude {
        excluded.extend(named("--exclude", value)?);
    }
    let selected: Vec<String> = repos
        .iter()
        .filter(|r| include.is_empty() || included.contains(&r.id.as_path()))
        .filter(|r| !excluded.contains(&r.id.as_path()))
        .map(|r| r.dir.clone())
        .collect();
    if selected.is_empty() {
        bail!("--include/--exclude leave no repo to run in");
    }
    Ok(selected)
}

/// A repo `select` chooses among: as given, as filters name it, and the
/// directory it is.
struct Repo<'a> {
    dir: &'a String,
    path: String,
    id: PathBuf,
}

/// `repos` with each directory once.
fn distinct<'a>(repos: impl Iterator<Item = &'a Repo<'a>>) -> Vec<&'a Repo<'a>> {
    let mut out: Vec<&Repo> = Vec::new();
    for repo in repos {
        if !out.iter().any(|r| r.id == repo.id) {
            out.push(repo);
        }
    }
    out
}

/// `dir`'s path from `root`, `/`-separated and `.` for the root itself, with
/// `..` parts for a directory outside `root` (`../shared`); `dir` as given
/// when the two share no prefix (another drive). Relative paths are taken from
/// the current directory.
fn repo_path(dir: &Path, root: &Path) -> String {
    let absolute = |path: &Path| {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let (dir_abs, root_abs) = (absolute(dir), absolute(root));
    let shared = dir_abs
        .components()
        .zip(root_abs.components())
        .take_while(|(a, b)| a == b)
        .count();
    if shared == 0 {
        return dir.display().to_string();
    }
    let up = root_abs
        .components()
        .skip(shared)
        .map(|_| Component::ParentDir);
    let rel: PathBuf = up.chain(dir_abs.components().skip(shared)).collect();
    match normalize(&rel.to_string_lossy()) {
        rel if rel.is_empty() => ".".to_string(),
        rel => rel,
    }
}

/// A filter value as the path it names from the root: `.` for the root,
/// otherwise [`normalize`]d. None for a value that cannot be such a path
/// (empty or absolute).
fn path_value(value: &str) -> Option<String> {
    let absolute = Path::new(value)
        .components()
        .any(|c| matches!(c, Component::RootDir | Component::Prefix(_)));
    match normalize(value) {
        _ if absolute || value.is_empty() => None,
        path if path.is_empty() => Some(".".to_string()),
        path => Some(path),
    }
}

/// `path` `/`-separated, without `.` parts or a trailing `/`; `.` alone
/// becomes empty. `..` parts stay: dropping them would give two different
/// directories the same name.
fn normalize(path: &str) -> String {
    let parts: Vec<_> = Path::new(path)
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|s| s.to_string()).collect()
    }

    fn project(name: &str, path: &str) -> ProjectInfo {
        ProjectInfo {
            name: name.to_string(),
            path: path.to_string(),
            repo: None,
            tags: vec![],
            provides: vec![],
            depends_on: vec![],
            meta: false,
        }
    }

    #[test]
    fn values_name_whole_repos_never_parts_of_paths() {
        let root = std::env::temp_dir().join("ws").join("a");
        let rels = ["", "api", "api-legacy", "libs/core"];
        let dirs: Vec<String> = rels
            .iter()
            .map(|rel| root.join(rel).display().to_string())
            .collect();
        let names = repo_names(&root, &[project("core-lib", "./libs/core/")], &[]);
        let select = |include: &[&str], exclude: &[&str]| {
            select(&strings(include), &strings(exclude), &dirs, &root, &names)
        };
        let picked = |include: &[&str], exclude: &[&str]| -> Vec<String> {
            let dir = |rel: &str| root.join(rel).display().to_string();
            let chosen = select(include, exclude).unwrap();
            rels.iter()
                .filter(|rel| chosen.contains(&dir(rel)))
                .map(|rel| if rel.is_empty() { "." } else { rel }.to_string())
                .collect()
        };

        assert_eq!(picked(&["api"], &[]), ["api"]);
        assert_eq!(picked(&["./api/"], &[]), ["api"]);
        assert_eq!(picked(&["."], &[]), ["."]);
        assert_eq!(picked(&["libs/core"], &[]), ["libs/core"]);
        assert_eq!(picked(&["core-lib"], &[]), ["libs/core"]);
        // `./` makes a value a path only
        assert_eq!(picked(&["./libs/core/"], &[]), ["libs/core"]);
        assert!(select(&["./core-lib"], &[]).is_err());
        assert_eq!(picked(&[], &["api"]), [".", "api-legacy", "libs/core"]);
        assert_eq!(picked(&["api", "api-legacy"], &["api"]), ["api-legacy"]);
        assert_eq!(picked(&[], &[]), [".", "api", "api-legacy", "libs/core"]);
        // Parts of a name or of the absolute path name nothing
        let absolute = root.join("api").display().to_string();
        for value in ["a", "ap", "core", "ws", absolute.as_str(), ""] {
            let err = select(&[value], &[]).unwrap_err().to_string();
            assert!(err.contains("names none of the repos"), "{value}: {err}");
            let err = select(&[], &[value]).unwrap_err().to_string();
            assert!(err.contains("--exclude"), "{value}: {err}");
        }
        let err = select(&["api"], &["api"]).unwrap_err().to_string();
        assert!(err.contains("leave no repo"), "{err}");
    }

    #[test]
    fn repos_are_directories_not_spellings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("ws");
        for dir in ["api", "libs", "child"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let rels = ["", "api", "libs/../api", "child/.."];
        let dirs: Vec<String> = rels
            .iter()
            .map(|rel| root.join(rel).display().to_string())
            .collect();
        let names = repo_names(&root, &[project("outer", "libs/../api")], &[]);
        let picked = |include: &[&str], exclude: &[&str]| -> Vec<&str> {
            let chosen =
                select(&strings(include), &strings(exclude), &dirs, &root, &names).unwrap();
            rels.iter()
                .zip(&dirs)
                .filter(|(_, dir)| chosen.contains(dir))
                .map(|(rel, _)| *rel)
                .collect()
        };
        // Excluding a repo excludes every spelling of its directory
        assert_eq!(picked(&[], &["api"]), ["", "child/.."]);
        assert_eq!(picked(&[], &["."]), ["api", "libs/../api"]);
        assert_eq!(picked(&["outer"], &[]), ["api", "libs/../api"]);
        // `..` is kept: the workspace's parent is not the workspace
        let parent = repo_names(&root, &[project("parent", "..")], &[]);
        let up = [root.join("..").display().to_string()];
        let err = select(&strings(&["."]), &[], &up, &root, &parent).unwrap_err();
        assert!(err.to_string().contains("names none"), "{err}");
    }

    #[test]
    fn names_from_the_tree_match_nested_and_top_level_repos() {
        let root = std::env::temp_dir().join("ws").join("a");
        let dirs: Vec<String> = ["", "parent", "parent/libs/core"]
            .iter()
            .map(|rel| root.join(rel).display().to_string())
            .collect();
        let node = |info: ProjectInfo, children| MetaTreeNode {
            info,
            is_meta: true,
            children,
        };
        let tree = [node(
            project("top", "parent"),
            vec![node(project("core-lib", "./libs/core"), vec![])],
        )];
        let names = repo_names(&root, &[], &tree);
        for (value, rel) in [("top", "parent"), ("core-lib", "parent/libs/core")] {
            let chosen = select(&strings(&[value]), &[], &dirs, &root, &names).unwrap();
            assert_eq!(chosen, [root.join(rel).display().to_string()], "{value}");
        }
    }

    #[test]
    fn absolute_paths_and_ambiguous_names() {
        let root = std::env::temp_dir().join("ws").join("a");
        let outside = std::env::temp_dir().join("elsewhere");
        let dirs: Vec<String> = [
            root.join("libs/core"),
            root.join("web"),
            root.join("frontend"),
            outside.clone(),
        ]
        .iter()
        .map(|dir| dir.display().to_string())
        .collect();
        let absolute = |dir: &std::path::Path| dir.display().to_string();
        let names = repo_names(
            &root,
            &[
                project("core-lib", &absolute(&root.join("libs/core"))),
                project("far", &absolute(&outside)),
                project("web", "frontend"),
            ],
            &[],
        );
        let select = |value: &str| select(&strings(&[value]), &[], &dirs, &root, &names);
        assert_eq!(select("core-lib").unwrap(), [dirs[0].clone()]);
        assert_eq!(select("far").unwrap(), [dirs[3].clone()]);
        // A repo outside the root has a ../ path
        assert_eq!(select("../../elsewhere").unwrap(), [dirs[3].clone()]);
        assert_eq!(select("./../../elsewhere").unwrap(), [dirs[3].clone()]);
        // `web` is one repo's path and another's name
        let err = select("web").unwrap_err().to_string();
        assert!(
            err.contains(
                "names the repo at web by path and the repo at frontend by .meta.yaml name"
            ),
            "{err}"
        );
    }

    #[test]
    fn a_name_shared_by_two_repos_is_ambiguous_and_relative_roots_work() {
        let root = std::path::PathBuf::from("ws-relative");
        let dirs: Vec<String> = ["api", "group/api"]
            .iter()
            .map(|rel| root.join(rel).display().to_string())
            .collect();
        let names = repo_names(
            &root,
            &[project("api", "api"), project("api", "group/api")],
            &[],
        );
        let select = |value: &str| select(&strings(&[value]), &[], &dirs, &root, &names);
        let err = select("api").unwrap_err().to_string();
        assert!(
            err.contains("is the .meta.yaml name of the repos at api, group/api"),
            "{err}"
        );
        // A ./ path picks one, with a relative root as with an absolute one
        assert_eq!(select("./api").unwrap(), [dirs[0].clone()]);
        assert_eq!(select("group/api").unwrap(), [dirs[1].clone()]);
    }
}
