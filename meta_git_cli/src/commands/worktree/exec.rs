use anyhow::Result;
use meta_plugin::display::escape;
use meta_plugin::{errln, outln};
use meta_plugin::{Ctx, Plan, Request, Response, Scope, Step};
use std::path::{Path, PathBuf};

use meta_core::worktree::{discover_worktree_repos, WorktreeRepoInfo};
use meta_git_lib::worktree::helpers::*;

use super::cli_types::{CreateArgs, ExecArgs, RemoveArgs};
use crate::git_env;

/// RAII guard that removes an ephemeral worktree, at the directory it was
/// made in. `finish` removes it and reports failure; drop is the fallback
/// that still cleans up after an early return or a panic.
struct EphemeralGuard<'a> {
    name: String,
    dir: PathBuf,
    ctx: &'a Ctx,
    request: &'a Request,
    finished: bool,
}

impl EphemeralGuard<'_> {
    fn remove(&self) -> Result<()> {
        let verbose = self.request.meta.verbose;
        if verbose {
            errln!("Removing ephemeral worktree '{}'...", self.name);
        }
        let remove_args = RemoveArgs {
            name: self.name.clone(),
            force: true,
        };
        // Only a set this run really created has a guard; its hooks warn
        let configs = super::workspace_configs(self.request)?;
        let how = super::remove::How {
            preview: false,
            verbose,
            json: self.request.meta.json,
            strict: false,
        };
        super::remove::remove_set(
            &remove_args,
            &self.dir,
            &configs,
            self.ctx,
            how,
            &mut Vec::new(),
        )
    }

    fn finish(mut self) -> Result<()> {
        self.finished = true;
        self.remove()
    }
}

impl Drop for EphemeralGuard<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Err(e) = self.remove() {
            self.ctx.warn(format!(
                "Failed to remove ephemeral worktree '{}': {e:#}\n  \
                 Run 'meta git worktree remove {} --force' or 'meta git worktree prune' to clean up.",
                self.name, self.name
            ));
        }
    }
}

/// `exec NAME -- CMD`: a plan of `CMD` in each checkout of the set meta's
/// `--include`/`--exclude`/`--tag` select (the host resolved them), named by
/// its path in the set; `--ephemeral`: make the set, run it there, remove
/// the set.
pub(crate) fn handle_exec(args: ExecArgs, ctx: &Ctx, request: &Request) -> Result<Response> {
    if args.ephemeral {
        return handle_ephemeral_exec(args, ctx, request);
    }

    let configs = super::configs(request)?;
    let owner = configs.as_ref().map(|c| &c.owner);
    let (_, repos) = discover_and_validate_worktree(&args.name, owner, &request.cwd)?;
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

/// The loop that runs `command` in each repo of the set at `wt_dir`.
fn loop_config(
    wt_dir: &Path,
    repos: &[WorktreeRepoInfo],
    request: &Request,
) -> Result<loop_lib::LoopConfig> {
    Ok(loop_lib::LoopConfig {
        directories: repos.iter().map(|r| r.path.clone()).collect(),
        verbose: request.meta.verbose,
        silent: request.meta.silent,
        parallel: request.meta_options.typed_has("--parallel"),
        dry_run: request.meta.dry_run,
        json_output: request.meta.json,
        env: Some(git_env::git_env()),
        aliases: loop_lib::aliases()?,
        root_dir: Some(wt_dir.to_path_buf()),
        ..Default::default()
    })
}

fn handle_ephemeral_exec(args: ExecArgs, ctx: &Ctx, request: &Request) -> Result<Response> {
    let verbose = request.meta.verbose;
    let name = args.name.clone();
    meta_core::worktree::validate_name(&name)?;

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
    // this process is gone, however it ends, prune removes the set
    let made = super::create::handle_create(create_args, &lenient, request, &mut Vec::new());
    for warning in lenient.finish(Response::done()).warnings {
        ctx.warn(warning);
    }
    // A preview makes no set
    let Some(created) = made? else {
        outln!(
            "Then would run `{}` in each repo planned above, and remove the set.",
            escape(&loop_lib::display_command(cmd_parts))
        )?;
        return Ok(Response::done());
    };

    // Guard ensures cleanup even on panic
    let guard = EphemeralGuard {
        name: name.clone(),
        dir: created.dir.clone(),
        ctx,
        request,
        finished: false,
    };

    // Run the command in the set just made; whatever fails, the cleanup
    // below runs
    let exec_result = discover_worktree_repos(&created.dir)
        .and_then(|repos| loop_config(&created.dir, &repos, request))
        .and_then(|config| loop_lib::run(&config, cmd_parts));

    // Clean up before propagating the result; a failed cleanup (such as a
    // checkout git cannot delete) fails the command too
    let cleanup = guard.finish();
    // a set this run couldn't remove is the next prune's: its lease stays
    match &cleanup {
        Err(_) => created.lease.leave(),
        Ok(()) => drop(created.lease),
    }
    match (exec_result, cleanup) {
        (Err(e), Err(cleanup_error)) => {
            ctx.warn(format!("{cleanup_error:#}"));
            Err(e)
        }
        (Err(e), Ok(())) => Err(e),
        // a command that failed stays the failure; the cleanup only warns
        (Ok(outcome), Err(cleanup_error)) if outcome.failed() > 0 => {
            ctx.warn(format!("{cleanup_error:#}"));
            Ok(Response::printed(outcome.rows()))
        }
        (Ok(_), Err(cleanup_error)) => Err(cleanup_error),
        // a command that failed fails the run, by its row
        (Ok(outcome), Ok(())) => Ok(Response::printed(outcome.rows())),
    }
}
