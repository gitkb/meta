//! Worktree lifecycle hooks.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use super::types::{CreateRepoEntry, PruneEntry};
use meta_core::config::{HookEvent, Workspace};

/// Run the hook `owner`'s `.meta.yaml` configures for `event`
/// (`worktree.hooks.<event>`), if any: spawn the command and pipe `payload`
/// JSON to its stdin.
/// The hook's stderr goes to meta's stderr, as git does for its hooks; a
/// pipe that nobody reads would block a verbose hook forever.
/// A hook that fails, or can't start or be sent its payload, is an error
/// naming its event (not its command, which may hold a secret); what it
/// followed is done either way. A hook that doesn't read its payload is no
/// failure.
pub fn fire_worktree_hook(
    event: HookEvent,
    payload: &serde_json::Value,
    owner: Option<&Workspace>,
) -> Result<()> {
    let Some(owner) = owner else {
        return Ok(());
    };
    let Some(cmd_str) = owner.worktree.hooks.get(&event) else {
        return Ok(());
    };
    let hook = format!("The {} hook", event.name());

    // From the directory whose config names the hook: a set removed from
    // inside it takes the caller's directory with it
    let mut command = meta_core::git::in_repo("sh", &owner.dir);
    let status = meta_core::children::spawn(
        command
            .args(["-c", cmd_str])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit()),
    )
    .and_then(|child| child.exchange(payload.to_string().as_bytes()))
    .with_context(|| format!("{hook} could not run, or be sent its payload"))?
    .status;
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        anyhow::bail!("{hook} was killed by signal {signal}");
    }
    match status.code() {
        Some(0) => Ok(()),
        Some(code) => anyhow::bail!("{hook} exited with code {code}"),
        None => anyhow::bail!("{hook} ended without an exit code ({status})"),
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
    owner: Option<&Workspace>,
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
    fire_worktree_hook(HookEvent::PostCreate, &payload, owner)
}

/// Fire post-destroy hook with structured payload.
pub fn fire_post_destroy(
    name: &str,
    path: &Path,
    force: bool,
    owner: Option<&Workspace>,
) -> Result<()> {
    let payload = serde_json::json!({
        "action": "destroy",
        "name": name,
        "path": path.display().to_string(),
        "force": force,
    });
    fire_worktree_hook(HookEvent::PostDestroy, &payload, owner)
}

/// Fire post-prune hook with structured payload.
pub fn fire_post_prune(removed: &[PruneEntry], owner: Option<&Workspace>) -> Result<()> {
    let payload = serde_json::json!({
        "action": "prune",
        "removed": removed,
    });
    fire_worktree_hook(HookEvent::PostPrune, &payload, owner)
}
