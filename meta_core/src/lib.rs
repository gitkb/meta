//! `meta_core` — Shared infrastructure for `~/.meta/` directory management.
//!
//! Provides:
//! - `data_dir` — Locate and create the `~/.meta/` data directory and namespaced files
//! - `display` — Escaping untrusted text for human-readable output
//! - `store` — Atomic JSON read/write

use std::path::PathBuf;

pub mod args;
pub mod children;
pub mod config;
pub mod data_dir;
pub mod display;
pub mod duration;
pub mod git;
pub mod gitdir;
pub mod mode;
pub mod set_record;
pub use mode::ExecutionMode;
pub mod store;

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
