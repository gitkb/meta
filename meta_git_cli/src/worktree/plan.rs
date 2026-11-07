//! What a create or add puts in a worktree set: each repo, where its
//! checkout goes, and on which branch.

use crate::here::{Here, Node, Tree};
use anyhow::Result;
use meta_plugin::display::escape;
use std::path::{Path, PathBuf};

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

/// The projects of a workspace by name or by place, as meta read its
/// configs. Shallow, its keys are the names of the config's own projects
/// (a nested one is found by its place); recursive (meta's `-r`/`-A`), every
/// place of the tree, `depth` meta repos down.
pub struct Index<'a> {
    /// The config's folder: places start here
    pub dir: PathBuf,
    /// The outermost folder whose configs list the way to `dir`
    pub listed_root: PathBuf,
    /// `dir`'s place from the workspace root, with a `/` (empty at the
    /// root): what meta's names of nested projects start with
    pub prefix: String,
    tree: Tree<'a>,
    /// meta's members of the workspace, each URL as configured
    pub members: &'a [meta_plugin::Project],
    /// With `--recursive`: how deep (none: no limit)
    depth: Option<Option<usize>>,
}

impl<'a> Index<'a> {
    pub fn new(here: &Here<'a>) -> Self {
        Index {
            dir: here.dir.clone(),
            listed_root: here.workspace.listed_root.clone(),
            prefix: crate::here::place(&here.dir, &here.workspace.root)
                .map(|place| format!("{place}/"))
                .unwrap_or_default(),
            tree: here.tree(),
            members: &here.workspace.members,
            depth: None,
        }
    }

    /// The index of the whole tree under `here`'s config, `depth` deep.
    pub fn recursive(here: &Here<'a>, depth: Option<usize>) -> Self {
        Index {
            depth: Some(depth),
            ..Index::new(here)
        }
    }

    /// Whether this index is of the whole tree (its keys are places).
    pub fn is_recursive(&self) -> bool {
        self.depth.is_some()
    }

