use anyhow::Result;
use meta_plugin::client::Meta;
use meta_plugin::display::escape;
use meta_plugin::{errln, outln};
use meta_plugin::{Ctx, ErrorCode, ErrorInfo, Plan, ProjectResult, Request, Response, Scope, Step};
use std::path::Path;

use crate::worktree::helpers::*;

use super::cli_types::{CreateArgs, ExecArgs};
use crate::git_env;

/// `exec NAME -- CMD`: a plan of `CMD` in each checkout of the set meta's
/// `--include`/`--exclude`/`--tag` select (the host resolved them), named by
/// its path in the set; `--ephemeral`: make the set, run it there, remove
/// the set.
pub(crate) fn handle_exec(args: ExecArgs, ctx: &Ctx, request: &Request) -> Result<Response> {
    if args.ephemeral {
        return handle_ephemeral_exec(args, ctx, request);
    }

    let (_, repos) = discover_and_validate_worktree(&args.name, request)?;
    let git_env: std::collections::BTreeMap<String, String> =
        git_env::git_env().into_iter().collect();
    let step = |project: &str, dir: &Path| {
        let mut step = Step::new(Some(project), dir, args.command.clone());
        step.env = git_env.clone();
        step
    };
    // The checkouts the host selected in the set it named; a command line
    // it couldn't name the set from (`exec --repo x NAME`) takes no
    // selection, so every checkout
    let named = request.scope == Scope::NamedSet
        && request
            .set
            .as_ref()
            .is_some_and(|set| set.name == args.name);
    let steps = match named {
        true => request
            .projects
            .iter()
            .map(|p| step(&p.path, &p.abs))
            .collect(),
        false => repos.iter().map(|r| step(&r.alias, &r.path)).collect(),
    };
    // one checkout after another, at once only with meta's --parallel
    // typed (a config's default is for `meta exec`)
    Ok(Response::plan(Plan {
        steps,
        parallel: Some(request.meta_options.typed_has("--parallel")),
        ..Plan::default()
    }))
}

/// meta's options for a call back from this one: `--config` with the
/// workspace's config (the same workspace, wherever it is called from), and
/// `--verbose` when given.
fn meta_options(request: &Request) -> Vec<std::ffi::OsString> {
    let mut options = Vec::new();
    if let Some(workspace) = &request.workspace {
        options.extend(["--config".into(), workspace.config.clone().into_os_string()]);
    }
    if request.meta.verbose {
        options.push("--verbose".into());
    }
    options
}

