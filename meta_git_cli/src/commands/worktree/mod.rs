//! Worktree command dispatch and help.

mod add;
pub(crate) mod cli_types;
mod create;
mod diff;
mod exec;
mod list;
mod prune;
mod remove;
mod status;

use anyhow::Result;
use clap::Parser;
use meta_core::display::{escape, escape_path};
use meta_core::ExecutionMode;
use meta_plugin_protocol::{CommandResult, PluginRequestOptions};
use std::path::Path;

use cli_types::WorktreeCommands;

/// Manage git worktrees across repos
#[derive(Parser)]
#[command(arg_required_else_help = true)]
struct WorktreeParser {
    /// Show what would be created, run or removed, and change nothing
    #[arg(long, global = true)]
    dry_run: bool,
    /// Output results in JSON format
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: WorktreeCommands,
}

/// Execute a worktree command dispatched from the plugin.
///
/// `command` is the matched command string (e.g., "worktree create", "worktree", "git worktree").
/// `args` contains the remaining arguments after the command prefix.
/// `cwd` is the protocol request directory, independent of the plugin process.
pub fn execute_worktree_command(
    command: &str,
    args: &[String],
    options: &PluginRequestOptions,
    cwd: &Path,
) -> CommandResult {
    // Extract the subcommand name from the matched command prefix (if present).
    // When the plugin registers bare "worktree", the subcommand is in args[0] instead.
    let subcommand = command
        .strip_prefix("worktree ")
        .or_else(|| command.strip_prefix("git worktree "))
        .unwrap_or("");

    // Build args for clap
    let mut clap_args: Vec<String> = if subcommand.is_empty() {
        // Bare "worktree" or "git worktree" — args already contains the subcommand (if any)
        args.to_vec()
    } else {
        // Subcommand was part of the command prefix — prepend it
        std::iter::once(subcommand.to_string())
            .chain(args.iter().cloned())
            .collect()
    };

    // Inject --recursive if the protocol option was set but the flag wasn't in the
    // command args (the host strips global flags like --recursive before forwarding).
    // Inject only for subcommands that explicitly support recursive behavior.
    // The subcommand: the first word that isn't a flag (`--dry-run create`)
    let supports_recursive = matches!(
        clap_args
            .iter()
            .find(|a| !a.starts_with('-'))
            .map(String::as_str),
        Some("create" | "list" | "status")
    );
    if options.recursive
        && supports_recursive
        && !clap_args.iter().any(|a| a == "--recursive" || a == "-r")
    {
        clap_args.push("--recursive".to_string());
    }

    let name = if command.starts_with("git ") {
        "git worktree"
    } else {
        "worktree"
    };
    match meta_plugin_protocol::parse_args::<WorktreeParser>(name, &clap_args) {
        Ok(parsed) => match handle_worktree_command(
            parsed.command,
            // meta's --dry-run, or the worktree command's own
            ExecutionMode::from_dry_run(options.dry_run || parsed.dry_run),
            options.verbose,
            // meta's --json, or the command's own: its errors are JSON too
            meta_core::out::set_json(options.json_output || parsed.json),
            options.strict,
            cwd,
        ) {
            Ok(()) => CommandResult::Message(String::new()),
            Err(e) => e.into(),
        },
        Err(result) => result,
    }
}

/// Dispatch parsed worktree commands while retaining command-specific policies.
fn handle_worktree_command(
    command: WorktreeCommands,
    mode: ExecutionMode,
    verbose: bool,
    json: bool,
    global_strict: bool,
    cwd: &Path,
) -> Result<()> {
    match command {
        WorktreeCommands::Create(args) => {
            // An ephemeral set is the next prune's: its lease stays beside it
            let ephemeral = args.ephemeral;
            let created = create::handle_create(args, mode, verbose, json, global_strict)?;
            if let Some(created) = created.filter(|_| ephemeral) {
                created.lease.leave();
            }
            Ok(())
        }
        WorktreeCommands::Add(args) => add::handle_add(args, mode, verbose, json),
        WorktreeCommands::Remove(args) => {
            remove::handle_remove(args, mode, verbose, json, global_strict)
        }
        WorktreeCommands::List(args) => list::handle_list(args, json, cwd),
        WorktreeCommands::Status(args) => status::handle_status(args, json),
        WorktreeCommands::Diff(args) => diff::handle_diff(args, json),
        WorktreeCommands::Exec(args) => exec::handle_exec(args, mode, verbose, json),
        WorktreeCommands::Prune => prune::handle_prune(mode, json, global_strict),
    }
}

/// Why the project `alias` at `source` has no repository to make a worktree
/// of, if it has none (see [`meta_core::gitdir::Checkout`]).
pub(crate) fn not_a_repository(alias: &str, source: &Path) -> Option<String> {
    meta_core::gitdir::Checkout::of(source)
        .problem()
        .map(|problem| {
            format!(
                "'{}' at {}: {}",
                escape(alias),
                escape_path(source),
                escape(&problem)
            )
        })
}
