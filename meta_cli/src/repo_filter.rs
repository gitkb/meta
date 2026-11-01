//! `--include`, `--exclude` and `--tag`: which repos a command runs in.

use anyhow::{bail, Result};
use meta_core::config::{MetaTreeNode, ProjectInfo};
use meta_core::display::{escape, escape_path};
use std::path::{Component, Path, PathBuf};

/// What `--include`, `--exclude` and `--tag` ask for.
#[derive(Debug, Default, Clone)]
pub struct Filters {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub tags: Vec<String>,
}

impl Filters {
    /// The filters as typed: `tag` is a comma list, and a blank entry in it
    /// is no tag.
    pub fn new(
        include: Option<Vec<String>>,
        exclude: Option<Vec<String>>,
        tag: Option<&str>,
    ) -> Self {
        Self {
            include: include.unwrap_or_default(),
            exclude: exclude.unwrap_or_default(),
            tags: tag
                .into_iter()
                .flat_map(|tags| tags.split(','))
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .map(str::to_string)
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty() && self.tags.is_empty()
    }
}

/// A repo a command may run in: its directory, and the `.meta.yaml` name and
/// tags of its project (none for the workspace root).
#[derive(Debug, Clone)]
pub struct Candidate {
    pub dir: PathBuf,
    pub name: Option<String>,
    pub tags: Vec<String>,
}

impl Candidate {
    /// The project `info` at `dir`
    pub fn project(dir: PathBuf, info: &ProjectInfo) -> Self {
        Self {
            dir,
            name: Some(info.name.clone()),
            tags: info.tags.clone(),
        }
    }

    /// `dir` as a repo of no project: the root of a workspace or a set
    pub fn root(dir: PathBuf) -> Self {
        Self {
            dir,
            name: None,
            tags: vec![],
        }
    }
}

/// The repos a command may run in, from the workspace or set at `root`.
#[derive(Debug, Clone)]
pub struct Scope {
    pub root: PathBuf,
    /// The `.meta.yaml` they come from, named in errors
    pub config: Option<PathBuf>,
    pub repos: Vec<Candidate>,
    /// Every tag of the projects in the configs read, whether or not their
    /// repo is one of `repos`: `--tag` with another is a typo
    pub tags: Vec<String>,
    /// The configs of nested meta repos were read too
    pub nested: bool,
}

impl Scope {
    /// The workspace at `root` (whose config is `config`): the root itself,
    /// then each of `projects`, or with a walked `tree` (`--recursive`)
    /// every project in it, nested ones under their parents, each with its
    /// own tags.
    pub fn workspace(
        root: &Path,
        config: Option<&Path>,
        projects: &[ProjectInfo],
        tree: &[MetaTreeNode],
    ) -> Self {
        fn walk(nodes: &[MetaTreeNode], dir: &Path, out: &mut Vec<Candidate>) {
            for node in nodes {
                let path = dir.join(&node.info.path);
                out.push(Candidate::project(path.clone(), &node.info));
                walk(&node.children, &path, out);
            }
        }
        let mut repos = vec![Candidate::root(root.to_path_buf())];
        if tree.is_empty() {
            repos.extend(
                projects
                    .iter()
                    .map(|p| Candidate::project(root.join(&p.path), p)),
            );
        } else {
            walk(tree, root, &mut repos);
        }
        let tags = repos.iter().flat_map(|r| r.tags.clone()).collect();
        Self {
            root: root.to_path_buf(),
            config: config.map(Path::to_path_buf),
            repos,
            tags,
            nested: !tree.is_empty(),
        }
    }

