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

/// Clap parser wrapper for worktree subcommands.
#[derive(Parser)]
struct WorktreeParser {
    /// Show what would be created, run or removed, and change nothing
    #[arg(long, global = true)]
    dry_run: bool,
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
    let supports_recursive = matches!(
        clap_args.first().map(String::as_str),
        Some("create" | "list" | "status")
    );
    if options.recursive
        && supports_recursive
        && !clap_args.iter().any(|a| a == "--recursive" || a == "-r")
    {
        clap_args.push("--recursive".to_string());
    }

    // No subcommand at all — show help
    if clap_args.is_empty() {
        print_worktree_help();
        return CommandResult::Message(String::new());
    }
    if clap_args.len() == 1 && matches!(clap_args[0].as_str(), "--help" | "-h") {
        print_worktree_help();
        return CommandResult::Message(String::new());
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
            options.json_output,
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
            create::handle_create(args, mode, verbose, json, global_strict).map(|_| ())
        }
        WorktreeCommands::Add(args) => add::handle_add(args, mode, verbose, json),
        WorktreeCommands::Remove(args) => remove::handle_remove(args, mode, verbose, json),
        WorktreeCommands::List(args) => list::handle_list(args, json, cwd),
        WorktreeCommands::Status(args) => status::handle_status(args, json),
        WorktreeCommands::Diff(args) => diff::handle_diff(args, json),
        WorktreeCommands::Exec(args) => exec::handle_exec(args, mode, verbose, json),
        WorktreeCommands::Prune => prune::handle_prune(mode, json, global_strict),
        WorktreeCommands::Unknown(args) => {
            let cmd = escape(args.first().map(|s| s.as_str()).unwrap_or(""));
            // the help, then the error, which meta prints once
            eprint_worktree_help();
            anyhow::bail!("unrecognized worktree subcommand '{cmd}'");
        }
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
/// of, if it has none: not cloned yet, or a directory git doesn't take for
/// a repository (it would act on the one around it).
pub(crate) fn not_a_repository(alias: &str, source: &Path) -> Result<Option<String>> {
    if meta_core::gitdir::common_dir(source)?.is_some() {
        return Ok(None);
    }
    Ok(Some(match source.exists() {
        false => format!(
            "'{}' is not cloned at {} (run 'meta git update' to clone it)",
            escape(alias),
            escape_path(source)
        ),
        true => format!(
            "'{}' at {} is not a git repository",
            escape(alias),
            escape_path(source)
        ),
    }))
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

/// Print worktree help text to stdout.
pub fn print_worktree_help() {
    write_worktree_help(&mut std::io::stdout());
}

/// Print worktree help text to stderr (for error cases in plugin context).
pub fn eprint_worktree_help() {
    write_worktree_help(&mut std::io::stderr());
}

/// Write top-level help including recursive inventory usage.
fn write_worktree_help(w: &mut dyn std::io::Write) {
    let _ = writeln!(w, "Manage git worktrees across repos");
    let _ = writeln!(w);
    let _ = writeln!(w, "USAGE: meta git worktree <COMMAND> [OPTIONS]");
    let _ = writeln!(w);
    let _ = writeln!(w, "COMMANDS:");
    let _ = writeln!(w, "  create   Create a new worktree set");
    let _ = writeln!(w, "  add      Add a repo to an existing worktree set");
    let _ = writeln!(w, "  remove   Remove a worktree set");
    let _ = writeln!(
        w,
        "  list     List worktree sets (current set when inside one)"
    );
    let _ = writeln!(w, "  status   Show detailed status of a worktree set");
    let _ = writeln!(w, "  diff     Show cross-repo diff vs base branch");
    let _ = writeln!(w, "  exec     Run a command across worktree repos");
    let _ = writeln!(
        w,
        "  prune    Remove expired/orphaned worktree sets of this workspace"
    );
    let _ = writeln!(w);
    let _ = writeln!(w, "OPTIONS (every command):");
    let _ = writeln!(
        w,
        "  --dry-run                Show what would be created, run or removed; change nothing"
    );
    let _ = writeln!(w);
    let _ = writeln!(w, "CREATE OPTIONS:");
    let _ = writeln!(w, "  --repo <ALIAS[:BRANCH]>  Add specific repo(s)");
    let _ = writeln!(
        w,
        "  --all                    Add all repos from .meta.yaml (uncloned ones skipped)"
    );
    let _ = writeln!(w, "  --branch <NAME>          Override default branch name");
    let _ = writeln!(
        w,
        "  <COMMIT-ISH>             Start from a tag, SHA or branch (after the name)"
    );
    let _ = writeln!(
        w,
        "  --from-pr <OWNER/REPO#N> Start from a PR's head branch"
    );
    let _ = writeln!(w, "  --ephemeral              Mark for automatic cleanup");
    let _ = writeln!(
        w,
        "  --ttl <DURATION>         Time-to-live (30s, 5m, 1h, 2d, 1w)"
    );
    let _ = writeln!(w, "  --meta <KEY=VALUE>       Record custom metadata");
    let _ = writeln!(
        w,
        "  --no-deps                Only the named repos (and the repos they are nested in)"
    );
    let _ = writeln!(
        w,
        "  --strict                 Fail where a repo would be skipped"
    );
    let _ = writeln!(
        w,
        "  -r, --recursive          Resolve dependencies across nested meta repos"
    );
    let _ = writeln!(w);
    let _ = writeln!(w, "ADD OPTIONS:");
    let _ = writeln!(w, "  --repo <ALIAS[:BRANCH]>  Repo(s) to add");
    let _ = writeln!(w);
    let _ = writeln!(w, "LIST / STATUS OPTIONS:");
    let _ = writeln!(
        w,
        "  -r, --recursive          Include repositories nested beneath worktree repos"
    );
    let _ = writeln!(w);
    let _ = writeln!(w, "LIST OPTIONS:");
    let _ = writeln!(
        w,
        "LIST [NAME] defaults to the enclosing set, or all sets outside a worktree."
    );
    let _ = writeln!(
        w,
        "  --metadata-only          List membership paths without Git-state probes"
    );
    let _ = writeln!(w);
    let _ = writeln!(w, "REMOVE OPTIONS:");
    let _ = writeln!(
        w,
        "  --force                  Remove even with uncommitted changes"
    );
    let _ = writeln!(w);
    let _ = writeln!(w, "EXEC OPTIONS:");
    let _ = writeln!(w, "  --include <REPOS>        Only run in specified repos");
    let _ = writeln!(w, "  --exclude <REPOS>        Skip specified repos");
    let _ = writeln!(w, "  --parallel               Run commands concurrently");
    let _ = writeln!(w, "  --ephemeral              Atomic create+exec+remove");
    let _ = writeln!(
        w,
        "  --repo, --all, --branch, <COMMIT-ISH>, --from-pr, --meta"
    );
    let _ = writeln!(
        w,
        "                           As for create, with --ephemeral"
    );
    let _ = writeln!(w);
    let _ = writeln!(w, "DIFF OPTIONS:");
    let _ = writeln!(
        w,
        "  --base <BRANCH>          Base branch for comparison (default: main)"
    );
    let _ = writeln!(w, "  --stat                   Show diffstat summary only");
    let _ = writeln!(w);
    let _ = writeln!(
        w,
        "Use 'meta git worktree <command> --help' for more details."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The overview names every option of each command in that command's
    /// own section, so it can't fall behind what clap accepts.
    #[test]
    fn help_lists_every_option_under_its_command() {
        let mut help = Vec::new();
        write_worktree_help(&mut help);
        let help = String::from_utf8(help).unwrap();
        let parser = <WorktreeParser as clap::CommandFactory>::command();
        for command in parser.get_subcommands() {
            let name = command.get_name().to_uppercase();
            // the sections whose heading names this command
            let section: String = help
                .split("\n\n")
                .filter(|block| {
                    block.lines().next().is_some_and(|heading| {
                        heading.ends_with("OPTIONS:")
                            && heading.split([' ', '/']).any(|word| word == name)
                    })
                })
                .collect();
            for arg in command.get_arguments() {
                let id = arg.get_id().as_str();
                if matches!(id, "name" | "command" | "help") {
                    continue;
                }
                let shown = match arg.get_long() {
                    Some(long) => format!("--{long}"),
                    None => format!("<{}>", id.to_uppercase().replace('_', "-")),
                };
                assert!(section.contains(&shown), "{name} OPTIONS lacks {shown}");
            }
        }
    }

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
