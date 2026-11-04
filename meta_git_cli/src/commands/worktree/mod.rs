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
use meta_plugin::display::{escape, escape_path};
use meta_plugin::{CommandResult, PluginRequestOptions};
use std::path::Path;

use cli_types::WorktreeCommands;

/// Manage git worktrees across repos
#[derive(Parser)]
#[command(
    arg_required_else_help = true,
    after_help = "Git options: none of git worktree's; these commands are meta's own. \
                  meta's options go before them: meta --dry-run git worktree remove NAME, \
                  meta --json git worktree list."
)]
struct WorktreeParser {
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
    let clap_args: Vec<String> = if subcommand.is_empty() {
        // Bare "worktree" or "git worktree" — args already contains the subcommand (if any)
        args.to_vec()
    } else {
        // Subcommand was part of the command prefix — prepend it
        std::iter::once(subcommand.to_string())
            .chain(args.iter().cloned())
            .collect()
    };

    // The subcommand: the first word that isn't a flag
    let subcommand = clap_args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str);
    // list and status always show nested repos: -r would only have been the
    // strict scan, which is --strict's
    if let Some(list @ ("list" | "status")) = subcommand {
        // in a cluster too (`-rv`): list and status take no short options
        let typed = |a: &String| {
            ["--recursive", "--all"].contains(&a.as_str())
                || a.strip_prefix('-')
                    .is_some_and(|s| !s.starts_with('-') && (s.contains('r') || s.contains('A')))
        };
        if options.recursive || options.all || clap_args.iter().any(typed) {
            return CommandResult::Usage(format!(
                "worktree {list} always shows nested repos; -r and -A change nothing here. For \
                 the scan that fails on folders it can't read: meta --strict git worktree {list}"
            ));
        }
    }
    // They find their repos in the set; exec applies meta's selection to
    // the set's members
    let exec = subcommand == Some("exec");
    if options.filtered() && !exec {
        return CommandResult::Usage(format!(
            "meta {command} does not take --include/--exclude/--tag; use `meta --include <repo> \
             git worktree exec <name> -- <command>`"
        ));
    }
    let meta = match exec {
        true => options.meta_options.clone(),
        false => options.meta_options.refusing(&crate::FILTERS),
    };

    let name = if command.starts_with("git ") {
        "git worktree"
    } else {
        "worktree"
    };
    // create's and exec's own --all became --every-repo: meta's --all
    // (-A) before the command is the whole workspace, not every repo here;
    // with no --repo, it is the old --all that was meant. The line with
    // --every-repo is checked as any other (another option of meta's after
    // the command moves too).
    let words = || clap_args.iter().take_while(|a| *a != "--");
    let all = |a: &String| a == "--all" || a == "-A";
    if matches!(subcommand, Some("create" | "exec"))
        && words().any(all)
        && !words().any(|a| a == "--repo" || a.starts_with("--repo="))
    {
        let every: Vec<String> = clap_args
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let before = clap_args.iter().take(i).all(|w| w != "--");
                match before && all(a) {
                    true => "--every-repo".to_string(),
                    false => a.clone(),
                }
            })
            .collect();
        let said = "-A (--all) is meta's own option, the whole workspace, and goes before the \
                    command; every repo meta reaches is --every-repo";
        return match meta_plugin::parse_args::<WorktreeParser>(name, &every, &meta) {
            Err(CommandResult::Usage(why)) => CommandResult::Usage(format!("{said}.\n\n{why}")),
            Err(other) => other,
            Ok(_) => {
                let line: Vec<String> = std::iter::once("meta".to_string())
                    .chain(options.meta_options.typed.iter().cloned())
                    .chain(name.split(' ').map(str::to_string))
                    .chain(every)
                    .collect();
                CommandResult::Usage(format!(
                    "{said}, as in {}",
                    escape(&meta_plugin::display::display_command(&line))
                ))
            }
        };
    }
    match meta_plugin::parse_args::<WorktreeParser>(name, &clap_args, &meta) {
        Ok(parsed) => match handle_worktree_command(parsed.command, options, cwd) {
            Ok(()) => CommandResult::Message(String::new()),
            Err(e) => e.into(),
        },
        Err(result) => result,
    }
}

/// Dispatch parsed worktree commands while retaining command-specific policies.
fn handle_worktree_command(
    command: WorktreeCommands,
    options: &PluginRequestOptions,
    cwd: &Path,
) -> Result<()> {
    let (mode, verbose, json) = (options.mode(), options.verbose, options.json_output);
    let global_strict = options.strict;
    match command {
        WorktreeCommands::Create(mut args) => {
            args.recursive = options.recursive;
            args.from_root = options.all;
            args.depth = options.depth;
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
        WorktreeCommands::List(mut args) => {
            args.strict = global_strict;
            list::handle_list(args, json, cwd)
        }
        WorktreeCommands::Status(mut args) => {
            args.strict = global_strict;
            status::handle_status(args, json)
        }
        WorktreeCommands::Diff(args) => diff::handle_diff(args, json),
        WorktreeCommands::Exec(args) => exec::handle_exec(args, options),
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
