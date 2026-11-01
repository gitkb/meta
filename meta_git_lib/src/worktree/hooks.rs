//! Worktree lifecycle hooks.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

use super::types::{CreateRepoEntry, PruneEntry};
use meta_core::config::Workspace;

/// Fire a worktree lifecycle hook if `owner`'s `.meta.yaml` configures one
/// (`worktree.hooks.<hook_name>`): spawn the command and pipe `payload` JSON
/// to its stdin.
/// The hook's stderr goes to meta's stderr, as git does for its hooks; a
/// pipe that nobody reads would block a verbose hook forever.
/// Hook failure prints a warning but doesn't block the operation.
pub fn fire_worktree_hook(hook_name: &str, payload: &serde_json::Value, owner: Option<&Workspace>) {
    let Some(owner) = owner else {
        return;
    };
    let Some(cmd_str) = owner.worktree.hooks.get(hook_name) else {
        return;
    };

    let payload_json = match serde_json::to_string(payload) {
        Ok(j) => j,
        Err(_) => return,
    };

    // From the directory whose config names the hook: a set removed from
    // inside it takes the caller's directory with it
    let mut command = Command::new("sh");
    meta_core::git::clear_repo_env(&mut command);
    let result = meta_core::children::spawn(
        command
            .args(["-c", cmd_str])
            .current_dir(&owner.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit()),
    )
    .and_then(|mut child| {
        // Write payload then drop stdin to signal EOF before waiting
        if let Some(mut stdin) = child.stdin() {
            use std::io::Write;
            let _ = stdin.write_all(payload_json.as_bytes());
        }
        // stdin is now dropped — child sees EOF
        child.wait()
    });

    match result {
        Ok(status) if !status.success() => {
            log::warn!("Hook '{hook_name}' exited with status {status}");
        }
        Err(e) => {
            log::warn!("Hook '{hook_name}' failed to execute: {e}");
        }
        _ => {}
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
) {
    let payload = serde_json::json!({
        "action": "create",
        "name": name,
        "path": path.display().to_string(),
        "repos": repos,
        "ephemeral": ephemeral,
        "ttl_seconds": ttl_seconds,
        "custom": custom,
    });
    fire_worktree_hook("post-create", &payload, owner);
}

/// Fire post-destroy hook with structured payload.
pub fn fire_post_destroy(name: &str, path: &Path, force: bool, owner: Option<&Workspace>) {
    let payload = serde_json::json!({
        "action": "destroy",
        "name": name,
        "path": path.display().to_string(),
        "force": force,
    });
    fire_worktree_hook("post-destroy", &payload, owner);
}

/// Fire post-prune hook with structured payload.
pub fn fire_post_prune(removed: &[PruneEntry], owner: Option<&Workspace>) {
    let payload = serde_json::json!({
        "action": "prune",
        "removed": removed,
    });
    fire_worktree_hook("post-prune", &payload, owner);
}
