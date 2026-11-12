use anyhow::{Context, Result};
use colored::*;
use meta_plugin::display::{escape, escape_path};
use meta_plugin::{errln, outln};
use meta_plugin::{Ctx, ProjectResult, Request, Response};

use crate::here::Here;
use crate::worktree::git_ops::{
    ensure_checkout_config_is_not_bare, git_worktree_add, worktree_branch_is_free, worktree_start,
    WorktreeStart,
};
use crate::worktree::helpers::*;
use crate::worktree::origin::{branches_on_origin, Wanted};
use crate::worktree::plan::{Index, PlannedRepo, StartBranches};
use crate::worktree::scan::discover_worktree_repos;
use crate::worktree::types::*;
use meta_core::set_record as record;

use super::cli_types::AddArgs;

/// Add the repos `args` names to the set, in the workspace `request` runs
/// in: one record per repo added (with `--dry-run`, that would be).
pub(crate) fn handle_add(args: AddArgs, ctx: &Ctx, request: &Request) -> Result<Response> {
    let (mode, verbose, json) = (request.meta.mode(), request.meta.verbose, request.meta.json);
    let name = &args.name;
    meta_plugin::sets::validate_name(name)?;
    let from_ref = args.commit_ish.as_deref();

    let repo_specs = args.repos.clone();

    // Check for "." alias
    if repo_specs.iter().any(|r| r.alias == ".") {
        anyhow::bail!(
            "Cannot add '.' to an existing worktree. The meta repo root can only be established at create time.\n\
             Use 'meta git worktree remove {name}' then 'meta git worktree create {name} --repo . ...' instead."
        );
    }

    let here = Here::need(request)?;
    // each project's branch:, read only when a new branch starts from one
    let start_branches =
        StartBranches::new(&here.dir, &here.workspace.root, &here.workspace.config);
    let meta_dir = &here.dir;
    let wt_dir = crate::here::worktree_root(request)?.join(name);

    if !wt_dir.exists() {
        anyhow::bail!(
            "Worktree '{}' not found at {}",
            escape(name),
            escape_path(&wt_dir)
        );
    }

    // Check existing repos in the worktree
    let existing = discover_worktree_repos(&wt_dir, &here.places())?;

    // What is added joins the set's record, if meta made the set; one it
    // didn't make stays unrecorded, so `remove` keeps refusing it.
    let set_record = record::find(&wt_dir, existing.iter().map(|r| &r.path));

    // Check every repo before adding any: one refused after others were
    // added would leave a set the caller didn't ask for
    let index = Index::new(&here);
    // As in create: a meta repo that isn't cloned on the way to a repo asked
    // for is cloned first, and a repo asked for that isn't cloned too
    let wanted: Vec<String> = repo_specs.iter().map(|r| r.alias.clone()).collect();
    let parents = super::create::uncloned_meta_parents(&index, &wanted, name);
    if !parents.is_empty() {
        if mode.is_preview() {
            let places: Vec<_> = parents.iter().map(|r| escape(&r.place)).collect();
            outln!(
                "[DRY RUN] Would clone the meta repos on the way to the repos to add ({}), \
                 then add them to worktree set '{}'; nothing was cloned or added.",
                places.join(", "),
                escape(name)
            )?;
            return Ok(Response::done());
        }
        // the clones list more: from the top again
        super::create::clone_missing(&index, &parents)?;
        return crate::again(ctx, request);
    }
    // Resolve each explicit spec once; synthetic parents already carry
    // their source identity and must never be looked up by name again.
    let mut resolved = repo_specs
        .iter()
        .map(|spec| Ok((spec.clone(), index.find(&spec.alias)?.source)))
        .collect::<Result<Vec<_>>>()?;
    let nested = resolved
        .iter()
        .map(|(spec, source)| {
            (
                place_in_set(meta_dir, &spec.alias, source),
                spec.alias.clone(),
            )
        })
        .collect::<Vec<_>>();
    for (parent, _) in crate::worktree::plan::intermediate_parents(&index, nested, ctx)? {
        let place = place_in_set(meta_dir, &parent.key, &parent.source);
        if let Some(occupant) = existing.iter().find(|r| same_place(&r.alias, &place)) {
            let identity = |path: &std::path::Path| -> Result<std::path::PathBuf> {
                meta_core::gitdir::common_dir(path)?
                    .with_context(|| format!("{} is not a repository", escape_path(path)))?
                    .canonicalize()
                    .map_err(Into::into)
            };
            anyhow::ensure!(
                identity(&occupant.path)? == identity(&parent.source)?,
                "Intermediate parent '{}' is occupied by a checkout of another repository at {}",
                escape(&parent.key),
                escape_path(&occupant.path)
            );
            continue;
        }
        if resolved.iter().any(|(_, source)| *source == parent.source) {
            continue;
        }
        let checkout = meta_core::gitdir::Checkout::of(&parent.source);
        if checkout != meta_core::gitdir::Checkout::Missing {
            if let Some(problem) = checkout.problem() {
                ctx.warn_or_bail(format!(
                    "Intermediate parent '{}' for nested repo: {}",
                    escape(&parent.key),
                    escape(&problem)
                ))?;
                continue;
            }
        }
        resolved.push((
            RepoSpec {
                alias: parent.key,
                branch: None,
            },
            parent.source,
        ));
    }
    let mut missing: Vec<PlannedRepo> = Vec::new();
    for (spec, source) in &resolved {
        if meta_core::gitdir::Checkout::of(source) == meta_core::gitdir::Checkout::Missing
            && !missing.iter().any(|r| r.source == *source)
        {
            missing.push(PlannedRepo {
                place: place_in_set(meta_dir, &spec.alias, source),
                project: spec.alias.clone(),
                source: source.clone(),
                branch: resolve_branch(name, None, spec.branch.as_deref()),
            });
        }
    }
    if !missing.is_empty() {
        if mode.is_preview() {
            let places: Vec<_> = missing.iter().map(|r| escape(&r.place)).collect();
            outln!(
                "[DRY RUN] Would clone the repos to add that aren't cloned ({}), then add \
                 them to worktree set '{}'; nothing was cloned or added.",
                places.join(", "),
                escape(name)
            )?;
            return Ok(Response::done());
        }
        super::create::clone_missing(&index, &missing)?;
    }
    let mut planned: Vec<(&RepoSpec, std::path::PathBuf, String)> = Vec::new();
    for (spec, source) in &resolved {
        let source = source.clone();
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

    // Parents must be checked out before their children, as in create.
    planned.sort_by_key(|(_, _, place)| place.split('/').count());

    let branches: Vec<String> = planned
        .iter()
        .map(|(spec, ..)| resolve_branch(name, None, spec.branch.as_deref()))
        .collect();
    let wanted: Vec<Wanted> = planned
        .iter()
        .zip(&branches)
        .map(|((spec, source, _), branch)| Wanted {
            project: &spec.alias,
            source,
            branch,
        })
        .collect();
    if from_ref.is_none() {
        for repo in &wanted {
            worktree_branch_is_free(repo.source, repo.branch)?;
        }
    }
    let on_origin = branches_on_origin(&wanted, from_ref, ctx)?;

    if mode.is_preview() {
        let mut repos = Vec::new();
        for (spec, source, place) in &planned {
            // what the run checks before adding
            let branch = resolve_branch(name, None, spec.branch.as_deref());
            let start = match on_origin.get(source) {
                Some(commit) => {
                    ensure_checkout_config_is_not_bare(source)?;
                    WorktreeStart::Remote {
                        commit: commit.clone(),
                    }
                }
                None => worktree_start(source, &branch, from_ref, &|| start_branches.of(source))?,
            };
            repos.push(CreateRepoEntry {
                alias: spec.alias.clone(),
                path: wt_dir.join(place).display().to_string(),
                started_from: start.base(&branch),
                created_branch: start.new_branch_at().is_some(),
                branch,
            });
        }
        if !json {
            outln!("[DRY RUN] Would add to worktree set '{}':", escape(name))?;
            for r in &repos {
                outln!(
                    "  {} at {} (branch: {}){}",
                    escape(&r.alias),
                    escape(&r.path),
                    escape(&r.branch),
                    super::create::new_branch_note(r)
                )?;
            }
        }
        return Ok(Response::printed(records_of(&wt_dir, &repos)));
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
        let made = git_worktree_add(&source, &dest, &branch, from_ref, &|| {
            start_branches.of(&source)
        });
        let (created_branch, started_from) = match made {
            Ok(start) => {
                let base = start.base(&branch);
                (start.new_branch_at().is_some(), base)
            }
            Err(e) => {
                failed = Some(e);
                break;
            }
        };
        // A worktree that can't be recorded is reported as failed, not added
        if let Some(set_record) = &set_record {
            if let Err(e) = meta_core::gitdir::linked(&dest).and_then(|linked| {
                record::write(&linked.admin, set_record)?;
                // so remove can delete the branch again, once nothing is lost
                match created_branch {
                    true => record::write_created_branch(&linked.admin, &branch),
                    false => Ok(()),
                }
            }) {
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
            started_from,
        });
    }

    if !json {
        for r in &added {
            outln!(
                "{} Added '{}' to worktree '{}' (branch: {}){}",
                "✓".green(),
                escape(&r.alias),
                name,
                escape(&r.branch),
                super::create::new_branch_note(r)
            )?;
        }
    }

    crate::printed(ctx, records_of(&wt_dir, &added), failed)
}

/// One record per repo of the set at `wt_dir`: its place in the set names
/// it, as the set's checkouts are named.
pub(super) fn records_of(
    wt_dir: &std::path::Path,
    repos: &[CreateRepoEntry],
) -> Vec<ProjectResult> {
    repos
        .iter()
        .map(|r| {
            let path = std::path::Path::new(&r.path);
            let place = match path.strip_prefix(wt_dir) {
                Ok(place) if place.as_os_str().is_empty() => ".".to_string(),
                Ok(place) => place.to_string_lossy().into_owned(),
                Err(_) => r.alias.clone(),
            };
            ProjectResult::record(place, r).at(path)
        })
        .collect()
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
