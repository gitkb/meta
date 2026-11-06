//! Worktree lifecycle hooks: the workspace's `worktree.hooks`, run through
//! [`meta_plugin::sets::fire`] with each event's payload.

use anyhow::Result;
use meta_plugin::sets::HookEvent;
use std::collections::HashMap;
use std::path::Path;

use super::types::{CreateRepoEntry, PruneEntry};

/// Where the hooks come from: the workspace's worktree settings, as meta
/// sent them (none outside a workspace).
pub type Hooks<'a> = Option<&'a meta_plugin::Worktree>;

fn fire(hooks: Hooks<'_>, event: HookEvent, payload: &serde_json::Value) -> Result<()> {
    match hooks {
        Some(worktree) => meta_plugin::sets::fire(worktree, &event.to_string(), payload),
        None => Ok(()),
    }
}

/// Fire post-create hook with structured payload.
pub fn fire_post_create(
    name: &str,
    path: &Path,
    repos: &[CreateRepoEntry],
    ephemeral: bool,
    ttl_seconds: Option<u64>,
    custom: &HashMap<String, String>,
    hooks: Hooks<'_>,
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
    fire(hooks, HookEvent::PostCreate, &payload)
}

/// Fire post-destroy hook with structured payload.
pub fn fire_post_destroy(name: &str, path: &Path, force: bool, hooks: Hooks<'_>) -> Result<()> {
    let payload = serde_json::json!({
        "action": "destroy",
        "name": name,
        "path": path.display().to_string(),
        "force": force,
    });
    fire(hooks, HookEvent::PostDestroy, &payload)
}

/// Fire post-prune hook with structured payload.
pub fn fire_post_prune(removed: &[PruneEntry], hooks: Hooks<'_>) -> Result<()> {
    let payload = serde_json::json!({
        "action": "prune",
        "removed": removed,
    });
    fire(hooks, HookEvent::PostPrune, &payload)
}