    /// The projects it keys: the config's own, or with `--recursive` every
    /// one within its depth.
    pub fn nodes(&self) -> Vec<&Node<'a>> {
        match self.depth {
            Some(depth) => self.tree.within(depth).collect(),
            None => self.tree.top().collect(),
        }
    }

    /// The key of `node` here: its name (a top-level project), else its place.
    pub fn key(&self, node: &Node<'_>) -> String {
        match self.depth.is_none() && node.level == 0 {
            true => node.project.name.clone(),
            false => node.place.clone(),
        }
    }

    /// The project `key` names: `.` is the workspace's own repo; else a
    /// project of that name at the top level, else the one at that place.
    /// With `--recursive`, a place anywhere in the tree, or the end of one
    /// (`core` for `open-source/gitkb/core`) if only one place ends so.
    pub fn find(&self, key: &str) -> Result<Found> {
        let found = |key: &str, source: &Path| Found {
            key: key.to_string(),
            source: source.to_path_buf(),
        };
        if key == "." {
            return Ok(found(".", &self.dir));
        }
        let top: Vec<&Node<'_>> = self.tree.top().collect();
        let by_name = top.iter().find(|n| n.project.name == key);
        if self.depth.is_none() {
            // a top-level project by its name, else by its path: keyed by
            // its name either way, as its dependencies name it
            let by_path = top.iter().find(|n| n.own == key);
            if let Some(n) = by_name.or(by_path) {
                return Ok(found(&n.project.name, &n.project.abs));
            }
            return match self.tree.at(key) {
                Some(n) => Ok(found(key, &n.project.abs)),
                None => Err(self.unknown(key)),
            };
        }
        if let Some(n) = by_name.filter(|n| self.at_place(&n.place).is_some()) {
            return Ok(found(&n.place, &n.project.abs));
        }
        if let Some(source) = self.at_place(key) {
            return Ok(found(key, &source));
        }
        let suffix = format!("/{key}");
        let mut ends: Vec<&Node<'_>> = self
            .nodes()
            .into_iter()
            .filter(|n| n.place.ends_with(&suffix))
            .collect();
        ends.sort_by(|a, b| a.place.cmp(&b.place));
        match ends.as_slice() {
            [n] => Ok(found(&n.place, &n.project.abs)),
            [] => Err(self.unknown(key)),
            _ => anyhow::bail!(
                "Repo '{}' is ambiguous: it ends {}. Use the whole path.",
                escape(key),
                escape(
                    &ends
                        .iter()
                        .map(|n| n.place.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            ),
        }
    }

    /// The project at `place`, if one is there (with `--recursive`, within
    /// its depth).
    pub fn at_place(&self, place: &str) -> Option<PathBuf> {
        let node = self.tree.at(place)?;
        match self.depth {
            Some(Some(depth)) if node.level > depth => None,
            _ => Some(node.project.abs.clone()),
        }
    }

    /// The project at `place`, which a project list named (with
    /// `--recursive`, every key is a place): never one whose name it is.
    pub fn place(&self, place: &str) -> Result<Found> {
        match self.at_place(place) {
            Some(source) => Ok(Found {
                key: place.to_string(),
                source,
            }),
            None => Err(self.unknown(place)),
        }
    }

    /// What the config lists, for cloning what of it isn't cloned.
    pub fn listing(&self) -> crate::here::Listing {
        self.tree.listing()
    }

    /// The node `key` (as [`Index::find`] keys it) names.
    pub fn node(&self, key: &str) -> Option<&Node<'a>> {
        self.nodes().into_iter().find(|n| self.key(n) == key)
    }

    /// The keys of the projects `key` depends on, as meta resolved them,
    /// and theirs, each once: those this index keys.
    pub fn dependencies(&self, key: &str) -> Vec<String> {
        let mut found: Vec<String> = Vec::new();
        let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        queue.push_back(key.to_string());
        while let Some(current) = queue.pop_front() {
            let Some(node) = self.node(&current) else {
                continue;
            };
            for dep in &node.project.depends_on {
                let target = self
                    .nodes()
                    .into_iter()
                    .find(|n| n.project.path == *dep)
                    .map(|n| self.key(n));
                if let Some(target) = target.filter(|t| t != key && !found.contains(t)) {
                    found.push(target.clone());
                    queue.push_back(target);
                }
            }
        }
        found
    }

    /// The `depends_on` entries of the project `key` that name no project
    /// meta found (as meta qualified them: from the workspace root).
    pub fn unresolved(&self, key: &str) -> Vec<String> {
        let Some(node) = self.node(key) else {
            return Vec::new();
        };
        node.project
            .depends_on
            .iter()
            .filter(|dep| !self.tree.nodes.iter().any(|n| n.project.path == **dep))
            .cloned()
            .collect()
    }

    /// The error for a key no project has, with the names and places there
    /// are (nested ones as far as their configs can be read).
    fn unknown(&self, key: &str) -> anyhow::Error {
        let mut valid: Vec<String> = self.tree.top().map(|n| n.project.name.clone()).collect();
        valid.extend(self.tree.nodes.iter().map(|n| n.place.clone()));
        valid.sort();
        valid.dedup();
        anyhow::anyhow!(
            "Unknown repo '{}'. Valid names and paths: {}",
            escape(key),
            escape(&valid.join(", "))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meta_plugin::Project;

    /// A workspace at `/w` with `members` (path, name, meta).
    fn workspace(members: &[(&str, &str, bool)]) -> meta_plugin::Workspace {
        let root = Path::new("/w");
        meta_plugin::Workspace {
            root: root.to_path_buf(),
            config: root.join(".meta.yaml"),
            listed_root: root.to_path_buf(),
            members: members
                .iter()
                .map(|(path, name, meta)| Project {
                    name: name.to_string(),
                    path: path.to_string(),
                    abs: root.join(path),
                    meta: *meta,
                    present: true,
                    ..Project::default()
                })
                .collect(),
        }
    }

    static WORKTREE: meta_plugin::Worktree = meta_plugin::Worktree {
        dir: None,
        refused: None,
        hooks: std::collections::BTreeMap::new(),
        home: None,
    };

    fn here(workspace: &meta_plugin::Workspace) -> Here<'_> {
        Here {
            workspace,
            worktree: &WORKTREE,
            dir: PathBuf::from("/w"),
            home: PathBuf::from("/w"),
        }
    }

    #[test]
    fn a_name_is_found_before_a_place_whatever_it_holds() {
        let ws = workspace(&[
            ("actual-api", "group/api", false),
            ("group/api", "web", false),
        ]);
        let here = here(&ws);
        let found = Index::new(&here).find("group/api").unwrap();
        assert_eq!(found.source, Path::new("/w/actual-api"));
        assert_eq!(found.key, "group/api");
        // and a place, when no project has that name, keyed by the name
        // of the project there
        let found = Index::new(&here).find("actual-api").unwrap();
        assert_eq!(found.source, Path::new("/w/actual-api"));
        assert_eq!(found.key, "group/api");
        let found = Index::recursive(&here, None).find("group/api").unwrap();
        assert_eq!(found.source, Path::new("/w/actual-api"));
        assert_eq!(found.key, "actual-api");
    }

    #[test]
    fn nested_projects_are_found_by_place() {
        let ws = workspace(&[
            ("vendor", "vendor", true),
            ("vendor/packages/mylib", "mylib", false),
            ("vendor/sub", "sub", true),
            ("vendor/sub/deep", "deep", false),
        ]);
        let here = here(&ws);
        let index = Index::new(&here);
        for place in ["vendor/packages/mylib", "vendor/sub/deep"] {
            assert_eq!(
                index.find(place).unwrap().source,
                Path::new("/w").join(place)
            );
        }
        assert!(index.find("vendor/nope").is_err());
        // a nested project's name alone is no key without --recursive
        assert!(index.find("deep").is_err());
        // nor beyond --depth
        assert!(Index::recursive(&here, Some(1))
            .find("vendor/sub/deep")
            .is_err());
    }

    #[test]
    fn recursive_finds_the_one_place_that_ends_with_a_key() {
        let ws = workspace(&[
            ("a", "a", true),
            ("a/core", "core", false),
            ("a/x", "x", false),
            ("b", "b", true),
            ("b/core", "core", false),
        ]);
        let here = here(&ws);
        let index = Index::recursive(&here, None);
        assert_eq!(index.find("x").unwrap().key, "a/x");
        let err = index.find("core").unwrap_err().to_string();
        assert!(err.contains("a/core, b/core"), "{err}");
    }

    #[test]
    fn errors_escape_the_names_they_show() {
        let ws = workspace(&[("vendor", "vendor", true)]);
        let here = here(&ws);
        for key in ["a\nFORGED", "vendor/\nFORGED", "vendor/\x1b[2J"] {
            let err = Index::new(&here).find(key).unwrap_err().to_string();
            assert!(!err.contains('\n') && !err.contains('\x1b'), "{err:?}");
        }
    }

    #[test]
    fn dependencies_follow_what_meta_resolved() {
        let mut ws = workspace(&[
            ("app", "app", false),
            ("api", "api", false),
            ("db", "db", false),
        ]);
        ws.members[0].depends_on = vec!["api".to_string(), "nowhere".to_string()];
        ws.members[1].depends_on = vec!["db".to_string()];
        let here = here(&ws);
        let index = Index::new(&here);
        assert_eq!(index.dependencies("app"), ["api", "db"]);
        assert_eq!(index.unresolved("app"), ["nowhere"]);
    }
}
