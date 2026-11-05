//! Multi-repo worktree management building blocks.
//!
//! Provides types, git operations, helpers, and hooks
//! for worktree management. Command handlers live in `meta_git_cli::commands::worktree`.

pub mod git_ops;
pub mod helpers;
pub mod hooks;
pub mod lease;
pub mod plan;
pub mod safety;
pub mod ttl;
pub mod types;
pub mod workspace;

// Re-export commonly-used types
pub use types::RepoSpec;
