use anyhow::Result;
use meta_core::display::escape;
use meta_core::ExecutionMode;
use meta_core::{errln, outln};
use meta_plugin_protocol::PluginRequestOptions;
use std::path::{Path, PathBuf};

use meta_core::repo_filter;
use meta_core::worktree::{discover_worktree_repos, WorktreeRepoInfo};
use meta_git_lib::worktree::helpers::*;

use super::cli_types::{CreateArgs, ExecArgs, RemoveArgs};
use crate::git_env;

/// meta's `--include`, `--exclude` and `--tag`
fn filters(options: &PluginRequestOptions) -> repo_filter::Filters {
    repo_filter::Filters {
        include: options.include_filters.clone().unwrap_or_default(),
        exclude: options.exclude_filters.clone().unwrap_or_default(),
        tags: options.tag_filters.clone().unwrap_or_default(),
    }
}

/// Runs in each of `repos` that meta's `--include`/`--exclude`/`--tag`
/// select (by name, or alias: its path from `wt_dir`), naming them by that
/// path; at once only with meta's `--parallel` (meta sends it as typed for
/// worktree commands: the config's default is for meta's own loop).
fn build_loop_config(
    wt_dir: &Path,
    repos: &[WorktreeRepoInfo],
    options: &PluginRequestOptions,
) -> Result<loop_lib::LoopConfig> {
    let filters = filters(options);
    let paths: Vec<PathBuf> = repos.iter().map(|r| r.path.clone()).collect();
    // A set lays its repos out as the workspace, so the set's config names
    // them, by project name or path; only a filter reads it
    let workspace = if filters.is_empty() {
        None
    } else {
        meta_core::worktree::set_config(wt_dir, &paths, None)?
            .map(|config| meta_core::config::Workspace::load(&config))
            .transpose()?
    };
    Ok(loop_lib::LoopConfig {
        directories: meta_core::worktree::select_in_set(
            wt_dir,
            &paths,
            workspace.as_ref(),
            &filters,
        )?,
        verbose: options.verbose,
        parallel: options.parallel,
        dry_run: options.mode().is_preview(),
        json_output: options.json_output,
        env: Some(git_env::git_env()),
        aliases: loop_lib::aliases()?,
        root_dir: Some(wt_dir.to_path_buf()),
        ..Default::default()
    })
}

/// RAII guard that removes an ephemeral worktree, at the directory it was
/// made in. `finish` removes it and reports failure; drop is the fallback
/// that still cleans up after an early return or a panic.
struct EphemeralGuard {
    name: String,
    dir: PathBuf,
    verbose: bool,
    json: bool,
    finished: bool,
}

impl EphemeralGuard {
    fn remove(&self) -> Result<()> {
        if self.verbose {
            errln!("Removing ephemeral worktree '{}'...", self.name);
        }
        let remove_args = RemoveArgs {
            name: self.name.clone(),
            force: true,
        };
        // Only a set this run really created has a guard; its hooks warn
        let configs = Configs::find(&std::env::current_dir()?)?;
        super::remove::remove_set(
            &remove_args,
            &self.dir,
            &configs,
            ExecutionMode::Execute,
            self.verbose,
            self.json,
            false,
        )
    }

    fn finish(mut self) -> Result<()> {
        self.finished = true;
        self.remove()
    }
}

impl Drop for EphemeralGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Err(e) = self.remove() {
            meta_core::out::warn(format!(
                "Failed to remove ephemeral worktree '{}': {e:#}\n  \
                 Run 'meta git worktree remove {} --force' or 'meta git worktree prune' to clean up.",
                self.name, self.name
            ));
        }
    }
}

pub(crate) fn handle_exec(args: ExecArgs, options: &PluginRequestOptions) -> Result<()> {
    if args.ephemeral {
        return handle_ephemeral_exec(args, options);
    }

    let (wt_dir, repos) = discover_and_validate_worktree(&args.name)?;
    let config = build_loop_config(&wt_dir, &repos, options)?;

    loop_lib::run(&config, &args.command)?;
    Ok(())
}

fn handle_ephemeral_exec(args: ExecArgs, options: &PluginRequestOptions) -> Result<()> {
    let (mode, verbose, json) = (options.mode(), options.verbose, options.json_output);
    let name = args.name.clone();
    validate_worktree_name(&name)?;

    let cmd_parts = &args.command;
    if cmd_parts.is_empty() {
        anyhow::bail!("No command specified after --");
    }
    // A run names the set's repos by the .meta.yaml checked out in it, which
    // a preview hasn't made (a start commit or an existing branch may hold
    // another): it can't tell what a filter selects
    if mode.is_preview() && !filters(options).is_empty() {
        anyhow::bail!(
            "--dry-run can't preview --include/--exclude of exec --ephemeral: the set's own \
             .meta.yaml names its repos; nothing was changed"
        );
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
        recursive: options.recursive,
        from_root: options.all,
        depth: options.depth,
    };

    if verbose {
        errln!("Creating ephemeral worktree '{name}'...");
    }
    // Ephemeral worktrees use global_strict=false to ensure creation succeeds
    // The lease, held for the run: the set lives as long as it does, and once
    // this process is gone, however it ends, prune removes the set
    // A preview makes no set
    let Some(created) = super::create::handle_create(create_args, mode, verbose, json, false)?
    else {
        outln!(
            "Then would run `{}` in each repo planned above, and remove the set.",
            escape(&loop_lib::display_command(cmd_parts))
        )?;
        return Ok(());
    };

    // Guard ensures cleanup even on panic
    let guard = EphemeralGuard {
        name: name.clone(),
        dir: created.dir.clone(),
        verbose,
        json,
        finished: false,
    };

    // Run the command in the set just made; whatever fails, the cleanup
    // below runs
    let exec_result = discover_worktree_repos(&created.dir)
        .and_then(|repos| build_loop_config(&created.dir, &repos, options))
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
            meta_core::out::warn(format!("{cleanup_error:#}"));
            Err(e)
        }
        (Err(e), Ok(())) => Err(e),
        (Ok(()), cleanup) => cleanup,
    }
}
