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
use meta_plugin::{Ctx, ErrorCode, ErrorInfo, Request, Response, Scope};
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
pub(crate) struct WorktreeParser {
    #[command(subcommand)]
    pub(crate) command: WorktreeCommands,
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
pub(crate) fn handle_worktree_command(
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
                Ok(create::Made::Created(lease)) => {
                    match ephemeral {
                        true => lease.leave(),
                        false => drop(lease),
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
        WorktreeCommands::List(_) | WorktreeCommands::Status(_) if request.recursive => {
            let list = match command {
                WorktreeCommands::List(_) => "list",
                _ => "status",
            };
            Err(ErrorInfo::new(
                ErrorCode::Usage,
                format!(
                    "worktree {list} always shows nested repos; -r and -A change nothing here. \
                     For the scan that fails on folders it can't read: meta --strict git \
                     worktree {list}"
                ),
            )
            .into())
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
        WorktreeCommands::Exec(args) => exec::handle_exec(args, request),
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
