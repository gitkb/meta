//! The rows of `meta --json project list`: written by meta, read by a
//! plugin that asks meta for a workspace's projects (meta-git).

use crate::RepoUrl;
use serde::{Deserialize, Serialize};

/// One row of `meta --json project list`: a project, or the workspace the
/// list is of (the first row).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListedProject {
    pub name: String,
    /// From the workspace root, as the row's `project` (`.` for the root)
    pub path: String,
    /// Git remote URL. Should be present for all normal projects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<RepoUrl>,
    pub tags: Vec<String>,
    /// Whether a `.meta.yaml` is in its folder now
    pub is_meta: bool,
    /// Whether its config says it is a meta repo (`meta: true`), cloned or not
    pub meta: bool,
    /// The branch it starts from, as its config's `branch:` names it; absent
    /// for its remote's default branch (and for the first row)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Why its folder couldn't be looked into (its own projects may be
    /// missing from the list); absent when it could.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The directory the command ran from (the first row only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The worktree set it ran in (the first row only)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<ListedWorktree>,
}

/// The worktree set `meta project list` ran in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListedWorktree {
    pub linked: bool,
    pub physical_root: String,
    pub canonical_root: String,
    pub name: String,
}
