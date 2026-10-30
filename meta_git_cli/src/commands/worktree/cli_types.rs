//! CLI argument types for worktree subcommands.
//!
//! These are clap-derived types that belong in the CLI crate, not the library.

use clap::{Args, Subcommand};
use meta_git_lib::worktree::RepoSpec;

/// Worktree subcommands parsed by clap.
#[derive(Subcommand)]
pub enum WorktreeCommands {
    /// Create a new worktree set
    Create(CreateArgs),
    /// Add a repo to an existing worktree set
    Add(AddArgs),
    /// Remove a worktree set
    Remove(DestroyArgs),
    /// Remove a worktree set (alias for remove)
    #[command(hide = true)]
    Destroy(DestroyArgs),
    /// List worktree sets (defaults to the current set when inside one)
    List(ListArgs),
    /// Show detailed status of a worktree set
    Status(StatusArgs),
    /// Show cross-repo diff vs base branch
    Diff(DiffArgs),
    /// Run a command across worktree repos
    Exec(ExecArgs),
    /// Remove expired/orphaned worktree sets of this workspace
    Prune(PruneArgs),
    #[command(external_subcommand)]
    Unknown(Vec<String>),
}

#[derive(Args)]
pub struct CreateArgs {
    // NOTE: Positional arg order matters — name then commit_ish must stay in this order.
    /// Worktree name
    pub name: String,

    /// Start from a specific commit-ish (tag, SHA, branch)
    pub commit_ish: Option<String>,

    /// Override default branch name
    #[arg(long)]
    pub branch: Option<String>,

    /// Add specific repo(s) (alias or alias:branch)
    #[arg(long = "repo", value_name = "ALIAS[:BRANCH]")]
    pub repos: Vec<RepoSpec>,

    /// Add all repos from .meta.yaml
    #[arg(long, conflicts_with = "repos")]
    pub all: bool,

    /// Preview planned worktree operations without creating anything
    #[arg(long)]
    pub dry_run: bool,

    /// Alias for positional <COMMIT-ISH> (hidden, kept for backward compatibility)
    #[arg(long, value_name = "REF", hide = true, conflicts_with_all = ["commit_ish", "from_pr"])]
    pub from_ref: Option<String>,

    /// Start from a PR's head branch (owner/repo#N)
    #[arg(long, value_name = "OWNER/REPO#N", conflicts_with = "commit_ish")]
    pub from_pr: Option<String>,

    /// Mark for automatic cleanup
    #[arg(long)]
    pub ephemeral: bool,

    /// Time-to-live (30s, 5m, 1h, 2d, 1w)
    #[arg(long, value_name = "DURATION", value_parser = parse_duration_clap)]
    pub ttl: Option<u64>,

    /// Record custom metadata (key=value)
    #[arg(long = "meta", value_name = "KEY=VALUE")]
    pub custom_meta: Vec<String>,

    /// Fail if commit-ish doesn't exist in all repos (errors instead of warnings)
    ///
    /// When starting worktrees from a specific tag/SHA/branch,
    /// repos that don't have that ref are normally skipped with a warning.
    /// With --strict, missing refs cause the entire operation to fail instead.
    /// Useful in CI/automation where you want all-or-nothing behavior.
    #[arg(long)]
    pub strict: bool,

    /// Skip automatic dependency resolution
    ///
    /// By default, worktree create includes the root repo and resolves
    /// dependencies via provides/depends_on from .meta.yaml.
    /// Use --no-deps to include only the named repos (and, for a repo nested
    /// inside another project's path, that parent project).
    #[arg(long)]
    pub no_deps: bool,

    /// Resolve dependencies across the full nested meta tree
    ///
    /// By default, dependencies are resolved from the current .meta.yaml level.
    /// With --recursive, walks up to the root meta directory and resolves
    /// dependencies across all nested .meta.yaml workspaces. Repo aliases
    /// use full paths from root (e.g., open-source/gitkb/core).
    #[arg(long, short = 'r')]
    pub recursive: bool,
}

