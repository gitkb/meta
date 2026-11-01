//! Multi-repo worktree management building blocks.
//!
//! Provides types, meta's records of the sets it created, git operations, helpers, and hooks
//! for worktree management. Command handlers live in `meta_git_cli::commands::worktree`.

pub mod git_ops;
pub mod helpers;
pub mod hooks;
pub mod lease;
pub mod record;
pub mod safety;
pub mod types;
pub mod workspace;

// Re-export commonly-used types
pub use types::RepoSpec;
