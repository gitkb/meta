use anyhow::{Context, Result};
use colored::*;
use meta_core::display::{escape, escape_path};
use meta_core::ExecutionMode;

use meta_git_lib::worktree::helpers::*;
use meta_git_lib::worktree::hooks::fire_post_destroy;
use meta_git_lib::worktree::lease;
use meta_git_lib::worktree::safety::{check_worktree_removal, delete_checked_set};
use meta_git_lib::worktree::types::*;
use meta_git_lib::worktree::workspace::Workspace;

use super::cli_types::RemoveArgs;

pub(crate) fn handle_remove(
    args: RemoveArgs,
    mode: ExecutionMode,
    verbose: bool,
    json: bool,
) -> Result<()> {
    let name = &args.name;
    validate_worktree_name(name)?;
    let force = args.force;

    // Hooks come from the owner: inside the set being removed, its own
    // config goes with it
    let cwd = std::env::current_dir()?;
    let configs = Configs::find(&cwd)?;
    let worktree_root = resolve_worktree_root(Some(&configs.owner), &cwd)?;
    let wt_dir = worktree_root.join(name);

    if !wt_dir.exists() {
        anyhow::bail!("Worktree '{}' not found at {}", name, escape_path(&wt_dir));
    }

    let workspace = Workspace::of(&configs.here);
    // A set being made is its creator's, and one in use by its ephemeral
    // run is the run's (a run removing its own set holds its lease itself).
    // Otherwise the set's lease is held until the set is deleted, so no
    // create of it starts meanwhile; one its creator left marks a set it
    // never finished, which is meta's to remove, recorded or not. A preview
    // only looks.
    let set = wt_dir
        .canonicalize()
        .with_context(|| format!("Cannot resolve {}", escape_path(&wt_dir)))?;
    let home = lease::home(&workspace)?;
    // where the home was when its creator took it, too (see `lease::hold`)
    let found = std::iter::once(&home)
        .chain(&workspace.repos)
        .find_map(|repo| lease::find(repo, &set));
    let mine = found.as_ref().is_some_and(|found| found.is_mine());
    let held = match () {
        _ if mine => None,
        _ if mode.is_preview() => match &found {
            Some(found) if found.live => anyhow::bail!(
                "Refusing to remove worktree '{name}': it is {}. Wait for that to finish, or \
                 stop it first.",
                found.doing()
            ),
            _ => None,
        },
        _ => Some(lease::hold(&home, &workspace.repos, &set).map_err(|e| {
            anyhow::anyhow!(
                "Refusing to remove worktree '{name}': {e:#}. Wait for that to finish, or stop \
                 it first."
            )
        })?),
    };
    let left = match (&held, &found) {
        (Some((left, _)), _) => left.clone(),
        (None, Some(found)) if !mine => found.lease.clone(),
        _ => None,
    };
    // another workspace's lease, made from a repository both share, is no
    // authority here
    let abandoned = left.is_some_and(|lease| workspace.made_here(&lease.project));

    let check = check_worktree_removal(&wt_dir)?;
    // In an abandoned set, worktrees not recorded yet are the creator's: meta's
    let blockers: Vec<&str> = check.blockers_but(abandoned).map(String::as_str).collect();
    // Only this workspace's sets: a shared worktree directory may hold
    // another workspace's. No flag overrides this.
    let foreign = workspace.foreign_members(&wt_dir, &check);
    if !foreign.is_empty() {
        // Repositories meta couldn't read make sets from them look foreign
        let unreadable: String = workspace
            .unreadable
            .iter()
            .map(|(path, e)| format!("\n  {}: {}", escape_path(path), escape(&format!("{e:#}"))))
            .collect();
        anyhow::bail!(
            "Refusing to remove worktree '{name}': it is not a set of this workspace ({}):\n  {}\n\
             Remove it from the workspace that made it.{}",
            escape_path(&workspace.root),
            foreign.join("\n  "),
            if unreadable.is_empty() {
                String::new()
            } else {
                format!("\nParts of this workspace couldn't be read:{unreadable}")
            }
        );
    }
    if !blockers.is_empty() {
        anyhow::bail!(
            "Refusing to remove worktree '{name}':\n  {}\n\
             meta only deletes linked worktrees it can check; move or delete these yourself first.",
            blockers.join("\n  ")
        );
    }
    // Only delete what meta created: it records each worktree it makes.
    if check.record().is_none() && !abandoned {
        anyhow::bail!(
            "Refusing to remove {}: meta has no record of creating a worktree set there.\n{}",
            escape_path(&wt_dir),
            if check.members.is_empty() {
                "It holds no worktrees (any it had are gone, and their records with them); \
                 delete it yourself if you don't need what is left in it."
            } else {
                "If it was made by hand or by an older meta (which kept its records elsewhere), \
                 run 'git worktree remove' for each worktree in it, then delete it."
            }
        );
    }
    // A set folder may be a link, but to this set: one swapped for a link to
    // another set must not remove that one
    if let Some(made) = check.record().filter(|made| made.name() != *name) {
        anyhow::bail!(
            "Refusing to remove worktree '{name}': {} leads to worktree set '{}'; remove that \
             by its own name",
            escape_path(&wt_dir),
            escape(&made.name())
        );
    }
    let preview = mode.is_preview();
    if !check.unsaved.is_empty() {
        let found = check.unsaved.join("\n  ");
        if !force {
            anyhow::bail!(
                "Worktree '{name}' has work that removing it would lose:\n  {found}\n\
                 Commit and push it, or use --force to discard it."
            );
        }
        eprintln!(
            "{} {} work in worktree '{name}':\n  {found}",
            "warning:".yellow().bold(),
            if preview {
                "would discard"
            } else {
                "discarding"
            }
        );
    }

    if !preview {
        // Remove exactly what was checked, nested worktrees first
        delete_checked_set(&wt_dir, &check, force, verbose)?;
        if let Some((_, claim)) = held {
            claim.delete()?;
        }

        // Fire post-destroy hook
        fire_post_destroy(name, &wt_dir, force, Some(&configs.owner));
    }

    if json {
        let output = DestroyOutput {
            name: name.to_string(),
            path: wt_dir.display().to_string(),
            repos_removed: check.members.len(),
            dry_run: preview,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if preview {
        println!(
            "[DRY RUN] Would remove worktree '{}' at {}:",
            escape(name),
            escape_path(&wt_dir)
        );
        for member in &check.members {
            println!("  {}", escape_path(&member.path));
        }
    } else {
        println!("{} Removed worktree '{}'", "✓".green(), name.bold());
    }
    Ok(())
}