#[derive(Args)]
pub struct AddArgs {
    /// Worktree name
    pub name: String,

    /// Repo(s) to add (alias or alias:branch)
    #[arg(long = "repo", value_name = "ALIAS[:BRANCH]", required = true)]
    pub repos: Vec<RepoSpec>,
}

#[derive(Args)]
pub struct DestroyArgs {
    /// Worktree name
    pub name: String,

    /// Remove even with uncommitted changes
    #[arg(long)]
    pub force: bool,
}

#[derive(Args)]
pub struct ListArgs {
    /// Worktree name (defaults to the current set inside .worktrees/<name>)
    pub name: Option<String>,

    /// Include realized repositories nested beneath other worktree repositories
    #[arg(long, short = 'r')]
    pub recursive: bool,

    /// Return membership paths without observing branches or Git status
    #[arg(long)]
    pub metadata_only: bool,
}

#[derive(Args)]
pub struct StatusArgs {
    /// Include realized repositories nested beneath other worktree repositories
    #[arg(long, short = 'r')]
    pub recursive: bool,
    /// Worktree name
    pub name: String,
}

#[derive(Args)]
pub struct DiffArgs {
    /// Worktree name
    pub name: String,

    /// Base branch for comparison
    #[arg(long, default_value = "main")]
    pub base: String,

    /// Show diffstat summary only
    #[arg(long)]
    pub stat: bool,
}

#[derive(Args)]
pub struct ExecArgs {
    // NOTE: Positional arg order matters — name, commit_ish, command must stay in this order.
    /// Worktree name
    pub name: String,

    /// Start from a specific commit-ish (ephemeral only)
    pub commit_ish: Option<String>,

    /// Only run in these repos (comma-separated): each value is a repo's name,
    /// or its whole path in the set (. for the root), never part of one
    #[arg(long, value_delimiter = ',')]
    pub include: Vec<String>,

    /// Skip these repos (comma-separated), named like --include
    #[arg(long, value_delimiter = ',')]
    pub exclude: Vec<String>,

    /// Run commands in parallel
    #[arg(long)]
    pub parallel: bool,

    /// Atomic create+exec+remove (requires --all or --repo, and -- <cmd>)
    #[arg(long)]
    pub ephemeral: bool,

    // --- Ephemeral-only create flags (ignored when not --ephemeral) ---
    /// Add specific repo(s) for ephemeral worktree (alias or alias:branch)
    #[arg(long = "repo", value_name = "ALIAS[:BRANCH]")]
    pub repos: Vec<RepoSpec>,

    /// Add all repos for ephemeral worktree
    #[arg(long)]
    pub all: bool,

    /// Record custom metadata for ephemeral worktree (key=value)
    #[arg(long = "meta", value_name = "KEY=VALUE")]
    pub custom_meta: Vec<String>,

    /// Alias for positional <COMMIT-ISH> (hidden, kept for backward compatibility)
    #[arg(long, value_name = "REF", hide = true, conflicts_with_all = ["commit_ish", "from_pr"])]
    pub from_ref: Option<String>,

    /// Start from a PR's head branch (ephemeral only, owner/repo#N)
    #[arg(long, value_name = "OWNER/REPO#N", conflicts_with = "commit_ish")]
    pub from_pr: Option<String>,

    /// Override branch name for ephemeral worktree
    #[arg(long = "branch")]
    pub branch: Option<String>,

    /// Command and arguments to execute (after --)
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}

#[derive(Args)]
pub struct PruneArgs {
    /// Preview without removing
    #[arg(long)]
    pub dry_run: bool,
}

/// Parse a human-friendly duration string for clap value_parser.
fn parse_duration_clap(s: &str) -> std::result::Result<u64, String> {
    meta_git_lib::worktree::helpers::parse_duration(s).map_err(|e| e.to_string())
}
