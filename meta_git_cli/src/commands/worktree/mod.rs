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
use colored::*;
use meta_core::display::{escape, escape_lines, escape_path};
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
            options.json_output || parsed.json,
            options.strict,
            cwd,
        ) {
            Ok(()) => CommandResult::Message(String::new()),
            Err(e) => CommandResult::Error(format!("{e:#}")),
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
            create::handle_create(args, mode, verbose, json, global_strict)
        }
        WorktreeCommands::Add(args) => add::handle_add(args, mode, verbose, json),
        WorktreeCommands::Remove(args) => remove::handle_remove(args, mode, verbose, json),
        WorktreeCommands::List(args) => list::handle_list(args, json, cwd),
        WorktreeCommands::Status(args) => status::handle_status(args, json),
        WorktreeCommands::Diff(args) => diff::handle_diff(args, json),
        WorktreeCommands::Exec(args) => exec::handle_exec(args, mode, verbose, json),
        WorktreeCommands::Prune => prune::handle_prune(mode, json, global_strict),
    }
}

/// Emit a warning message, or bail with an error in strict mode.
///
/// In strict mode, returns Err to fail the command.
/// In normal mode, prints a warning and returns Ok(()).
///
/// Use this for situations where an operation can continue despite a problem,
/// but strict mode should treat it as a failure.
pub(crate) fn warn_or_bail(strict: bool, message: impl std::fmt::Display) -> anyhow::Result<()> {
    if strict {
        anyhow::bail!("{message} (strict mode)")
    }
    warn(message);
    Ok(())
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

/// Print a warning; the message can carry paths and names from git and
/// the filesystem.
pub(crate) fn warn(message: impl std::fmt::Display) {
    eprintln!(
        "{} {}",
        "warning:".yellow().bold(),
        escape_lines(&message.to_string())
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── warn_or_bail tests ─────────────────────────────────

    #[test]
    fn warn_or_bail_returns_ok_in_normal_mode() {
        let result = warn_or_bail(false, "test message");
        assert!(result.is_ok());
    }

    #[test]
    fn warn_or_bail_returns_err_in_strict_mode() {
        let result = warn_or_bail(true, "test message");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("test message"));
        assert!(err.to_string().contains("strict mode"));
    }

    #[test]
    fn warn_or_bail_includes_message_in_error() {
        let result = warn_or_bail(true, "specific error details");
        let err = result.unwrap_err();
        assert!(err.to_string().contains("specific error details"));
    }
}
