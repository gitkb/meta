use anyhow::Result;
use colored::*;
use meta_core::display::{escape, escape_path};
use meta_core::ExecutionMode;
use meta_core::{errln, outln};

use meta_cli::worktree::discover_worktree_repos;
use meta_core::set_record as record;
use meta_git_lib::worktree::git_ops::{git_worktree_add, worktree_start};
use meta_git_lib::worktree::helpers::*;
use meta_git_lib::worktree::plan::Index;
use meta_git_lib::worktree::types::*;

use super::cli_types::AddArgs;

pub(crate) fn handle_add(
    args: AddArgs,
    mode: ExecutionMode,
    verbose: bool,
    json: bool,
) -> Result<()> {
    let name = &args.name;
    validate_worktree_name(name)?;

    let repo_specs = &args.repos;

    // Check for "." alias
    if repo_specs.iter().any(|r| r.alias == ".") {
        anyhow::bail!(
            "Cannot add '.' to an existing worktree. The meta repo root can only be established at create time.\n\
             Use 'meta git worktree remove {name}' then 'meta git worktree create {name} --repo . ...' instead."
        );
    }

    let cwd = std::env::current_dir()?;
    let configs = Configs::find(&cwd)?;
    let meta_dir = &configs.here.dir;
    let worktree_root = resolve_worktree_root(Some(&configs.owner), &cwd)?;
    let wt_dir = worktree_root.join(name);

    if !wt_dir.exists() {
        anyhow::bail!(
            "Worktree '{}' not found at {}",
            escape(name),
            escape_path(&wt_dir)
        );
    }

    // Check existing repos in the worktree
    let existing = discover_worktree_repos(&wt_dir)?;

    // What is added joins the set's record, if meta made the set; one it
    // didn't make stays unrecorded, so `remove` keeps refusing it.
    let set_record = record::find(&wt_dir, existing.iter().map(|r| &r.path));

    // Check every repo before adding any: one refused after others were
    // added would leave a set the caller didn't ask for
    let index = Index::new(&configs.here);
    let mut planned: Vec<(&RepoSpec, std::path::PathBuf, String)> = Vec::new();
    for spec in repo_specs {
        // by name, else by place, as in create
        let source = index.find(&spec.alias)?.source;
        // as in create: no worktree of a project not cloned, nor of the
        // repository around a directory that is none
        if let Some(why) = super::not_a_repository(&spec.alias, &source) {
            anyhow::bail!("{why}");
        }
        // The repo goes where it is in the workspace
        let place = place_in_set(meta_dir, &spec.alias, &source);
        // As in create: a repo given twice, by name or by path, is one
        // worktree, and refused on two branches
        if let Some((first, ..)) = planned.iter().find(|(_, _, p)| same_place(p, &place)) {
            let first_branch = resolve_branch(name, None, first.branch.as_deref());
            let branch = resolve_branch(name, None, spec.branch.as_deref());
            if first_branch != branch {
                anyhow::bail!(
                    "Repo '{}' is given twice, on branches '{}' and '{}'",
                    escape(&place),
                    escape(&first_branch),
                    escape(&branch)
                );
            }
            continue;
        }
        // A checkout already in the worktree (such as the root repo's) may
        // hold a symlink where this one goes; git must not follow it out
        let dest = dest_in_set(&wt_dir, &place).map_err(|why| {
            anyhow::anyhow!(
                "Refusing to add the worktree for '{}': {why}",
                escape(&spec.alias)
            )
        })?;
        // There already, under its place or where a link there leads, or
        // nested in a member (`parent/leaf`); or its place holds other files
        let real = |path: &std::path::Path| path.canonicalize().ok();
        let member = dest.join(".git").symlink_metadata().is_ok()
            || existing.iter().any(|r| {
                same_place(&r.alias, &place)
                    || (real(&r.path).is_some() && real(&r.path) == real(&dest))
            });
        if member {
            anyhow::bail!(
                "Repo '{}' already exists in worktree '{}'",
                escape(&spec.alias),
                escape(name)
            );
        }
        let taken = std::fs::read_dir(&dest)
            .map_or(dest.symlink_metadata().is_ok(), |mut entries| {
                entries.next().is_some()
            });
        if taken {
            anyhow::bail!(
                "{} in worktree '{}' already holds files that aren't a checkout of '{}'; move them away first",
                escape_path(&dest),
                escape(name),
                escape(&spec.alias)
            );
        }
        planned.push((spec, source, place));
    }

    if mode.is_preview() {
        let mut repos = Vec::new();
        for (spec, source, place) in &planned {
            // what the run checks before adding
            let branch = resolve_branch(name, None, spec.branch.as_deref());
            let start = worktree_start(source, &branch, None)?;
            repos.push(CreateRepoEntry {
                alias: spec.alias.clone(),
                path: wt_dir.join(place).display().to_string(),
                created_branch: start.new_branch_at().is_some(),
                branch,
            });
        }
        if json {
            let output = AddOutput {
                name: name.to_string(),
                repos,
                dry_run: true,
            };
            outln!("{}", serde_json::to_string_pretty(&output)?)?;
        } else {
            outln!("[DRY RUN] Would add to worktree set '{}':", escape(name))?;
            for r in &repos {
                outln!(
                    "  {} at {} (branch: {}{})",
                    escape(&r.alias),
                    escape(&r.path),
                    escape(&r.branch),
                    if r.created_branch { ", new" } else { "" }
                )?;
            }
        }
        return Ok(());
    }

    let mut added = Vec::new();
    let mut failed = None;
    for (spec, source, place) in planned {
        let branch = resolve_branch(name, None, spec.branch.as_deref());
        // Again now: a checkout added above (a parent repo's) may have put a
        // symlink where this one goes
        let dest = match dest_in_set(&wt_dir, &place) {
            Ok(dest) => dest,
            Err(why) => {
                failed = Some(anyhow::anyhow!(
                    "Refusing to add the worktree for '{}': {why}",
                    escape(&spec.alias)
                ));
                break;
            }
        };

        // Ensure parent directories exist for nested paths (e.g., "vendor/")
        if let Some(parent) = dest.parent() {
            if parent != wt_dir {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    failed = Some(
                        anyhow::Error::from(e)
                            .context(format!("Cannot create {}", escape_path(parent))),
                    );
                    break;
                }
            }
        }

        if verbose {
            errln!(
                "Adding worktree for '{}' at {} (branch: {})",
                escape(&spec.alias),
                escape_path(&dest),
                escape(&branch)
            );
        }

        // A failed add stops here; what was added is reported
        let created_branch = match git_worktree_add(&source, &dest, &branch, None) {
            Ok(created) => created.is_some(),
            Err(e) => {
                failed = Some(e);
                break;
            }
        };
        // A worktree that can't be recorded is reported as failed, not added
        if let Some(set_record) = &set_record {
            if let Err(e) = meta_core::gitdir::linked(&dest)
                .and_then(|linked| record::write(&linked.admin, set_record))
            {
                failed = Some(e.context(format!(
                    "Added '{}', but cannot record it in worktree '{name}'",
                    escape(&spec.alias)
                )));
                break;
            }
        }
        added.push(CreateRepoEntry {
            alias: spec.alias.clone(),
            path: dest.display().to_string(),
            branch,
            created_branch,
        });
    }

    if json {
        let output = AddOutput {
            name: name.to_string(),
            repos: added,
            dry_run: false,
        };
        outln!("{}", serde_json::to_string_pretty(&output)?)?;
    } else {
        for r in &added {
            let branch_note = if r.created_branch { " (new)" } else { "" };
            outln!(
                "{} Added '{}' to worktree '{}' (branch: {}{})",
                "✓".green(),
                escape(&r.alias),
                name,
                escape(&r.branch),
                branch_note
            )?;
        }
    }

    failed.map_or(Ok(()), Err)
}

/// Whether two places in a set name the same directory. Discovery spells a
/// place with the platform's separator (`vendor\\app` on Windows), plans with
/// `/`; as paths they compare equal.
fn same_place(a: &str, b: &str) -> bool {
    std::path::Path::new(a) == std::path::Path::new(b)
}

#[cfg(test)]
mod tests {
    use super::same_place;

    #[test]
    fn places_compare_as_paths() {
        assert!(same_place("vendor/app", "vendor/app"));
        assert!(!same_place("vendor/app", "vendor/other"));
        #[cfg(windows)]
        assert!(same_place("vendor\\app", "vendor/app"));
    }
}
