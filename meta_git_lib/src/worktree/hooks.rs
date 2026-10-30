//! Worktree lifecycle hooks.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

use super::helpers::read_meta_config_value;
use super::types::{CreateRepoEntry, PruneEntry};

/// Fire a worktree lifecycle hook if configured in `.meta.yaml`.
///
/// Reads the `.meta.yaml` config for `worktree.hooks.<hook_name>`.
/// If configured, spawns the command and pipes `payload` JSON to stdin.
/// The hook's stderr goes to meta's stderr, as git does for its hooks; a
/// pipe that nobody reads would block a verbose hook forever.
/// Hook failure prints a warning but doesn't block the operation.
pub fn fire_worktree_hook(hook_name: &str, payload: &serde_json::Value, meta_dir: Option<&Path>) {
    let dir = match meta_dir {
        Some(d) => d,
        None => return,
    };

    let config = match read_meta_config_value(dir) {
        Ok(Some(c)) => c,
        Ok(None) => return,
        Err(e) => {
            log::warn!(
                "Hook '{hook_name}' not run: {}",
                meta_core::display::escape_lines(&format!("{e:#}"))
            );
            return;
        }
    };

    let hook_cmd = config
        .get("worktree")
        .and_then(|wt| wt.get("hooks"))
        .and_then(|hooks| hooks.get(hook_name))
        .and_then(|v| v.as_str());

    let cmd_str = match hook_cmd {
        Some(c) => c,
        None => return,
    };

    let payload_json = match serde_json::to_string(payload) {
        Ok(j) => j,
        Err(_) => return,
    };

    // From the directory whose config names the hook: a set removed from
    // inside it takes the caller's directory with it
    let result = Command::new("sh")
        .args(["-c", cmd_str])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .and_then(|mut child| {
            // Write payload then drop stdin to signal EOF before waiting
            if let Some(mut stdin) = child.stdin.take() {
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
    meta_dir: Option<&Path>,
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
    fire_worktree_hook("post-create", &payload, meta_dir);
}

/// Fire post-destroy hook with structured payload.
pub fn fire_post_destroy(name: &str, path: &Path, force: bool, meta_dir: Option<&Path>) {
    let payload = serde_json::json!({
        "action": "destroy",
        "name": name,
        "path": path.display().to_string(),
        "force": force,
    });
    fire_worktree_hook("post-destroy", &payload, meta_dir);
}

/// Fire post-prune hook with structured payload.
pub fn fire_post_prune(removed: &[PruneEntry], meta_dir: Option<&Path>) {
    let payload = serde_json::json!({
        "action": "prune",
        "removed": removed,
    });
    fire_worktree_hook("post-prune", &payload, meta_dir);
}
