use anyhow::Result;
use colored::*;
use meta_core::display::escape_lines;
use std::path::{Path, PathBuf};

use meta_cli::repo_filter;
use meta_cli::worktree::{discover_worktree_repos, WorktreeRepoInfo};
use meta_git_lib::worktree::helpers::*;

use super::cli_types::{CreateArgs, ExecArgs, RemoveArgs};
use crate::git_env;

/// Runs in each of `repos` that `--include`/`--exclude` select (by alias:
/// its path from `wt_dir`), naming them by that path.
fn build_loop_config(
    wt_dir: &Path,
    repos: &[WorktreeRepoInfo],
    include_filters: Vec<String>,
    exclude_filters: Vec<String>,
    parallel: bool,
    verbose: bool,
    json: bool,
) -> Result<loop_lib::LoopConfig> {
    let directories: Vec<String> = repos.iter().map(|r| r.path.display().to_string()).collect();
    // A set lays its repos out as the workspace, so the set's config names
    // them, by project name or path; only a filter reads it
    let names = if include_filters.is_empty() && exclude_filters.is_empty() {
        Vec::new()
    } else {
        let paths: Vec<PathBuf> = repos.iter().map(|r| r.path.clone()).collect();
        match meta_cli::worktree::set_config(wt_dir, &paths, None)? {
            Some(config) => {
                let projects = meta_core::config::Workspace::load(&config)?.projects;
                let tree = meta_cli::worktree::set_tree(wt_dir, &paths, &projects)?;
                repo_filter::repo_names(wt_dir, &projects, &tree)
            }
            None => Vec::new(),
        }
    };
    let directories = repo_filter::select(
        &include_filters,
        &exclude_filters,
        &directories,
        wt_dir,
        &names,
    )?;
    Ok(loop_lib::LoopConfig {
        directories,
        ignore: vec![],
        verbose,
        silent: false,
        parallel,
        dry_run: false,
        json_output: json,
        add_aliases_to_global_looprc: false,
        spawn_stagger_ms: 0,
        env: Some(git_env::git_env()),
        max_parallel: None,
        root_dir: Some(wt_dir.to_path_buf()),
    })
}

/// RAII guard that removes an ephemeral worktree.
/// `finish` removes it and reports failure; drop is the fallback that still
/// cleans up after an early return or a panic.
struct EphemeralGuard {
    name: String,
    verbose: bool,
    json: bool,
    finished: bool,
}

impl EphemeralGuard {
    fn remove(&self) -> Result<()> {
        if self.verbose {
            eprintln!("Removing ephemeral worktree '{}'...", self.name);
        }
        let remove_args = RemoveArgs {
            name: self.name.clone(),
            force: true,
        };
        super::remove::handle_remove(remove_args, self.verbose, self.json)
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
            eprintln!(
                "{} Failed to remove ephemeral worktree '{}': {}",
                "warning:".yellow().bold(),
                self.name,
                escape_lines(&format!("{e:#}"))
            );
            eprintln!(
                "  Run 'meta worktree remove {} --force' or 'meta worktree prune' to clean up.",
                self.name
            );
        }
    }
}

pub(crate) fn handle_exec(args: ExecArgs, verbose: bool, json: bool) -> Result<()> {
    if args.ephemeral {
        return handle_ephemeral_exec(args, verbose, json);
    }

    let name = &args.name;
    let repos = discover_and_validate_worktree(name)?;

    let config = build_loop_config(
        &resolve_existing_worktree(name)?,
        &repos,
        args.include,
        args.exclude,
        args.parallel,
        verbose,
        json,
    )?;

    loop_lib::run(&config, &args.command)?;
    Ok(())
}

fn handle_ephemeral_exec(args: ExecArgs, verbose: bool, json: bool) -> Result<()> {
    let name = args.name.clone();
    validate_worktree_name(&name)?;

    let cmd_parts = args.command;
    if cmd_parts.is_empty() {
        anyhow::bail!("No command specified after --");
    }

    // Extract filters and parallel flag before moving remaining args into CreateArgs
    let include_filters = args.include;
    let exclude_filters = args.exclude;
    let parallel = args.parallel;

    // Build CreateArgs from the exec args
    let create_args = CreateArgs {
        name: name.clone(),
        commit_ish: args.commit_ish,
        branch: args.branch,
        repos: args.repos,
        all: args.all,
        dry_run: false,
        from_pr: args.from_pr,
        ephemeral: true,
        ttl: None,
        custom_meta: args.custom_meta,
        strict: false,  // Exec creates ephemeral worktrees, don't fail on skipped repos
        no_deps: false, // Include dependencies by default
        recursive: false, // Ephemeral exec uses current meta level
    };

    if verbose {
        eprintln!("Creating ephemeral worktree '{name}'...");
    }
    // Ephemeral worktrees use global_strict=false to ensure creation succeeds
    super::create::handle_create(create_args, verbose, json, false)?;

    // Guard ensures cleanup even on panic
    let guard = EphemeralGuard {
        name: name.clone(),
        verbose,
        json,
        finished: false,
    };

    // Resolve worktree path for exec
    let cwd = std::env::current_dir()?;
    let configs = Configs::find_optional(&cwd)?;
    let worktree_root = resolve_worktree_root(configs.as_ref().map(|c| &c.owner), &cwd)?;
    let wt_dir = worktree_set_dir(&worktree_root, &name)?;

    // Run the command
    let repos = discover_worktree_repos(&wt_dir)?;

    let config = build_loop_config(
        &wt_dir,
        &repos,
        include_filters,
        exclude_filters,
        parallel,
        verbose,
        json,
    );

    let exec_result = config.and_then(|config| loop_lib::run(&config, &cmd_parts));

    // Clean up before propagating the result; a failed cleanup (such as a
    // checkout git cannot delete) fails the command too
    let cleanup = guard.finish();
    match (exec_result, cleanup) {
        (Err(e), Err(cleanup_error)) => {
            eprintln!(
                "{} {}",
                "warning:".yellow().bold(),
                escape_lines(&format!("{cleanup_error:#}"))
            );
            Err(e)
        }
        (Err(e), Ok(())) => Err(e),
        (Ok(()), cleanup) => cleanup,
    }
}
