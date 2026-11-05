//! The workspace a command runs in, and the repositories that make it up.
//!
//! Commands that delete worktree sets keep to this workspace's sets: those
//! whose every worktree belongs to one of its repositories. Where the sets
//! are kept (`META_WORKTREES` may be shared, or point into another
//! workspace) doesn't change that.

use meta_plugin::display::escape_path;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::safety::{relative_label, RemovalCheck};

/// A workspace's root meta directory and its repositories.
pub struct Workspace {
    pub root: PathBuf,
    /// The git directory of each repository (the root's own, then every one
    /// in its `.meta.yaml` tree, nested ones included): where the records of the
    /// worktrees made from it are.
    pub repos: Vec<PathBuf>,
    /// What couldn't be read (the project tree, a repository), with why:
    /// worktrees from there count as another workspace's.
    pub unreadable: Vec<(PathBuf, anyhow::Error)>,
    git_dirs: HashSet<PathBuf>,
    /// `root`, resolved, to compare other workspaces' roots with.
    canonical_root: PathBuf,
    /// The root of each meta directory asked about, resolved (`None`: it
    /// can't be told)
    roots: RefCell<HashMap<PathBuf, Option<PathBuf>>>,
}

impl Workspace {
    /// The workspace `home` (the folder whose sets these are) belongs to,
    /// from its outermost config, as meta reads the configs (`meta -A
    /// project list` there).
    pub fn of(home: &Path) -> anyhow::Result<Workspace> {
        let (paths, mut unreadable) = match crate::here::whole_tree(home) {
            Ok((paths, unreadable)) => (
                paths,
                unreadable
                    .into_iter()
                    .map(|(path, why)| (path, anyhow::anyhow!("{why}")))
                    .collect(),
            ),
            Err(e) => (Vec::new(), vec![(home.to_path_buf(), e)]),
        };
        // a folder with no config of its own is a workspace of itself alone
        let root = paths.first().cloned().unwrap_or_else(|| home.to_path_buf());
        let paths = match paths.is_empty() {
            true => vec![root.clone()],
            false => paths,
        };
        let mut repos = Vec::new();
        for path in paths {
            match meta_core::gitdir::common_dir(&path) {
                Ok(Some(git_dir)) => repos.push(git_dir),
                Ok(None) => {}
                Err(e) => unreadable.push((path, e)),
            }
        }
        let git_dirs = repos
            .iter()
            .filter_map(|git_dir| git_dir.canonicalize().ok())
            .collect();
        Ok(Workspace {
            canonical_root: root.canonicalize().unwrap_or_else(|_| root.clone()),
            root,
            repos,
            unreadable,
            git_dirs,
            roots: RefCell::new(HashMap::new()),
        })
    }

    /// Whether `git_dir` is the git directory of one of this workspace's
    /// repositories.
    pub fn holds(&self, git_dir: &Path) -> bool {
        git_dir
            .canonicalize()
            .is_ok_and(|git_dir| self.git_dirs.contains(&git_dir))
    }

    /// Whether `project`, the meta directory a record names, is in this
    /// workspace. Two workspaces can share a repository (one checked out
    /// with `git worktree add` from the other), so the repository alone
    /// doesn't say whose a set is. A project that is gone (or moved) leaves
    /// its sets of unknown ownership: not this workspace's, and so does
    /// one whose workspace can't be told.
    pub fn made_here(&self, project: &Path) -> bool {
        if !project.exists() {
            return false;
        }
        let root = self
            .roots
            .borrow_mut()
            .entry(project.to_path_buf())
            .or_insert_with(|| {
                let (paths, _) = crate::here::whole_tree(project).ok()?;
                let root = paths
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| project.to_path_buf());
                Some(root.canonicalize().unwrap_or(root))
            })
            .clone();
        root.as_ref() == Some(&self.canonical_root)
    }

    /// The worktrees of the set at `wt_dir` (checked as `check`) that come
    /// from repositories outside this workspace, or that another workspace
    /// made, each with the workspace its record names. A set with any is
    /// another workspace's.
    pub fn foreign_members(&self, wt_dir: &Path, check: &RemovalCheck) -> Vec<String> {
        check
            .members
            .iter()
            .filter(|m| {
                !self.holds(&m.repo_git_dir)
                    || m.record
                        .as_ref()
                        .is_some_and(|r| !self.made_here(&r.project))
            })
            .map(|m| {
                let label = relative_label(wt_dir, &m.path);
                let repo = m.repo_git_dir.canonicalize();
                let repo = escape_path(repo.as_deref().unwrap_or(&m.repo_git_dir));
                match &m.record {
                    Some(record) if !record.project.exists() => format!(
                        "{label}: the workspace at {} made it, and it is gone or was moved; \
                         remove this worktree with 'git worktree remove'",
                        escape_path(&record.project)
                    ),
                    Some(record) if self.holds(&m.repo_git_dir) => format!(
                        "{label}: the workspace at {} made it",
                        escape_path(&record.project)
                    ),
                    Some(record) => format!(
                        "{label}: its repository {repo} is not one of this workspace's; its \
                         record names the workspace at {}",
                        escape_path(&record.project)
                    ),
                    None => {
                        format!("{label}: its repository {repo} is not one of this workspace's")
                    }
                }
            })
            .collect()
    }
}
