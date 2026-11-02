//! What a create or add puts in a worktree set: each repo, where its
//! checkout goes, and on which branch.

use anyhow::Result;
use meta_core::config::Workspace;
use meta_core::display::escape;
use std::collections::HashMap;
use std::path::PathBuf;

/// One repo of a set being made.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedRepo {
    /// The project, as the index knows it (`.` for the workspace's own repo).
    pub project: String,
    /// Its checkout in the workspace.
    pub source: PathBuf,
    /// Where it goes in the set, from the set's root (`.` for the root).
    pub place: String,
    pub branch: String,
}

/// A project the index found: its key and its checkout.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// The project's name at the top level; a nested project's place from
    /// the workspace; with `--recursive`, every project's place.
    pub key: String,
    pub source: PathBuf,
}

/// The projects of a workspace by name or by place. Shallow, it reads only
/// the configs of the meta repos on the way to a place it is asked about;
/// recursive, it walks the whole tree, once.
pub struct Index<'a> {
    pub workspace: &'a Workspace,
    /// Every project of the tree by its place, with `--recursive`.
    tree: Option<HashMap<String, (PathBuf, meta_core::config::ProjectInfo)>>,
}

impl<'a> Index<'a> {
    pub fn new(workspace: &'a Workspace) -> Self {
        Index {
            workspace,
            tree: None,
        }
    }

    /// The index of the whole tree under `workspace`.
    pub fn recursive(workspace: &'a Workspace) -> Result<Self> {
        let nodes = meta_core::config::walk_projects(&workspace.dir, &workspace.projects, None)?;
        let tree = meta_core::config::build_project_map(&nodes, &workspace.dir, "");
        Ok(Index {
            workspace,
            tree: Some(tree),
        })
    }

    /// Every project of the tree by its place, if this index walked it.
    pub fn tree(&self) -> Option<&HashMap<String, (PathBuf, meta_core::config::ProjectInfo)>> {
        self.tree.as_ref()
    }

