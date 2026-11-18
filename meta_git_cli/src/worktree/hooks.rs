//! Worktree lifecycle hooks: the workspace's `worktree.hooks`, run through
//! [`meta_plugin::sets::fire`] with each event's payload.

use anyhow::Result;
use meta_plugin::sets::fire;
use meta_plugin::Worktree;
use std::path::Path;

use super::types::PruneEntry;

/// Fire post-create hook with structured payload.
pub fn fire_post_create(
    name: &str,
    path: &Path,
    repos: &[CreateRepoEntry],
    ephemeral: bool,
    ttl_seconds: Option<u64>,
    custom: &HashMap<String, String>,
    hooks: &Worktree,
) -> Result<()> {
    let payload = serde_json::json!({
        "action": "create",
        "name": name,
        "path": path.display().to_string(),
        "repos": repos,
        "ephemeral": ephemeral,
        "ttl_seconds": ttl_seconds,
        "custom": custom,
    });
    fire(hooks, "post-create", &payload)
}

/// Fire post-destroy hook with structured payload.
pub fn fire_post_destroy(name: &str, path: &Path, force: bool, hooks: &Worktree) -> Result<()> {
    let payload = serde_json::json!({
        "action": "destroy",
        "name": name,
        "path": path.display().to_string(),
        "force": force,
    });
    fire(hooks, "post-destroy", &payload)
}

/// Fire post-prune hook with structured payload.
pub fn fire_post_prune(removed: &[PruneEntry], hooks: &Worktree) -> Result<()> {
    let payload = serde_json::json!({
        "action": "prune",
        "removed": removed,
    });
    fire(hooks, "post-prune", &payload)
}
