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
use meta_plugin::{Answer, Ctx, ErrorCode, ErrorInfo, Request, Response, Scope};
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

/// Refuse meta's selection for `command` on the whole set `name`: a
/// selection that leaves out a checkout of it would read as "only these".
/// The host applies `--include`/`--exclude`/`--tag` to the set it names;
/// only `exec` runs in the checkouts they select.
fn whole_set(request: &Request, command: &str, name: &str) -> Result<()> {
    let Some(set) = request.set.as_ref().filter(|set| set.name == name) else {
        return Ok(());
    };
    let narrowed = set
        .members
        .iter()
        .any(|member| !request.projects.iter().any(|p| p.abs == member.abs));
    if narrowed {
        return Err(ErrorInfo::new(
            ErrorCode::Usage,
            format!(
                "meta git worktree {command} does not take --include/--exclude/--tag; use `meta \
                 --include <repo> git worktree exec <name> -- <command>`"
            ),
        )
        .into());
    }
    Ok(())
}

/// Execute a worktree command dispatched from the plugin.
///
/// `request.command` is the matched command (e.g. "git worktree create", or
/// "git worktree" with the subcommand in `request.args`).
pub fn execute_worktree_command(ctx: &Ctx, request: &Request) -> Result<Response> {
    let command = request.command.as_str();
    let args = &request.args;
    // Extract the subcommand name from the matched command prefix (if present).
    // When the plugin registers bare "git worktree", the subcommand is in args[0] instead.
    let subcommand = command.strip_prefix("git worktree ").unwrap_or("");

    // Build args for clap
    let clap_args: Vec<String> = if subcommand.is_empty() {
        // Bare "git worktree" — args already contains the subcommand (if any)
        args.to_vec()
    } else {
        // Subcommand was part of the command prefix — prepend it
        std::iter::once(subcommand.to_string())
            .chain(args.iter().cloned())
            .collect()
    };
    let usage = |message: String| Ok(Response::error(ErrorInfo::new(ErrorCode::Usage, message)));

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
        if request.recursive || clap_args.iter().any(typed) {
            return usage(format!(
                "worktree {list} always shows nested repos; -r and -A change nothing here. For \
                 the scan that fails on folders it can't read: meta --strict git worktree {list}"
            ));
        }
    }
    let meta = &request.meta_options;
    let name = "git worktree";
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
        return match meta_plugin::parse_args::<WorktreeParser>(name, &every, meta) {
            Err(response) => match response.answer {
                Answer::Error(why) if why.code == ErrorCode::Usage => {
                    usage(format!("{said}.\n\n{}", why.message))
                }
                _ => Ok(*response),
            },
            Ok(_) => {
                let line: Vec<String> = std::iter::once("meta".to_string())
                    .chain(meta.typed.iter().cloned())
                    .chain(name.split(' ').map(str::to_string))
                    .chain(every)
                    .collect();
                usage(format!(
                    "{said}, as in {}",
                    escape(&meta_plugin::display::display_command(&line))
                ))
            }
        };
    }
    match meta_plugin::parse_args::<WorktreeParser>(name, &clap_args, meta) {
        Ok(parsed) => handle_worktree_command(parsed.command, ctx, request),
        Err(response) => Ok(*response),
    }
}

/// Refuse to make or act on sets from inside a set whose checkouts hold a
/// record of meta-git's that can't be read: which workspace the set stands
/// for, and so where its sets are, is then unknown. Naming the record.
fn readable_records(request: &Request) -> Result<()> {
    let Some(set) = request
        .set
        .as_ref()
        .filter(|_| request.scope != Scope::NamedSet)
    else {
        return Ok(());
    };
    let checkouts =
        std::iter::once(set.dir.as_path()).chain(set.members.iter().map(|m| m.abs.as_path()));
    for checkout in checkouts {
        meta_core::set_record::member(checkout)?;
    }
    Ok(())
}

/// Dispatch parsed worktree commands while retaining command-specific policies.
fn handle_worktree_command(
    command: WorktreeCommands,
    ctx: &Ctx,
    request: &Request,
) -> Result<Response> {
    if !matches!(
        command,
        WorktreeCommands::List(_) | WorktreeCommands::Status(_) | WorktreeCommands::Diff(_)
    ) {
        readable_records(request)?;
    }
    match command {
        WorktreeCommands::Create(args) => {
            // An ephemeral set is the next prune's: its lease stays beside it
            let ephemeral = args.ephemeral;
            let mut records = Vec::new();
            match create::handle_create(args, ctx, request, &mut records) {
                Ok(create::Made::Again) => crate::again(ctx, request),
                Ok(create::Made::Preview(must_clone)) => {
                    Ok(Response::printed(records).skipping(must_clone))
                }
                Ok(create::Made::Created(created)) => {
                    match ephemeral {
                        true => created.lease.leave(),
                        false => drop(created.lease),
                    }
                    Ok(Response::printed(records))
                }
                Err(e) => crate::printed_with_git(ctx, request, records, Some(e)),
            }
        }
        WorktreeCommands::Add(args) => {
            whole_set(request, "add", &args.name)?;
            add::handle_add(args, ctx, request)
        }
        WorktreeCommands::Remove(args) => {
            whole_set(request, "remove", &args.name)?;
            remove::handle_remove(args, ctx, request)
        }
        WorktreeCommands::List(mut args) => {
            args.strict = ctx.meta().strict;
            list::handle_list(args, ctx, request)
        }
        WorktreeCommands::Status(mut args) => {
            whole_set(request, "status", &args.name)?;
            args.strict = ctx.meta().strict;
            status::handle_status(args, ctx, request)
        }
        WorktreeCommands::Diff(args) => {
            whole_set(request, "diff", &args.name)?;
            diff::handle_diff(args, ctx, request)
        }
        WorktreeCommands::Exec(args) => exec::handle_exec(args, ctx, request),
        WorktreeCommands::Prune => prune::handle_prune(ctx, request),
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