    /// The directories of the repos that `filters` select, in order.
    ///
    /// With no `--include` and no `--tag`, every repo; otherwise each repo
    /// an `--include` value names, and each repo that has one of the `--tag`
    /// tags. Then `--exclude` leaves out the repos it names. The root has no
    /// tags, so `--tag` alone never selects it; `--include .` adds it.
    ///
    /// A value names a repo by its `.meta.yaml` name or by its whole path
    /// from `root` (`.` for the root; a trailing `/` is ignored), never by a
    /// part of either. A value starting with `./` is a path only. Repos are
    /// compared as directories, so excluding one excludes every spelling of
    /// it (`api`, `./api` and `api/`). A value that names none of the repos
    /// is an error, so a typo never runs a command in a repo it was meant to
    /// leave out; so is a tag no project has, a value that names several
    /// repos, and a selection that leaves no repo.
    pub fn select(self, filters: &Filters) -> Result<Vec<PathBuf>> {
        let Scope {
            root,
            config,
            repos,
            tags: known,
            nested,
        } = self;
        let (root, config) = (root.as_path(), config.as_deref());
        if filters.is_empty() {
            return Ok(repos.into_iter().map(|r| r.dir).collect());
        }
        let repos: Vec<Repo> = repos
            .into_iter()
            .map(|candidate| Repo {
                path: repo_path(&candidate.dir, root),
                id: std::fs::canonicalize(&candidate.dir).unwrap_or_else(|_| candidate.dir.clone()),
                candidate,
            })
            .collect();
        let read_from = config
            .map(|config| format!(" (from {})", escape_path(config)))
            .unwrap_or_default();
        // A path is the repo's, or leads there through a link
        let names_dir = |path: &str, repo: &Repo| {
            *path == repo.path
                || std::fs::canonicalize(root.join(path)).is_ok_and(|id| id == repo.id)
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
                distinct(
                    repos
                        .iter()
                        .filter(|r| r.candidate.name.as_deref() == Some(value)),
                )
            };
            let listed = |repos: &[&Repo]| -> String {
                repos
                    .iter()
                    .map(|r| escape(&r.path))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let value = escape(value);
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
                    "{flag} '{value}' names none of the repos this command runs in{read_from}. Use a \
                     repo's name from .meta.yaml or its path from the workspace root (`.` for the \
                     root), as `meta project list` shows it; in a worktree set, the same, from the \
                     set's root. --recursive adds the repos of nested meta repos."
                );
            }
            Ok(hits.iter().map(|r| r.id.as_path()).collect())
        };
        let mut chosen = Vec::new();
        for value in &filters.include {
            chosen.extend(named("--include", value)?);
        }
        for tag in &filters.tags {
            if !known.contains(tag) {
                let mut known: Vec<&str> = known.iter().map(String::as_str).collect();
                known.sort_unstable();
                known.dedup();
                let known = match known.as_slice() {
                    [] => "no project has a tag".to_string(),
                    known => format!(
                        "the tags are {}",
                        known
                            .iter()
                            .map(|t| escape(t))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
                let hint = if nested {
                    ""
                } else {
                    " --recursive reads the tags of nested meta repos too."
                };
                bail!(
                    "--tag '{}' is no project's tag{read_from}: {known}.{hint}",
                    escape(tag)
                );
            }
            chosen.extend(
                repos
                    .iter()
                    .filter(|r| r.candidate.tags.contains(tag))
                    .map(|r| r.id.as_path()),
            );
        }
        let mut excluded = Vec::new();
        for value in &filters.exclude {
            excluded.extend(named("--exclude", value)?);
        }
        let everything = filters.include.is_empty() && filters.tags.is_empty();
        let selected: Vec<PathBuf> = repos
            .iter()
            .filter(|r| everything || chosen.contains(&r.id.as_path()))
            .filter(|r| !excluded.contains(&r.id.as_path()))
            .map(|r| r.candidate.dir.clone())
            .collect();
        if selected.is_empty() {
            bail!("--include/--exclude/--tag leave no repo to run in{read_from}");
        }
        Ok(selected)
    }
}

/// A repo `select` chooses among: as given, as filters name it, and the
/// directory it is.
struct Repo {
    candidate: Candidate,
    path: String,
    id: PathBuf,
}

/// `repos` with each directory once.
fn distinct<'a>(repos: impl Iterator<Item = &'a Repo>) -> Vec<&'a Repo> {
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

    fn project(name: &str, path: &str, tags: &[&str]) -> ProjectInfo {
        ProjectInfo {
            name: name.to_string(),
            path: path.to_string(),
            repo: None,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            provides: vec![],
            depends_on: vec![],
            meta: false,
        }
    }

    fn filters(include: &[&str], exclude: &[&str], tag: Option<&str>) -> Filters {
        let list = |values: &[&str]| Some(values.iter().map(|s| s.to_string()).collect());
        Filters::new(list(include), list(exclude), tag)
    }

    /// The paths from `root` (`.` for it) of the repos `filters` select
    fn picked(
        root: &Path,
        repos: &[Candidate],
        filters: &Filters,
        config: Option<&Path>,
    ) -> Result<Vec<String>> {
        let scope = Scope {
            root: root.to_path_buf(),
            config: config.map(Path::to_path_buf),
            tags: repos.iter().flat_map(|r| r.tags.clone()).collect(),
            repos: repos.to_vec(),
            nested: false,
        };
        Ok(scope
            .select(filters)?
            .iter()
            .map(|dir| repo_path(dir, root))
            .collect())
    }

    #[test]
    fn values_name_whole_repos_never_parts_of_paths() {
        let root = std::env::temp_dir().join("ws").join("a");
        let projects = [
            project("api", "api", &[]),
            project("api-legacy", "api-legacy", &[]),
            project("core-lib", "./libs/core/", &[]),
        ];
        let repos = Scope::workspace(&root, None, &projects, &[]).repos;
        let pick = |include: &[&str], exclude: &[&str]| {
            picked(&root, &repos, &filters(include, exclude, None), None)
        };

        assert_eq!(pick(&["api"], &[]).unwrap(), ["api"]);
        assert_eq!(pick(&["./api/"], &[]).unwrap(), ["api"]);
        assert_eq!(pick(&["."], &[]).unwrap(), ["."]);
        assert_eq!(pick(&["libs/core"], &[]).unwrap(), ["libs/core"]);
        assert_eq!(pick(&["core-lib"], &[]).unwrap(), ["libs/core"]);
        // `./` makes a value a path only
        assert_eq!(pick(&["./libs/core/"], &[]).unwrap(), ["libs/core"]);
        assert!(pick(&["./core-lib"], &[]).is_err());
        assert_eq!(
            pick(&[], &["api"]).unwrap(),
            [".", "api-legacy", "libs/core"]
        );
        assert_eq!(
            pick(&["api", "api-legacy"], &["api"]).unwrap(),
            ["api-legacy"]
        );
        assert_eq!(
            pick(&[], &[]).unwrap(),
            [".", "api", "api-legacy", "libs/core"]
        );
        // Parts of a name or of the absolute path name nothing
        let absolute = root.join("api").display().to_string();
        for value in ["a", "ap", "core", "ws", absolute.as_str(), ""] {
            let err = pick(&[value], &[]).unwrap_err().to_string();
            assert!(err.contains("names none of the repos"), "{value}: {err}");
            let err = pick(&[], &[value]).unwrap_err().to_string();
            assert!(err.contains("--exclude"), "{value}: {err}");
        }
        let err = pick(&["api"], &["api"]).unwrap_err().to_string();
        assert!(err.contains("leave no repo"), "{err}");
    }

    #[test]
    fn tags_select_tagged_repos_never_the_root_and_include_adds_to_them() {
        let root = std::env::temp_dir().join("ws").join("a");
        let projects = [
            project("api", "api", &["backend"]),
            project("worker", "worker", &["backend", "jobs"]),
            project("web", "web", &["frontend"]),
        ];
        let repos = Scope::workspace(&root, None, &projects, &[]).repos;
        let pick = |include: &[&str], exclude: &[&str], tag: &str| {
            picked(&root, &repos, &filters(include, exclude, Some(tag)), None)
        };
        assert_eq!(pick(&[], &[], "backend").unwrap(), ["api", "worker"]);
        assert_eq!(pick(&[], &[], "jobs, frontend").unwrap(), ["worker", "web"]);
        // `-i .` adds the root, `-i web` adds a repo without the tag
        assert_eq!(
            pick(&["."], &[], "backend").unwrap(),
            [".", "api", "worker"]
        );
        assert_eq!(pick(&["web"], &[], "jobs").unwrap(), ["worker", "web"]);
        assert_eq!(pick(&[], &["api"], "backend").unwrap(), ["worker"]);
        // A blank tag list is no filter
        assert_eq!(pick(&[], &[], " , ").unwrap().len(), 4);

        let config = root.join(".meta.yaml");
        let err = picked(
            &root,
            &repos,
            &filters(&[], &[], Some("backend,backnd")),
            Some(&config),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("--tag 'backnd' is no project's tag"), "{err}");
        assert!(
            err.contains("the tags are backend, frontend, jobs"),
            "{err}"
        );
        assert!(err.contains(&config.display().to_string()), "{err}");
        let err = pick(&[], &["api", "worker"], "backend").unwrap_err();
        assert!(err.to_string().contains("leave no repo"), "{err}");
        let err = picked(&root, &repos[..1], &filters(&[], &[], Some("x")), None);
        assert!(err
            .unwrap_err()
            .to_string()
            .contains("no project has a tag"));
        // A real tag of no repo in reach (a set without `web`) selects nothing
        let set = Scope {
            root: root.clone(),
            config: None,
            repos: repos[..3].to_vec(),
            tags: repos.iter().flat_map(|r| r.tags.clone()).collect(),
            nested: true,
        };
        let set_pick = |tag: &str| {
            set.clone()
                .select(&filters(&[], &[], Some(tag)))
                .map(|dirs| dirs.iter().map(|d| repo_path(d, &root)).collect::<Vec<_>>())
        };
        assert_eq!(set_pick("jobs,frontend").unwrap(), ["worker"]);
        let err = set_pick("frontend").unwrap_err().to_string();
        assert!(err.contains("leave no repo"), "{err}");
    }

    #[test]
    fn nested_projects_match_on_their_own_tags_and_names() {
        let root = std::env::temp_dir().join("ws").join("a");
        let node = |info: ProjectInfo, children| MetaTreeNode {
            info,
            is_meta: true,
            children,
        };
        // `cli` is tagged under a parent that isn't
        let tree = [node(
            project("tools", "tools", &[]),
            vec![node(project("cli", "./cli", &["rust"]), vec![])],
        )];
        let repos = Scope::workspace(&root, None, &[], &tree).repos;
        let pick = |include: &[&str], tag: Option<&str>| {
            picked(&root, &repos, &filters(include, &[], tag), None).unwrap()
        };
        assert_eq!(pick(&[], Some("rust")), ["tools/cli"]);
        assert_eq!(pick(&["tools"], None), ["tools"]);
        assert_eq!(pick(&["cli"], None), ["tools/cli"]);
        assert_eq!(pick(&[], None), [".", "tools", "tools/cli"]);
    }

    #[test]
    fn errors_name_the_config_they_read() {
        let root = std::env::temp_dir().join("ws").join("a");
        let config = root.join("infra").join(".meta.yaml");
        let repos = Scope::workspace(&root, None, &[project("tf", "tf", &[])], &[]).repos;
        let err = picked(&root, &repos, &filters(&["api"], &[], None), Some(&config))
            .unwrap_err()
            .to_string();
        assert!(err.contains("names none of the repos"), "{err}");
        assert!(err.contains(&config.display().to_string()), "{err}");
    }

    #[test]
    fn repos_are_directories_not_spellings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("ws");
        for dir in ["api", "libs", "child"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let repos = vec![
            Candidate::root(root.clone()),
            Candidate::root(root.join("api")),
            Candidate::project(
                root.join("libs/../api"),
                &project("outer", "libs/../api", &[]),
            ),
            Candidate::root(root.join("child/..")),
        ];
        let spellings = ["", "api", "libs/../api", "child/.."];
        let pick = |include: &[&str], exclude: &[&str]| -> Vec<&str> {
            let scope = Scope {
                root: root.clone(),
                config: None,
                repos: repos.clone(),
                tags: vec![],
                nested: false,
            };
            let chosen = scope.select(&filters(include, exclude, None)).unwrap();
            spellings
                .iter()
                .zip(&repos)
                .filter(|(_, repo)| chosen.contains(&repo.dir))
                .map(|(rel, _)| *rel)
                .collect()
        };
        // Excluding a repo excludes every spelling of its directory
        assert_eq!(pick(&[], &["api"]), ["", "child/.."]);
        assert_eq!(pick(&[], &["."]), ["api", "libs/../api"]);
        assert_eq!(pick(&["outer"], &[]), ["api", "libs/../api"]);
        // `..` is kept: the workspace's parent is not the workspace
        let up = Scope::workspace(&root, None, &[project("parent", "..", &[])], &[]).repos;
        let scope = Scope {
            root: root.clone(),
            config: None,
            repos: up[1..].to_vec(),
            tags: vec![],
            nested: false,
        };
        let err = scope.select(&filters(&["."], &[], None)).unwrap_err();
        assert!(err.to_string().contains("names none"), "{err}");
    }

    #[test]
    fn absolute_paths_and_ambiguous_names() {
        let root = std::env::temp_dir().join("ws").join("a");
        let outside = std::env::temp_dir().join("elsewhere");
        let absolute = |dir: &Path| dir.display().to_string();
        let projects = [
            project("core-lib", &absolute(&root.join("libs/core")), &[]),
            project("frontend-web", "web", &[]),
            project("web", "frontend", &[]),
            project("far", &absolute(&outside), &[]),
        ];
        let repos: Vec<Candidate> = projects
            .iter()
            .map(|p| Candidate::project(root.join(&p.path), p))
            .collect();
        let pick = |value: &str| picked(&root, &repos, &filters(&[value], &[], None), None);
        assert_eq!(pick("core-lib").unwrap(), ["libs/core"]);
        assert_eq!(pick("far").unwrap(), ["../../elsewhere"]);
        // A repo outside the root has a ../ path
        assert_eq!(pick("../../elsewhere").unwrap(), ["../../elsewhere"]);
        assert_eq!(pick("./../../elsewhere").unwrap(), ["../../elsewhere"]);
        // `web` is one repo's path and another's name
        let err = pick("web").unwrap_err().to_string();
        assert!(
            err.contains(
                "names the repo at web by path and the repo at frontend by .meta.yaml name"
            ),
            "{err}"
        );
    }

    #[test]
    fn a_name_shared_by_two_repos_is_ambiguous_and_relative_roots_work() {
        let root = PathBuf::from("ws-relative");
        let projects = [project("api", "api", &[]), project("api", "group/api", &[])];
        let repos = Scope::workspace(&root, None, &projects, &[]).repos;
        let pick = |value: &str| picked(&root, &repos, &filters(&[value], &[], None), None);
        let err = pick("api").unwrap_err().to_string();
        assert!(
            err.contains("is the .meta.yaml name of the repos at api, group/api"),
            "{err}"
        );
        // A ./ path picks one, with a relative root as with an absolute one
        assert_eq!(pick("./api").unwrap(), ["api"]);
        assert_eq!(pick("group/api").unwrap(), ["group/api"]);
    }
}
