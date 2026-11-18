//! `meta_core` — The workspace domain meta and its plugins share: config,
//! repo selection, worktree sets and the dependency graph.
//!
//! Printing, exit codes, escaping and the processes meta starts are the
//! plugin SDK's (`meta_plugin`), shared with every plugin.

use std::path::PathBuf;

pub mod config;
pub mod data_dir;
pub mod dependency_graph;
pub mod git;
pub mod gitdir;
pub mod query;
pub mod repo_filter;
pub mod set_record;
pub mod store;
pub mod worktree;

/// Default meta data directory name.
const META_DIR_NAME: &str = ".meta";

/// Environment variable to override the meta data directory location.
const META_DATA_DIR_ENV: &str = "META_DATA_DIR";

/// Get the meta data directory path.
/// Respects `META_DATA_DIR` env var, otherwise defaults to `~/.meta/`.
///
/// Fails when there is no home directory, rather than falling back to a
/// shared location such as `/tmp` that other users could control.
pub fn meta_dir() -> anyhow::Result<PathBuf> {
    if let Some(override_path) = std::env::var_os(META_DATA_DIR_ENV).filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(override_path));
    }
    let home = dirs::home_dir().ok_or_else(|| {
        anyhow::anyhow!("Cannot find your home directory; set {META_DATA_DIR_ENV}")
    })?;
    Ok(home.join(META_DIR_NAME))
}

/// The user's home directory, where `meta init skills` installs.
pub fn home_dir() -> anyhow::Result<PathBuf> {
    dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot find your home directory"))
}