    /// The project `key` names: `.` is the workspace's own repo; else a
    /// project of that name at the top level, else the one at that place.
    /// With `--recursive`, a place anywhere in the tree, or the end of one
    /// (`core` for `open-source/gitkb/core`) if only one place ends so.
    pub fn find(&self, key: &str) -> Result<Found> {
        let found = |key: &str, source: PathBuf| Found {
            key: key.to_string(),
            source,
        };
        if key == "." {
            return Ok(found(".", self.workspace.dir.clone()));
        }
        let Some(tree) = &self.tree else {
            // a top-level project by its name, else by its path: keyed by
            // its name either way, as its dependencies name it
            let projects = &self.workspace.projects;
            let top = projects.iter().find(|p| p.name == key);
            if let Some(p) = top.or_else(|| projects.iter().find(|p| p.path == key)) {
                return Ok(found(&p.name, self.workspace.dir.join(&p.path)));
            }
            return match project_at(self.workspace, key)? {
                Some(source) => Ok(found(key, source)),
                None => Err(self.unknown(key)),
            };
        };
        if let Some(p) = self.workspace.projects.iter().find(|p| p.name == key) {
            let place = p.path.as_str();
            if let Some((source, _)) = tree.get(place) {
                return Ok(found(place, source.clone()));
            }
        }
        if let Some((source, _)) = tree.get(key) {
            return Ok(found(key, source.clone()));
        }
        let suffix = format!("/{key}");
        let mut ends: Vec<(&String, &PathBuf)> = tree
            .iter()
            .filter(|(place, _)| place.ends_with(&suffix))
            .map(|(place, (source, _))| (place, source))
            .collect();
        ends.sort();
        match ends.as_slice() {
            [(place, source)] => Ok(found(place, source.to_path_buf())),
            [] => Err(self.unknown(key)),
            _ => anyhow::bail!(
                "Repo '{}' is ambiguous: it ends {}. Use the whole path.",
                escape(key),
                escape(
                    &ends
                        .iter()
                        .map(|(place, _)| place.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            ),
        }
    }

    /// The project at `place`, if one is there.
    pub fn at_place(&self, place: &str) -> Result<Option<PathBuf>> {
        match &self.tree {
            Some(tree) => Ok(tree.get(place).map(|(source, _)| source.clone())),
            None => project_at(self.workspace, place),
        }
    }

    /// The project at `place`, which a project list named (with
    /// `--recursive`, every key is a place): never one whose name it is.
    pub fn place(&self, place: &str) -> Result<Found> {
        match self.at_place(place)? {
            Some(source) => Ok(Found {
                key: place.to_string(),
                source,
            }),
            None => Err(self.unknown(place)),
        }
    }

    /// The error for a key no project has, with the names and places there
    /// are (nested ones as far as their configs can be read).
    fn unknown(&self, key: &str) -> anyhow::Error {
        let mut valid: Vec<String> = self
            .workspace
            .projects
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let walked;
        let tree = match &self.tree {
            Some(tree) => Some(tree),
            None => {
                walked = Index::recursive(self.workspace).ok();
                walked.as_ref().and_then(|index| index.tree.as_ref())
            }
        };
        let tree = tree.map(|tree| tree.keys().cloned().collect::<Vec<_>>());
        valid.extend(tree.into_iter().flatten());
        valid.sort();
        valid.dedup();
        anyhow::anyhow!(
            "Unknown repo '{}'. Valid names and paths: {}",
            escape(key),
            escape(&valid.join(", "))
        )
    }
}

/// The checkout of the project at `place` in `workspace`, reading only the
/// configs of the meta repos on the way to it.
fn project_at(workspace: &Workspace, place: &str) -> Result<Option<PathBuf>> {
    for project in &workspace.projects {
        let path = workspace.dir.join(&project.path);
        if project.path == place {
            return Ok(Some(path));
        }
        if let Some(rest) = place.strip_prefix(&format!("{}/", project.path)) {
            if let Some(nested) = Workspace::in_dir(&path)? {
                if let Some(found) = project_at(&nested, rest)? {
                    return Ok(Some(found));
                }
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meta_test_support::Sandbox;
    use std::path::Path;

    fn workspace(dir: &Path) -> Workspace {
        Workspace::in_dir(dir).unwrap().unwrap()
    }

    fn write(path: &Path, config: &str) {
        std::fs::create_dir_all(path).unwrap();
        std::fs::write(path.join(".meta.yaml"), config).unwrap();
    }

    #[test]
    fn a_name_is_found_before_a_place_whatever_it_holds() {
        let sandbox = Sandbox::new();
        let tmp = sandbox.work();
        write(
            &tmp,
            r#"{"projects": {"group/api": {"repo": "u", "path": "actual-api"},
                             "web": {"repo": "u", "path": "group/api"}}}"#,
        );
        let ws = workspace(&tmp);
        let found = Index::new(&ws).find("group/api").unwrap();
        assert_eq!(found.source, tmp.join("actual-api"));
        assert_eq!(found.key, "group/api");
        // and a place, when no project has that name, keyed by the name
        // of the project there
        let found = Index::new(&ws).find("actual-api").unwrap();
        assert_eq!(found.source, tmp.join("actual-api"));
        assert_eq!(found.key, "group/api");
        let found = Index::recursive(&ws).unwrap().find("group/api").unwrap();
        assert_eq!(found.source, tmp.join("actual-api"));
        assert_eq!(found.key, "actual-api");
    }

    #[test]
    fn nested_projects_are_found_by_place_reading_only_the_way_there() {
        let sandbox = Sandbox::new();
        let tmp = sandbox.work();
        write(
            &tmp,
            r#"{"projects": {"vendor": {"repo": "u", "meta": true},
                             "broken": {"repo": "u", "meta": true}}}"#,
        );
        write(
            &tmp.join("vendor"),
            r#"{"projects": {"mylib": {"repo": "u", "path": "packages/mylib"},
                             "sub": {"repo": "u", "meta": true}}}"#,
        );
        write(&tmp.join("vendor/sub"), r#"{"projects": {"deep": "u"}}"#);
        write(&tmp.join("broken"), "{not yaml");
        let ws = workspace(&tmp);
        let index = Index::new(&ws);
        for (place, source) in [
            ("vendor/packages/mylib", "vendor/packages/mylib"),
            ("vendor/sub/deep", "vendor/sub/deep"),
        ] {
            assert_eq!(index.find(place).unwrap().source, tmp.join(source));
        }
        assert!(index.find("vendor/nope").is_err());
        // a nested project's name alone is no key without --recursive
        assert!(index.find("deep").is_err());
    }

    #[test]
    fn recursive_finds_the_one_place_that_ends_with_a_key() {
        let sandbox = Sandbox::new();
        let tmp = sandbox.work();
        write(
            &tmp,
            r#"{"projects": {"a": {"repo": "u", "meta": true}, "b": {"repo": "u", "meta": true}}}"#,
        );
        write(&tmp.join("a"), r#"{"projects": {"core": "u", "x": "u"}}"#);
        write(&tmp.join("b"), r#"{"projects": {"core": "u"}}"#);
        let ws = workspace(&tmp);
        let index = Index::recursive(&ws).unwrap();
        assert_eq!(index.find("x").unwrap().key, "a/x");
        let err = index.find("core").unwrap_err().to_string();
        assert!(err.contains("a/core, b/core"), "{err}");
    }

    #[test]
    fn errors_escape_the_names_they_show() {
        let sandbox = Sandbox::new();
        let tmp = sandbox.work();
        write(
            &tmp,
            r#"{"projects": {"vendor": {"repo": "u", "meta": true}}}"#,
        );
        let ws = workspace(&tmp);
        for key in ["a\nFORGED", "vendor/\nFORGED", "vendor/\x1b[2J"] {
            let err = Index::new(&ws).find(key).unwrap_err().to_string();
            assert!(!err.contains('\n') && !err.contains('\x1b'), "{err:?}");
        }
    }
}
