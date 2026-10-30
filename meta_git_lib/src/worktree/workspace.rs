//! The workspace a command runs in, and the repositories that make it up.
//!
//! Commands that delete worktree sets keep to this workspace's sets: those
//! whose every worktree belongs to one of its repositories. Where the sets
//! are kept (`META_WORKTREES` may be shared, or point into another
//! workspace) doesn't change that.

use anyhow::{Context, Result};
use meta_core::display::escape_path;
use std::collections::HashSet;
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
}

impl Workspace {
    /// The workspace `meta_dir` belongs to (see [`meta_core::config::workspace_root`]).
    pub fn of(meta_dir: &Path) -> Workspace {
        let root = meta_core::config::workspace_root(meta_dir);
        let mut paths = vec![root.clone()];
        let mut unreadable = Vec::new();
        match meta_core::config::walk_meta_tree(&root, None) {
            Ok(tree) => paths.extend(
                meta_core::config::build_project_map(&tree, &root, "")
                    .into_values()
                    .map(|(path, _)| path),
            ),
            Err(e) => unreadable.push((root.clone(), e)),
        }
        let mut repos = Vec::new();
        for path in paths {
            match repo_git_dir(&path) {
                Ok(Some(git_dir)) => repos.push(git_dir),
                Ok(None) => {}
                Err(e) => unreadable.push((path, e)),
            }
        }
        let git_dirs = repos
            .iter()
            .filter_map(|git_dir| git_dir.canonicalize().ok())
            .collect();
        Workspace {
            canonical_root: root.canonicalize().unwrap_or_else(|_| root.clone()),
            root,
            repos,
            unreadable,
            git_dirs,
        }
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
    /// its sets of unknown ownership: not this workspace's.
    pub fn made_here(&self, project: &Path) -> bool {
        let root = meta_core::config::workspace_root(project);
        project.exists() && root.canonicalize().unwrap_or(root) == self.canonical_root
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

/// The git directory holding the worktrees of the repository at `repo`:
/// `None` if there is no repository there (not cloned).
pub fn repo_git_dir(repo: &Path) -> Result<Option<PathBuf>> {
    let dot_git = repo.join(".git");
    // Links are followed: a `.git` linked to the real git directory is one
    let meta = match std::fs::metadata(&dot_git) {
        Ok(meta) => meta,
        // A bare repository is its own git directory
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let bare = repo.join("HEAD").is_file()
                && repo.join("objects").is_dir()
                && repo.join("refs").is_dir();
            return Ok(bare.then(|| repo.to_path_buf()));
        }
        Err(e) => {
            return Err(
                anyhow::Error::from(e).context(format!("Cannot inspect {}", escape_path(&dot_git)))
            )
        }
    };
    if meta.is_dir() {
        return Ok(Some(dot_git));
    }
    // A `.git` file: a linked worktree's (its admin directory names the
    // repository's git directory in `commondir`) or a `--separate-git-dir`
    // checkout's (it names the git directory itself, with no `commondir`)
    let content = std::fs::read_to_string(&dot_git)
        .with_context(|| format!("Cannot read {}", escape_path(&dot_git)))?;
    let target = content
        .trim()
        .strip_prefix("gitdir:")
        .map(|target| repo.join(target.trim()))
        .with_context(|| format!("{} has no gitdir line", escape_path(&dot_git)))?;
    let commondir = target.join("commondir");
    match std::fs::read_to_string(&commondir) {
        Ok(common) => Ok(Some(target.join(common.trim()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Some(target)),
        Err(e) => {
            Err(anyhow::Error::from(e).context(format!("Cannot read {}", escape_path(&commondir))))
        }
    }
}
