//! CLI argument types for worktree subcommands.
//!
//! These are clap-derived types that belong in the CLI crate, not the library.

use crate::worktree::RepoSpec;
use clap::{Args, Subcommand};

/// Worktree subcommands parsed by clap.
#[derive(Subcommand)]
pub enum WorktreeCommands {
    /// Create a new worktree set
    #[command(after_help = "meta's options go before the command. \
        `meta -r git worktree create <name> --repo <path>` finds --repo across the meta repos \
        nested under the nearest .meta.yaml (aliases are full paths from there) and resolves \
        dependencies there; `meta -A git worktree create <name> --repo <path>` does so across \
        the whole workspace, from its root (as open-source/gitkb), wherever you are. \
        `meta --strict git worktree create <name> --every-repo` fails, and makes nothing, where \
        a repo would be skipped with a warning (no start commit-ish; with --every-repo, not \
        cloned); a failing post-create hook fails it too, after the set was made.")]
    Create(CreateArgs),
    /// Add a repo to an existing worktree set
    Add(AddArgs),
    /// Remove a worktree set
    ///
    /// Like `git worktree remove` in each repo: modified or untracked files
    /// refuse the remove unless --force; files git ignores (.env, target/)
    /// are deleted with it. A branch create or add made for the set is
    /// deleted only when every commit on it is on a remote, a tag or another
    /// branch, as `git branch -d` would; otherwise it is kept and named.
    /// Branches the set only checked out are left.
    Remove(RemoveArgs),
    /// List worktree sets (defaults to the current set when inside one)
    #[command(
        after_help = "`meta --json git worktree list` prints JSON (with --metadata-only, \
        the members without git). Repos nested in others are listed either way. \
        `meta --strict git worktree list` fails on a folder that can't be scanned, a checkout \
        whose git files are broken, or a repo git can't read, instead of skipping it or marking \
        it unknown."
    )]
    List(ListArgs),
    /// Show detailed status of a worktree set
    #[command(after_help = "Repos nested in others are listed either way. \
        `meta --strict git worktree status <name>` fails on a folder that can't be scanned, a \
        checkout whose git files are broken, or a repo git can't read, instead of skipping it or \
        marking it unknown.")]
    Status(StatusArgs),
    /// Show cross-repo diff vs base branch
    Diff(DiffArgs),
    /// Run a command across worktree repos
    #[command(after_help = "meta's options go before the command: \
        `meta --include api,web git worktree exec <name> -- make` chooses the set's repos (by \
        name, or path in the set, . for the root; --exclude and --tag too), and \
        `meta --parallel git worktree exec <name> -- make` runs them at once.")]
    Exec(ExecArgs),
    /// Remove expired/orphaned worktree sets of this workspace
    Prune,
}

#[derive(Args)]
pub struct CreateArgs {
    // NOTE: Positional arg order matters — name then commit_ish must stay in this order.
    /// Worktree name
    pub name: String,

    /// Start new branches from this commit-ish (tag, SHA, branch); without
    /// it, each from its remote's default branch (origin/HEAD)
    pub commit_ish: Option<String>,

    /// Override default branch name
    #[arg(long)]
    pub branch: Option<String>,

    /// Add specific repo(s) (alias or alias:branch)
    #[arg(long = "repo", value_name = "ALIAS[:BRANCH]")]
    pub repos: Vec<RepoSpec>,

    /// Add every repo meta reaches: those of the nearest .meta.yaml, with
    /// meta -r the nested meta repos' too, with meta -A the whole workspace's
    /// (one that isn't cloned is skipped with a warning; an error with meta
    /// --strict)
    #[arg(long, conflicts_with = "repos")]
    pub every_repo: bool,

    /// Start from a PR's head branch (owner/repo#N)
    #[arg(long, value_name = "OWNER/REPO#N", conflicts_with = "commit_ish")]
    pub from_pr: Option<String>,

    /// Let the next `prune` remove it
    #[arg(long)]
    pub ephemeral: bool,

    /// Time-to-live (30s, 5m, 1h, 2d, 1w)
    #[arg(long, value_name = "DURATION", value_parser = parse_duration_clap)]
    pub ttl: Option<u64>,

    /// Record custom metadata (key=value)
    #[arg(long = "meta", value_name = "KEY=VALUE")]
    pub custom_meta: Vec<String>,

    /// Skip automatic dependency resolution
    ///
    /// By default, worktree create includes the root repo and resolves
    /// dependencies via provides/depends_on from .meta.yaml.
    /// Use --no-deps to include only the named repos (and, for a repo nested
    /// inside another project's path, that parent project), and to leave
    /// their depends_on unchecked (with --every-repo too).
    #[arg(long)]
    pub no_deps: bool,
}

#[derive(Args)]
pub struct AddArgs {
    /// Worktree name
    pub name: String,

    /// Start a new branch from this commit-ish (tag, SHA, branch); without
    /// it, from the remote's default branch (origin/HEAD)
    pub commit_ish: Option<String>,

    /// Repo(s) to add (alias or alias:branch)
    #[arg(long = "repo", value_name = "ALIAS[:BRANCH]", required = true)]
    pub repos: Vec<RepoSpec>,
}

#[derive(Args)]
pub struct RemoveArgs {
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

    /// meta's --strict: fail on what can't be scanned or read
    #[arg(skip)]
    pub strict: bool,

    /// Return membership paths without observing branches or Git status
    #[arg(long)]
    pub metadata_only: bool,
}

#[derive(Args)]
pub struct StatusArgs {
    /// meta's --strict: fail on what can't be scanned or read
    #[arg(skip)]
    pub strict: bool,
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
    /// Worktree name
    pub name: String,

    /// Command and arguments to execute (after --)
    #[arg(last = true, required = true)]
    pub command: Vec<String>,
}

/// Parse a human-friendly duration string for clap value_parser.
fn parse_duration_clap(s: &str) -> std::result::Result<u64, String> {
    meta_plugin::duration::parse_duration(s).map_err(|e| e.to_string())
}