/// `exec --ephemeral NAME -- CMD`: make the set (holding its lease), run
/// `meta exec -- CMD` in it, then remove it. The removal is started before
/// the run, waiting: it runs once the run is over, and also when this
/// program is stopped (Ctrl-C, SIGTERM) or killed meanwhile.
fn handle_ephemeral_exec(args: ExecArgs, ctx: &Ctx, request: &Request) -> Result<Response> {
    let verbose = request.meta.verbose;
    let name = args.name.clone();
    meta_plugin::sets::validate_name(&name)?;

    let cmd_parts = &args.command;
    if cmd_parts.is_empty() {
        anyhow::bail!("No command specified after --");
    }
    // Build CreateArgs from the exec args
    let create_args = CreateArgs {
        name: name.clone(),
        commit_ish: args.commit_ish.clone(),
        branch: args.branch.clone(),
        repos: args.repos.clone(),
        every_repo: args.every_repo,
        from_pr: args.from_pr.clone(),
        ephemeral: true,
        ttl: None,
        custom_meta: args.custom_meta.clone(),
        no_deps: false, // Include dependencies by default
    };

    if verbose {
        errln!("Creating ephemeral worktree '{name}'...");
    }
    // Ephemeral worktrees are created without --strict, so creation succeeds;
    // what it warns about is this run's
    let lenient = Ctx::new(meta_plugin::Meta {
        strict: false,
        ..ctx.meta().clone()
    });
    // The lease, held for the run: the set lives as long as it does, and once
    // this process is gone, however it ends, the removal below (or prune)
    // removes the set
    let made = super::create::handle_create(create_args, &lenient, request, &mut Vec::new());
    for warning in lenient.finish(Response::done()).warnings {
        ctx.warn(warning);
    }
    let created = match made? {
        super::create::Made::Created(created) => created,
        // the clones it made change the plan: from the top again
        super::create::Made::Again => return crate::again(ctx, request),
        // A preview makes no set
        super::create::Made::Preview(_) => {
            outln!(
                "Then would run `{}` in each repo planned above, and remove the set.",
                escape(&meta_plugin::display::display_command(cmd_parts))
            )?;
            return Ok(Response::done());
        }
    };
    let token = created.lease.token().to_string();
    let worktrees = created.dir.parent().unwrap_or(&created.dir).to_path_buf();

    // The removal, armed first: as `meta git worktree remove NAME --force`
    // with this run's lease, in the folder this set was made in (whatever
    // the config says by then)
    let mut remove = meta_options(request);
    if request.meta.json {
        remove.push("--silent".into());
    }
    remove.extend(["git", "worktree", "remove", &name, "--force"].map(Into::into));
    let removal = Meta::from_env()
        .and_then(|meta| {
            meta.cwd(&request.cwd)
                .env(meta_plugin::sets::WORKTREES_VAR, &worktrees)
                .args(remove)
                .lease(token.clone())
                .finishing()
        })
        .map_err(|e| {
            // no removal waits: the set is the next prune's
            anyhow::anyhow!(
                "Cannot arrange the removal of ephemeral worktree '{}': {e:#}; nothing was run. \
                 Run 'meta git worktree remove {} --force' to remove it.",
                escape(&name),
                escape(&name)
            )
        });
    let mut removal = match removal {
        Ok(removal) => removal,
        Err(e) => {
            created.lease.leave();
            return Err(e);
        }
    };

    // Run the command in the set just made, as `meta exec` runs one
    let run = run_in(&created.dir, cmd_parts, &token, request, &mut removal);

    if verbose {
        errln!("Removing ephemeral worktree '{name}'...");
    }
    // Clean up before propagating the result; a failed cleanup (such as a
    // checkout git cannot delete) fails the command too
    let cleanup = match removal.run() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(anyhow::anyhow!(
            "Failed to remove ephemeral worktree '{}' ({status}).\n  Run 'meta git worktree remove \
             {} --force' or 'meta git worktree prune' to clean up.",
            escape(&name),
            escape(&name)
        )),
        Err(e) => Err(e),
    };
    // a set this run couldn't remove is the next prune's: its lease stays
    match &cleanup {
        Err(_) => created.lease.leave(),
        Ok(()) => drop(created.lease),
    }
    match (run, cleanup) {
        (Err(e), Err(cleanup_error)) => {
            ctx.warn(format!("{cleanup_error:#}"));
            Err(e)
        }
        (Err(e), Ok(())) => Err(e),
        // a command that failed stays the failure; the cleanup only warns
        (Ok(rows), Err(cleanup_error)) if rows.iter().any(ProjectResult::is_failure) => {
            ctx.warn(format!("{cleanup_error:#}"));
            Ok(Response::printed(rows))
        }
        (Ok(_), Err(cleanup_error)) => Err(cleanup_error),
        // a command that failed fails the run, by its row
        (Ok(rows), Ok(())) => Ok(Response::printed(rows)),
    }
}

/// `meta exec -- command` in the set at `dir`, holding the set's lease
/// (`token`): in text, as the user watches it; under `--json`, its rows.
/// A run that fails is a failed row (or, in text, an error, as meta showed
/// the run).
fn run_in(
    dir: &Path,
    command: &[String],
    token: &str,
    request: &Request,
    removal: &mut meta_plugin::client::Finishing,
) -> Result<Vec<ProjectResult>> {
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    // one checkout after another, as `worktree exec` runs (at once only with
    // --parallel), whatever exec's own default
    match request.meta_options.typed_has("--parallel") {
        true => args.push("--parallel".into()),
        false => args.push("--sequential".into()),
    }
    if request.meta.verbose {
        args.push("--verbose".into());
    }
    if request.meta.silent {
        args.push("--silent".into());
    }
    args.extend(["exec".into(), "--".into()]);
    args.extend(command.iter().map(Into::into));
    let meta = Meta::from_env()?.cwd(dir).args(args).lease(token);
    // the removal waits for this run's end, even when this program is
    // killed outright
    if !request.meta.json {
        let started = meta.start_inherited()?;
        removal.running(started.id());
        let status = started.wait();
        removal.ended();
        let status = status?;
        if !status.success() {
            return Err(ErrorInfo::new(
                ErrorCode::ToolFailed,
                format!(
                    "`{}` failed in the ephemeral worktree ({status})",
                    escape(&meta_plugin::display::display_command(command))
                ),
            )
            .into());
        }
        return Ok(Vec::new());
    }
    let running = meta.json()?;
    removal.running(running.id());
    let envelope = running.wait();
    removal.ended();
    let envelope = envelope?;
    let mut rows = envelope.results;
    if let Some(error) = envelope.error {
        if !rows.iter().any(ProjectResult::is_failure) {
            rows.push(ProjectResult {
                error: Some(error),
                ..ProjectResult::default()
            });
        }
    }
    Ok(rows)
}
