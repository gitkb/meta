use anyhow::{Context, Result};
use chrono::Utc;
use colored::*;
use meta_plugin::display::{escape, escape_path};
use meta_plugin::{errln, outln};
use meta_plugin::{Ctx, ProjectResult, Request, Skipped};
use std::collections::BTreeSet;
use std::collections::HashMap;

use crate::here::Here;
use crate::worktree::git_ops::*;
use crate::worktree::helpers::*;
use crate::worktree::hooks::fire_post_create;
use crate::worktree::plan::{Found, Index, PlannedRepo};
use crate::worktree::safety::{check_worktree_removal, delete_checked_set};
use crate::worktree::types::{CreateOutput, CreatePlan, CreatePlanRepo, CreateRepoEntry};
use crate::worktree::RepoSpec;
use meta_core::set_record::{self as record, SetRecord};
use meta_plugin::duration::format_duration;
use meta_plugin::sets::lease;

use super::cli_types::CreateArgs;
use meta_plugin::display::shell_quote;

/// A set a create made: its directory, and its lease, held until dropped.
pub(crate) struct Created {
    pub dir: std::path::PathBuf,
    pub lease: lease::Held,
}

/// What a create did.
pub(crate) enum Made {
    /// Said what it would do (`--dry-run`), with the repos it must clone
    /// first when that stops it short of a plan
    Preview(Vec<Skipped>),
    Created(Created),
    /// Cloned what the set needs that wasn't cloned: the configs those
    /// clones hold change the plan, so the command runs again from the top
    Again,
}

/// Make the set `args` asks for in the workspace `request` runs in (with
/// meta's `-r`, resolving dependencies across the meta repos nested under
/// its config; with `-A`, from the outermost); `None` for a preview, which
/// makes none. The set made is in `records`, for `--json`, even when what
/// follows its making (a hook) fails.
pub(crate) fn handle_create(
    args: CreateArgs,
    ctx: &Ctx,
    request: &Request,
    records: &mut Vec<ProjectResult>,
) -> Result<Made> {
    let (mode, verbose) = (request.meta.mode(), request.meta.verbose);

    let name = &args.name;
    meta_plugin::sets::validate_name(name)?;

    let ephemeral = args.ephemeral;
    let ttl_seconds = args.ttl;
    // Parse custom metadata, collecting any invalid entries for strict mode
    let mut custom_meta = HashMap::new();
    for s in &args.custom_meta {
        if let Some((key, value)) = s.split_once('=') {
            custom_meta.insert(key.to_string(), value.to_string());
        } else {
            ctx.warn_or_bail(format!(
                "--meta value '{}' missing '=' separator (expected key=value), skipping",
                escape(s)
            ))?;
        }
    }
    let from_ref = args.commit_ish.as_deref();
    let from_pr_spec = args.from_pr.as_deref();

    // meta's -r or -A: the host sends how deep, and the config they start
    // from (-A: the root of the workspace tree; -r: the nearest .meta.yaml)
    let depth = request.depth;
    let recursive = request.recursive;

    if args.repos.is_empty() && !args.every_repo {
        anyhow::bail!("Specify repos with --repo <alias> or use --every-repo");
    }
    let here = Here::need(request)?;
    let worktree_root = here.worktrees()?;

    // Check if worktree already exists
    let wt_dir = worktree_root.join(name);
    if wt_dir.exists() {
        let lease = wt_dir.canonicalize().ok().and_then(|set| lease::find(&set));
        anyhow::bail!(
            "Worktree '{name}' already exists at {}{}. Use 'meta git worktree remove {name}' first.",
            escape_path(&wt_dir),
            match lease {
                Some(found) if found.live => format!(" ({})", found.doing()),
                Some(found) => format!(" (left half-made by {}, which is gone)", found.holder()),
                None => String::new(),
            }
        );
    }

    // The repos of the set, each resolved once: through one index of the
    // projects (the whole tree, as meta read it, with --recursive)
    let index = match recursive {
        true => Index::recursive(&here, depth),
        false => Index::new(&here),
    };
    // A repo asked for inside a meta repo that isn't cloned is unknown
    // until that meta repo is, and so is one on the way to a project they
    // depend on (open-source for open-source/atc), which only that meta
    // repo's .meta.yaml lists: it is cloned first. The repos the set needs
    // that aren't cloned are cloned too, as clone-missing would. A clone may
    // list more (a nested meta repo's projects): then the command runs
    // again, planning from the configs as they are now.
    let mut wanted: Vec<String> = args.repos.iter().map(|r| r.alias.clone()).collect();
    if !args.no_deps {
        wanted.extend(unresolved_dependencies(&index, &args.repos));
    }
    let parents = uncloned_meta_parents(&index, &wanted, name);
    if !parents.is_empty() {
        if mode.is_preview() {
            let places: Vec<_> = parents.iter().map(|r| escape(&r.place)).collect();
            outln!(
                "[DRY RUN] Would clone the meta repos on the way to the repos asked for \
                 ({}), then plan worktree set '{}' again; nothing was cloned or created.",
                places.join(", "),
                escape(name)
            )?;
            return Ok(Made::Preview(must_clone_first(&parents)));
        }
        clone_missing(&index, &parents)?;
        return Ok(Made::Again);
    }
    let planned = plan_repos(&index, &args, ctx, verbose)?;
    let mut repos_to_create = planned.repos;
    if !planned.missing.is_empty() {
        if mode.is_preview() {
            let places: Vec<_> = planned.missing.iter().map(|r| escape(&r.place)).collect();
            outln!(
                "[DRY RUN] Would clone the repos the set needs that aren't cloned ({}), then \
                 create worktree set '{}'; nothing was cloned or created.",
                places.join(", "),
                escape(name)
            )?;
            return Ok(Made::Preview(must_clone_first(&planned.missing)));
        }
        clone_missing(&index, &planned.missing)?;
        return Ok(Made::Again);
    }

    // A preview makes the checks a run makes (all but asking GitHub for a
    // PR's branch), skips what it skips and fails where it fails
    if mode.is_preview() {
        if let Some(spec) = from_pr_spec {
            let (pr_repo, pr_num) = parse_from_pr(spec)?;
            let Some(repo) = repos_to_create
                .iter_mut()
                .find(|repo| repo.place != "." && repo_matches_spec(&repo.source, &pr_repo))
            else {
                anyhow::bail!(
                    "No selected repo has origin '{}', so PR #{pr_num} can't be applied",
                    escape(&pr_repo)
                );
            };
            repo.branch = format!("<the head branch of {spec}>");
        }
        let mut kept = Vec::new();
        for repo in repos_to_create {
            match missing_ref(&repo.source, from_ref)? {
                Some(r) => ctx.warn_or_bail(format!(
                    "Skipping '{}': ref '{}' not found",
                    escape(&repo.project),
                    escape(r)
                ))?,
                None => kept.push(repo),
            }
        }
        if kept.is_empty() {
            match from_ref {
                Some(r) => anyhow::bail!(
                    "No selected repo has ref '{}'; no worktree was created",
                    escape(r)
                ),
                None => anyhow::bail!("No repos selected; no worktree was created"),
            }
        }
        // what the run checks before each add; a PR's branch is fetched
        // first, so its repo gets only the check that doesn't need it
        let pr_branch = from_pr_spec.map(|spec| format!("<the head branch of {spec}>"));
        for repo in &kept {
            if pr_branch.as_ref() == Some(&repo.branch) {
                ensure_checkout_config_is_not_bare(&repo.source)?;
            } else {
                worktree_start(&repo.source, &repo.branch, from_ref)?;
            }
        }
        let plan = CreateDryRunPlan {
            name,
            wt_dir: &wt_dir,
            repos_to_create: &kept,
            from_ref,
            from_pr_spec,
            ephemeral,
            ttl_seconds,
            custom_meta: &custom_meta,
        };
        print_create_dry_run(&plan)?;
        records.push(ProjectResult::about_workspace(CreatePlan {
            repos: kept
                .iter()
                .map(|repo| CreatePlanRepo {
                    alias: repo.project.clone(),
                    source: repo.source.clone(),
                    path: planned_dest(&wt_dir, repo),
                })
                .collect(),
        }));
        return Ok(Made::Preview(Vec::new()));
    }

    // Resolve/apply --from-pr: get PR head branch, override branch for the matching repo, and fetch
    let from_pr_info = from_pr_spec.map(resolve_from_pr).transpose()?;
    if let Some((ref pr_repo_spec, _pr_num, ref pr_branch)) = from_pr_info {
        let repo = repos_to_create
            .iter_mut()
            .find(|repo| repo.place != "." && repo_matches_spec(&repo.source, pr_repo_spec))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No selected repo has origin '{}', so PR branch '{}' can't be applied",
                    escape(pr_repo_spec),
                    escape(pr_branch)
                )
            })?;
        git_fetch_branch(&repo.source, pr_branch).map_err(|e| {
            e.context(format!(
                "Cannot fetch the PR branch into '{}'",
                escape(&repo.project)
            ))
        })?;
        repo.branch = pr_branch.clone();
    }

    // Before anything of the set exists, its lease: until this call is done,
    // whatever is in the set is this call's, even before it is recorded
    if let Some(parent) = wt_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let lease = {
        let set = match (wt_dir.parent(), wt_dir.file_name()) {
            (Some(parent), Some(name)) => parent
                .canonicalize()
                .with_context(|| format!("Cannot resolve {}", escape_path(parent)))?
                .join(name),
            _ => anyhow::bail!("Cannot resolve {}", escape_path(&wt_dir)),
        };
        lease::take(&lease::Lease::new(set, here.home.clone(), ephemeral))?
    };

    // Claim the set directory atomically: if another create claimed it first,
    // this fails and nothing of theirs is touched. Everything in it from here
    // on was made by this call.
    if let Err(e) = std::fs::create_dir(&wt_dir) {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            anyhow::bail!(
                "Worktree '{name}' already exists at {}",
                escape_path(&wt_dir)
            );
        }
        return Err(e).with_context(|| format!("Cannot create {}", escape_path(&wt_dir)));
    }

    // Anything that fails from here rolls back what this call created.
    let mut created_repos = Vec::new();
    let mut new_branches = Vec::new();
    // The checkout whose sets these are: inside a set, the one it stands for
    let owner = &here.home;
    let built = (|| -> Result<()> {
        // The set's owner, beside it: meta-git's, made from this workspace
        meta_plugin::sets::create(&wt_dir, meta_plugin::sets::DEFAULT_PROVIDER, owner)?;
        // Each worktree gets this record as soon as it exists, so `remove`
        // and `prune` know meta made it; a worktree that can't be recorded
        // is a failed create.
        let set_record = SetRecord {
            set: wt_dir
                .canonicalize()
                .with_context(|| format!("Cannot resolve {}", escape_path(&wt_dir)))?,
            // Inside a set, the checkout it stands for: a record never names
            // a set's checkout, which can be removed before this set
            project: owner.to_path_buf(),
            created_at: Utc::now().to_rfc3339(),
            ephemeral,
            ttl_seconds,
            custom: custom_meta.clone(),
        };
        let mark = |checkout: &std::path::Path| {
            meta_core::gitdir::linked(checkout)
                .and_then(|linked| record::write(&linked.admin, &set_record))
                .with_context(|| format!("Cannot record the worktree {}", escape_path(checkout)))
        };
        // Each repo where it is in the workspace, the root first (it becomes
        // the set's root: git accepts the empty directory claimed above),
        // then each repo before those inside it
        for repo in &repos_to_create {
            if let Some(r) = missing_ref(&repo.source, from_ref)? {
                // skip repos where the commit-ish doesn't exist
                ctx.warn_or_bail(format!(
                    "Skipping '{}': ref '{}' not found",
                    escape(&repo.project),
                    escape(r)
                ))?;
                continue;
            }
            let dest = if repo.place == "." {
                wt_dir.clone()
            } else {
                // A checkout already in the worktree (such as the root
                // repo's) may hold a symlink where this one goes; git must
                // not follow it out
                let dest = dest_in_set(&wt_dir, &repo.place).map_err(|why| {
                    anyhow::anyhow!(
                        "Refusing to add the worktree for '{}': {why}",
                        escape(&repo.project)
                    )
                })?;
                // Ensure parent directories exist for nested paths (e.g., "vendor/")
                if let Some(parent) = dest.parent().filter(|parent| *parent != wt_dir) {
                    std::fs::create_dir_all(parent)?;
                }
                dest
            };
            if verbose {
                errln!(
                    "Creating worktree for '{}' at {} (branch: {})",
                    escape(&repo.project),
                    escape_path(&dest),
                    escape(&repo.branch)
                );
            }
            // Any failure other than a missing ref is a failed create.
            let mut started_from = None;
            let created_branch =
                journal_new_branch(&repo.source, &repo.branch, &mut new_branches, || {
                    let start = git_worktree_add(&repo.source, &dest, &repo.branch, from_ref)?;
                    started_from = start.base(&repo.branch);
                    Ok(start.new_branch_at())
                })?;
            mark(&dest)?;
            // so remove can delete the branch again, once nothing is lost
            if created_branch {
                meta_core::gitdir::linked(&dest)
                    .and_then(|linked| record::write_created_branch(&linked.admin, &repo.branch))?;
            }
            created_repos.push(CreateRepoEntry {
                alias: repo.project.clone(),
                path: dest.display().to_string(),
                branch: repo.branch.clone(),
                created_branch,
                started_from,
            });
        }

        if created_repos.is_empty() {
            // Every repo was skipped because none has the ref.
            match from_ref {
                Some(r) => anyhow::bail!(
                    "No selected repo has ref '{}'; no worktree was created",
                    escape(r)
                ),
                None => anyhow::bail!("No repos selected; no worktree was created"),
            }
        }

        // Keep the sets out of the git status of the checkout whose sets
        // these are
        exclude_worktrees(owner, &worktree_root)
    })();
    built.map_err(|e| roll_back(&wt_dir, &new_branches, e))?;

    let output = CreateOutput {
        name: name.to_string(),
        root: wt_dir.display().to_string(),
        repos: created_repos,
        ephemeral,
        ttl_seconds,
        custom: custom_meta,
    };
    if !request.meta.json {
        outln!(
            "{} Created worktree '{}' at {}",
            "✓".green(),
            name.bold(),
            escape_path(&wt_dir)
        )?;
        for r in &output.repos {
            let branch_note = new_branch_note(r);
            outln!(
                "  {} -> {}{}",
                escape(&r.alias),
                escape(&r.branch),
                branch_note
            )?;
        }
        if ephemeral {
            outln!("  {}", "[ephemeral]".dimmed())?;
        }
        if let Some(ttl) = ttl_seconds {
            outln!(
                "  {}",
                format!(
                    "[TTL: {}]",
                    format_duration(i64::try_from(ttl).unwrap_or(i64::MAX))
                )
                .dimmed()
            )?;
        }
    }

    // What was made is reported first: a failing hook doesn't undo it
    let hook = fire_post_create(
        name,
        &wt_dir,
        &output.repos,
        ephemeral,
        ttl_seconds,
        &output.custom,
        Some(&here.workspace.worktree),
    )
    .or_else(|e| ctx.warn_or_bail(format!("{e:#}")));
    records.push(ProjectResult::about_workspace(&output).at(&wt_dir));
    if let Err(e) = hook {
        // the set is made: an ephemeral one is the next prune's
        if ephemeral {
            lease.leave();
        }
        return Err(e);
    }
    Ok(Made::Created(Created { dir: wt_dir, lease }))
}

/// How a repo's line ends: ` (new from origin/main)` for a branch this call
/// made, nothing for an existing one.
pub(super) fn new_branch_note(repo: &CreateRepoEntry) -> String {
    match (repo.created_branch, &repo.started_from) {
        (true, Some(base)) => format!(" (new from {})", escape(base)),
        (true, None) => " (new)".to_string(),
        (false, _) => String::new(),
    }
}

/// The commit-ish a repo doesn't have, if any: such repos are skipped. A
/// repo git can't read is an error, not a missing ref.
fn missing_ref<'a>(repo: &std::path::Path, from_ref: Option<&'a str>) -> Result<Option<&'a str>> {
    match from_ref {
        Some(r) if git_rev_parse(repo, &format!("{r}^{{commit}}"))?.is_none() => Ok(Some(r)),
        _ => Ok(None),
    }
}

/// Run a `git worktree add` and note the branch it created, with the commit
/// it was created at, so a rollback deletes only branches this call made and
/// only while they still point there (a post-checkout hook may have committed
/// on one). A branch that appears although the add failed may belong to
/// someone else: it is kept and named in the error.
fn journal_new_branch(
    repo: &std::path::Path,
    branch: &str,
    journal: &mut Vec<(std::path::PathBuf, String, String)>,
    add: impl FnOnce() -> Result<Option<String>>,
) -> Result<bool> {
    let branch_ref = format!("refs/heads/{branch}");
    let existed = git_rev_parse(repo, &branch_ref)?.is_some();
    match add() {
        Ok(Some(start)) => {
            journal.push((repo.to_path_buf(), branch.to_string(), start));
            Ok(true)
        }
        Ok(None) => Ok(false),
        Err(e) if !existed && matches!(git_rev_parse(repo, &branch_ref), Ok(Some(_))) => Err(e
            .context(format!(
                "branch '{branch}' now exists in {} and was kept: meta can't tell whether this \
             create made it",
                escape_path(repo)
            ))),
        Err(e) => Err(e),
    }
}

/// Undo a failed create: delete the set this call claimed, if the removal
/// check finds nothing to lose, and the branches it created, if they still
/// point where it left them. A record that can't be used or is missing (one
/// whose write failed, say) doesn't stop it: this call made the set, so it
/// needs no proof of that. Returns the error to report.
fn roll_back(
    wt_dir: &std::path::Path,
    new_branches: &[(std::path::PathBuf, String, String)],
    error: anyhow::Error,
) -> anyhow::Error {
    let undone = check_worktree_removal(wt_dir).and_then(|check| {
        let blockers = check
            .blockers
            .iter()
            .filter(|b| !check.unusable_records.contains(b) && !check.missing_records.contains(b));
        let problems: Vec<String> = blockers.chain(&check.unsaved).cloned().collect();
        anyhow::ensure!(problems.is_empty(), "{}", problems.join("; "));
        delete_checked_set(wt_dir, &check, false, false)
    });
    if let Err(undo) = undone {
        let branches: Vec<&str> = new_branches.iter().map(|(_, b, _)| b.as_str()).collect();
        return anyhow::anyhow!(
            "{error:#}\nThe partly created worktree at {} was left in place ({undo:#}); \
             delete it yourself and run 'git worktree prune' in its repositories. \
             Branches this create made: {}",
            escape_path(wt_dir),
            if branches.is_empty() {
                "none".to_string()
            } else {
                escape(&branches.join(", ")).into_owned()
            }
        );
    }
    let kept: Vec<String> = new_branches
        .iter()
        .filter_map(|(repo, branch, tip)| git_delete_branch_at(repo, branch, tip).err())
        .map(|e| format!("{e:#}"))
        .collect();
    if kept.is_empty() {
        error
    } else {
        anyhow::anyhow!(
            "{error:#}\nRolled back, but kept branches: {}",
            kept.join("; ")
        )
    }
}

struct CreateDryRunPlan<'a> {
    name: &'a str,
    wt_dir: &'a std::path::Path,
    repos_to_create: &'a [PlannedRepo],
    from_ref: Option<&'a str>,
    from_pr_spec: Option<&'a str>,
    ephemeral: bool,
    ttl_seconds: Option<u64>,
    custom_meta: &'a HashMap<String, String>,
}

/// Where a repo of the plan would go in the set at `wt_dir`.
fn planned_dest(wt_dir: &std::path::Path, repo: &PlannedRepo) -> std::path::PathBuf {
    match repo.place.as_str() {
        "." => wt_dir.to_path_buf(),
        place => wt_dir.join(place),
    }
}

fn print_create_dry_run(plan: &CreateDryRunPlan<'_>) -> Result<()> {
    outln!(
        "[DRY RUN] Would create worktree set '{}' at {}",
        plan.name,
        escape_path(plan.wt_dir)
    )?;
    outln!()?;

    if let Some(from_ref) = plan.from_ref {
        outln!("Start ref: {}", escape(from_ref))?;
    }
    if let Some(from_pr_spec) = plan.from_pr_spec {
        outln!(
            "PR source: would resolve {} and fetch the matching head branch",
            escape(from_pr_spec)
        )?;
    }
    if plan.ephemeral {
        outln!("Ephemeral: true")?;
    }
    if let Some(ttl) = plan.ttl_seconds {
        outln!(
            "TTL: {}",
            format_duration(i64::try_from(ttl).unwrap_or(i64::MAX))
        )?;
    }
    if !plan.custom_meta.is_empty() {
        outln!(
            "Metadata: {}",
            plan.custom_meta
                .iter()
                .map(|(k, v)| format!("{}={}", escape(k), escape(v)))
                .collect::<Vec<_>>()
                .join(", ")
        )?;
    }

    outln!("Planned repo operations:")?;
    for repo in plan.repos_to_create {
        let PlannedRepo {
            project: alias,
            source,
            place,
            branch,
        } = repo;
        let dest = planned_dest(plan.wt_dir, repo);
        let mut cmd = format!(
            "git -C {} worktree add -b {} {}",
            shell_quote(&source.display().to_string()),
            shell_quote(branch),
            shell_quote(&dest.display().to_string())
        );
        if let Some(from_ref) = plan.from_ref {
            cmd.push(' ');
            cmd.push_str(&shell_quote(from_ref));
        }
        outln!("  {}:", escape(alias))?;
        outln!("    source: {}", escape_path(source))?;
        if place != "."
            && source
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
        {
            outln!("    (a link: if the root checkout holds it, the repo goes where it leads)")?;
        }
        outln!("    dest:   {}", escape_path(&dest))?;
        outln!("    branch: {}", escape(branch))?;
        outln!("    bash:   {}", escape(&cmd))?;
    }

    outln!()?;
    outln!("No directories, branches, records, hooks, or git excludes were written.")?;
    Ok(())
}

/// `repos` as a preview left them out of its plan; quiet, as its text has
/// said so already.
fn must_clone_first(repos: &[PlannedRepo]) -> Vec<Skipped> {
    repos
        .iter()
        .map(|repo| Skipped {
            project: repo.place.clone(),
            reason: "not cloned: a create clones it first, so a preview has no plan".to_string(),
            quiet: true,
        })
        .collect()
}

/// Why a repo is in a set being made. A project that isn't cloned is an
/// error when it was asked for or is needed; with `--every-repo`, or as a parent
/// on the way to a nested repo, it is skipped with a warning (an error
/// with `--strict`).
#[derive(Debug, Clone, PartialEq)]
enum Why {
    /// Asked for, or needed by what was.
    Wanted,
    All,
    /// On the way to the nested repo named.
    Parent(String),
}

/// The repos of a set being made, one per place in the set. A place asked
/// for twice must be asked for on one branch; a repo that is only needed
/// (a dependency, a parent) takes the branch it was asked for on.
struct Plan<'a> {
    meta_dir: &'a std::path::Path,
    repos: Vec<(PlannedRepo, Why)>,
    /// The branch of each place a --repo asked for.
    asked: HashMap<String, String>,
}

impl Plan<'_> {
    fn add(&mut self, found: Found, branch: String, why: Why, asked: bool) -> Result<()> {
        let place = place_in_set(self.meta_dir, &found.key, &found.source);
        let repo = PlannedRepo {
            project: found.key,
            source: found.source,
            place: place.clone(),
            branch,
        };
        let known = self.repos.iter_mut().find(|(r, _)| r.place == place);
        match (known, self.asked.get(&place)) {
            (Some(_), Some(first)) if asked && *first != repo.branch => anyhow::bail!(
                "Repo '{}' is given twice, on branches '{}' and '{}'",
                escape(&place),
                escape(first),
                escape(&repo.branch)
            ),
            // what was asked for wins over what is only needed there
            (Some(known), None) if asked => *known = (repo.clone(), why),
            (Some(_), _) => {}
            (None, _) => self.repos.push((repo.clone(), why)),
        }
        if asked {
            self.asked.insert(place, repo.branch);
        }
        Ok(())
    }
}

/// The repos `args` ask for, each resolved once through `index`: placed as
/// in the workspace, on its branch, with the parents a nested one needs;
/// one per place, the root first, then each repo before those inside it.
fn plan_repos(index: &Index, args: &CreateArgs, ctx: &Ctx, verbose: bool) -> Result<Planned> {
    let meta_dir = index.dir.as_path();
    let branch = |asked: Option<&str>| resolve_branch(&args.name, args.branch.as_deref(), asked);
    let mut plan = Plan {
        meta_dir,
        repos: Vec::new(),
        asked: HashMap::new(),
    };

    if args.every_repo {
        // the root, when something is there for git (one git can't use is
        // skipped with the rest, saying why), and every project: by name,
        // or with --recursive, every place of the tree
        // (exec --ephemeral --every-repo may give a repo its branch: --repo name:branch)
        // (each such --repo is asked for, so two branches for one repo are refused)
        let mut all = |found: Found| {
            let specs: Vec<_> = args.repos.iter().filter(|r| r.alias == found.key).collect();
            if specs.is_empty() {
                return plan.add(found, branch(None), Why::All, false);
            }
            for spec in specs {
                plan.add(
                    found.clone(),
                    branch(spec.branch.as_deref()),
                    Why::All,
                    true,
                )?;
            }
            Ok(())
        };
        if !matches!(meta_core::gitdir::common_dir(meta_dir), Ok(None)) {
            all(index.find(".")?)?;
        }
        let keys: Vec<String> = index.nodes().into_iter().map(|n| index.key(n)).collect();
        for key in keys {
            let found = match index.is_recursive() {
                true => index.place(&key)?,
                false => index.find(&key)?,
            };
            all(found)?;
        }
    }
    // The projects whose depends_on this create follows: with --every-repo every
    // one, else those asked for and what they need. Only theirs are checked.
    let mut followed: Vec<String> = Vec::new();
    if args.every_repo {
        followed.extend(plan.repos.iter().map(|(repo, _)| repo.project.clone()));
    } else {
        let mut asked_keys = Vec::new();
        for spec in &args.repos {
            let found = index.find(&spec.alias)?;
            asked_keys.push(found.key.clone());
            plan.add(found, branch(spec.branch.as_deref()), Why::Wanted, true)?;
        }
        if !args.no_deps {
            // The root holds what the workspace's repos share (its Cargo
            // workspace, .claude/, build scripts)
            if meta_core::gitdir::common_dir(meta_dir)?.is_some() {
                plan.add(index.find(".")?, branch(None), Why::Wanted, false)?;
            }
            for key in asked_keys.iter().filter(|key| *key != ".") {
                followed.push(key.clone());
                let deps = index.dependencies(key);
                followed.extend(deps.iter().cloned());
                log::debug!("Resolved transitive dependencies for '{key}': {deps:?}");
                for dep in deps {
                    if verbose {
                        errln!(
                            "  Including '{}' (dependency of '{}')",
                            escape(&dep),
                            escape(key)
                        );
                    }
                    // as the index keys them
                    let found = match index.is_recursive() {
                        true => index.place(&dep)?,
                        false => index.find(&dep)?,
                    };
                    plan.add(found, branch(None), Why::Wanted, false)?;
                }
            }
        }
    }

    // A repo nested in the set needs a checkout of each project on the way
    // to it ("gitkb" for "gitkb/core"), not of its siblings: the thin spine
    // from the root. A parent is the project at that place, whatever name
    // selected the repo; a repo the workspace doesn't list is not added
    // (remove would refuse it as foreign).
    let nested: Vec<(String, String)> = plan
        .repos
        .iter()
        .map(|(r, _)| (r.place.clone(), r.project.clone()))
        .filter(|(place, _)| place.contains('/'))
        .collect();
    for (place, project) in nested {
        let parts: Vec<&str> = place.split('/').collect();
        for end in 1..parts.len() {
            let parent = parts
                .iter()
                .take(end)
                .copied()
                .collect::<Vec<_>>()
                .join("/");
            if plan.repos.iter().any(|(r, _)| r.place == parent) {
                continue;
            }
            let Some(source) = index.at_place(&parent) else {
                // A plain directory on the way is no parent; a repo that
                // isn't a project is worth saying so
                if meta_core::gitdir::common_dir(&meta_dir.join(&parent))?.is_some() {
                    ctx.warn(format!(
                        "Intermediate parent '{}' for nested repo '{}' is not a project of the \
                         workspace, skipping",
                        escape(&parent),
                        escape(&project)
                    ));
                }
                continue;
            };
            log::info!("Adding intermediate parent '{parent}' for nested repo '{project}'");
            let found = Found {
                key: parent,
                source,
            };
            plan.add(found, branch(None), Why::Parent(project.clone()), false)?;
        }
    }

    // What resolving the depends_on this create follows ran into; a parent
    // on the way to a nested repo is in the set for its place, not its needs

    // A project not cloned yet has no repository to make a worktree of: the
    // ones the set needs are returned, for the caller to clone; any other
    // folder that is no repository is named, each at once
    let mut missing: Vec<PlannedRepo> = Vec::new();
    let mut not_cloned: Vec<PlannedRepo> = Vec::new();
    let mut repos = Vec::with_capacity(plan.repos.len());
    for (repo, why) in plan.repos {
        let absent =
            meta_core::gitdir::Checkout::of(&repo.source) == meta_core::gitdir::Checkout::Missing;
        if absent && why != Why::All {
            missing.push(repo);
            continue;
        }
        let why = match why {
            Why::Parent(child) => {
                meta_core::gitdir::Checkout::of(&repo.source)
                    .problem()
                    .map(|problem| {
                        format!(
                            "Intermediate parent '{}' for nested repo '{}': {}",
                            escape(&repo.project),
                            escape(&child),
                            escape(&problem)
                        )
                    })
            }
            why => match super::not_a_repository(&repo.project, &repo.source) {
                Some(_) if why != Why::All => {
                    not_cloned.push(repo);
                    continue;
                }
                problem => problem,
            },
        };
        let Some(why) = why else {
            repos.push(repo);
            continue;
        };
        if ctx.meta().strict {
            anyhow::bail!("{why} (strict mode)");
        }
        ctx.warn(format!("{why}, skipping"));
    }
    // (once: a run that clones what the set needs plans again, from the
    // top, and says them then; with --strict, before it clones anything)
    let again = !missing.is_empty() && !ctx.meta().strict && !ctx.dry_run();
    if !args.no_deps && !again {
        report_problems(index, followed.iter().map(String::as_str), ctx)?;
    }
    if !not_cloned.is_empty() {
        not_cloned.extend(missing);
        anyhow::bail!("{}", not_cloned_error(&not_cloned, root_of(index)));
    }
    repos.sort_by(|a, b| {
        let order = |r: &PlannedRepo| (r.place != ".", r.place.split('/').count());
        order(a).cmp(&order(b)).then_with(|| a.place.cmp(&b.place))
    });
    Ok(Planned { repos, missing })
}

/// The meta repos that aren't cloned on the way to a repo `specs` ask for
/// that `index` doesn't know (`clients` for `clients/desktop`, whose place
/// only `clients/.meta.yaml` lists), each once, on the set's branch
/// (`name`).
pub(super) fn uncloned_meta_parents(
    index: &Index,
    wanted: &[String],
    name: &str,
) -> Vec<PlannedRepo> {
    let metas: Vec<(String, std::path::PathBuf)> = index
        .nodes()
        .into_iter()
        .filter(|n| n.project.meta)
        .map(|n| (n.place.clone(), n.project.abs.clone()))
        .collect();
    let mut parents: Vec<PlannedRepo> = Vec::new();
    for alias in wanted {
        if index.find(alias).is_ok() {
            continue;
        }
        for (place, source) in &metas {
            let on_the_way = alias.starts_with(&format!("{place}/"));
            if on_the_way
                && meta_core::gitdir::Checkout::of(source) == meta_core::gitdir::Checkout::Missing
                && !parents.iter().any(|r| r.place == *place)
            {
                parents.push(PlannedRepo {
                    project: place.clone(),
                    source: source.clone(),
                    place: place.clone(),
                    branch: resolve_branch(name, None, None),
                });
            }
        }
    }
    parents
}

/// The `depends_on` entries meta found no project for, of the repos
/// `specs` ask for and of every project they depend on: each as meta named
/// it (qualified with the place of the meta repo that lists it,
/// `clients/open-source/atc`, from the config's folder) and as written there
/// (`open-source/atc`), the places where an uncloned meta repo may hold it.
/// With --recursive only: without it, nested projects aren't dependencies.
fn unresolved_dependencies(index: &Index, specs: &[RepoSpec]) -> Vec<String> {
    if !index.is_recursive() {
        return Vec::new();
    }
    let mut unresolved: Vec<String> = Vec::new();
    for spec in specs {
        let Ok(found) = index.find(&spec.alias) else {
            continue;
        };
        let key = found.key;
        for project in std::iter::once(key.clone()).chain(index.dependencies(&key)) {
            // the place of the meta repo that lists it
            let prefix = index
                .node(&project)
                .and_then(|n| n.place.strip_suffix(n.own.as_str()))
                .filter(|prefix| prefix.is_empty() || prefix.ends_with('/'))
                .unwrap_or("")
                .to_string();
            for dep in index.unresolved(&project) {
                // named from the workspace root: from the config's folder
                let dep = dep.strip_prefix(&index.prefix).unwrap_or(&dep).to_string();
                if let Some(written) = dep.strip_prefix(&prefix).filter(|_| !prefix.is_empty()) {
                    unresolved.push(written.to_string());
                }
                unresolved.push(dep);
            }
        }
    }
    unresolved
}

/// The repos of a set being made: those to make worktrees of, and those it
/// needs that aren't cloned yet.
struct Planned {
    repos: Vec<PlannedRepo>,
    missing: Vec<PlannedRepo>,
}

/// Clone `missing`, the repos a set needs that aren't cloned (in the
/// workspace `index` reads), saying so; an error naming them, and why
/// cloning failed, when it does.
pub(super) fn clone_missing(index: &Index, missing: &[PlannedRepo]) -> Result<()> {
    let places: Vec<String> = missing
        .iter()
        .map(|r| escape(&r.place).into_owned())
        .collect();
    errln!(
        "Cloning {} the set needs: {}",
        match missing.len() {
            1 => "1 repo".to_string(),
            n => format!("{n} repos"),
        },
        places.join(", ")
    );
    let dirs: Vec<_> = missing.iter().map(|r| r.source.clone()).collect();
    let cloned = crate::clone_missing::clone_dirs(
        &index.listing(),
        index.members,
        &dirs,
        "git worktree create",
    )?;
    // the ones still not there, for the error: those cloned are fine now
    let still: Vec<PlannedRepo> = missing
        .iter()
        .filter(|r| {
            meta_core::gitdir::Checkout::of(&r.source)
                .problem()
                .is_some()
        })
        .cloned()
        .collect();
    match cloned {
        Ok(cloned) if !cloned.is_empty() => {
            errln!("{} {}", "Cloned".green(), places.join(", "));
            Ok(())
        }
        Ok(_) => anyhow::bail!(
            "{}\n(cloning cloned none of them)",
            not_cloned_error(&still, root_of(index))
        ),
        Err(why) => anyhow::bail!(
            "{}\nCloning failed: {}",
            not_cloned_error(&still, root_of(index)),
            why
        ),
    }
}

/// The error for the repos a set needs that can't be used, most often
/// because they aren't cloned, with the command that clones exactly the
/// missing ones (`recursive`: places are of the whole tree, so the command
/// needs -r).
fn not_cloned_error(repos: &[PlannedRepo], root: Option<std::path::PathBuf>) -> String {
    use meta_core::gitdir::Checkout;
    let mut message = format!(
        "{} the set needs can't be used; no worktree was created:",
        match repos.len() {
            1 => "1 repo".to_string(),
            n => format!("{n} repos"),
        }
    );
    let mut missing = Vec::new();
    let mut blocked = Vec::new();
    for repo in repos {
        let why = match Checkout::of(&repo.source) {
            Checkout::Missing => {
                // with -A, from anywhere, by its path from the root
                missing.push(
                    match root
                        .as_deref()
                        .and_then(|root| repo.source.strip_prefix(root).ok())
                    {
                        Some(path) => path.to_string_lossy().into_owned(),
                        None => repo.place.clone(),
                    },
                );
                "not cloned".to_string()
            }
            checkout => {
                blocked.push(escape_path(&repo.source));
                checkout.problem().unwrap_or_default()
            }
        };
        message.push_str(&format!(
            "\n  '{}' at {}: {}",
            escape(&repo.project),
            escape_path(&repo.source),
            escape(&why)
        ));
    }
    // a folder that holds something else is the user's: clone-missing
    // refuses it too, so the way on is to move it aside, and create clones
    // the project there
    if !blocked.is_empty() {
        message.push_str(&format!(
            "\nMove {} aside (or delete {}, if nothing in {} is needed), then run this \
             create again: it clones what isn't there.",
            blocked.join(", "),
            if blocked.len() == 1 { "it" } else { "them" },
            if blocked.len() == 1 { "it" } else { "them" }
        ));
    } else if !missing.is_empty() {
        let include = meta_plugin::display::shell_quote(&missing.join(","));
        message.push_str(&format!(
            "\nTo clone just the missing ones, then run this create again:\n  meta {}--include {} git clone-missing",
            if root.is_some() { "-A " } else { "" },
            escape(&include)
        ));
    }
    message
}

/// Where a recursive index names its places from, for the clone-missing
/// line (`-A`): none for a shallow one.
fn root_of(index: &Index) -> Option<std::path::PathBuf> {
    index.is_recursive().then(|| index.listed_root.clone())
}

/// Report what meta ran into resolving the `depends_on` of each project in
/// `set`: a warning, or with `--strict` an error.
fn report_problems<'a>(
    index: &Index,
    set: impl IntoIterator<Item = &'a str>,
    ctx: &Ctx,
) -> Result<()> {
    // a project reached twice is reported once
    let set: BTreeSet<&str> = set.into_iter().collect();
    for project in set {
        let Some(node) = index.node(project) else {
            continue;
        };
        for problem in &node.project.dependency_problems {
            ctx.warn_or_bail(format!("'{}' {problem}", escape(project)))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roll_back_deletes_a_set_whose_records_were_not_all_written() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.work();
        let set = tmp.join("set");
        for (repo, wt) in [("root", set.clone()), ("leaf", set.join("leaf"))] {
            let source = tmp.join(repo);
            std::fs::create_dir(&source).unwrap();
            sandbox.git(&source, &["init", "-q"]);
            sandbox.git(&source, &["commit", "-q", "--allow-empty", "-m", "init"]);
            sandbox.git(
                &source,
                &["worktree", "add", "-q", "-b", repo, wt.to_str().unwrap()],
            );
        }
        // The root was recorded; the leaf's record write failed
        let set_record = SetRecord {
            set: set.canonicalize().unwrap(),
            project: tmp.to_path_buf(),
            created_at: Utc::now().to_rfc3339(),
            ephemeral: false,
            ttl_seconds: None,
            custom: HashMap::new(),
        };
        record::write(&meta_core::gitdir::linked(&set).unwrap().admin, &set_record).unwrap();
        assert_eq!(
            check_worktree_removal(&set).unwrap().missing_records.len(),
            1
        );

        let err = roll_back(&set, &[], anyhow::anyhow!("record write failed"));
        assert_eq!(format!("{err:#}"), "record write failed");
        assert!(!set.exists());
    }

    #[test]
    fn journal_keeps_only_branches_this_call_created() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.work();
        sandbox.git(&tmp, &["init", "-q"]);
        sandbox.git(&tmp, &["commit", "-q", "--allow-empty", "-m", "init"]);
        let mut journal = Vec::new();

        // Our add created the branch: journaled.
        let created = journal_new_branch(&tmp, "ours", &mut journal, || {
            sandbox.git(&tmp, &["branch", "ours"]);
            Ok(Some("start".to_string()))
        });
        assert!(created.unwrap());
        assert_eq!(journal.len(), 1);

        // Another process created the branch while our add failed: kept and named.
        let err = journal_new_branch(&tmp, "theirs", &mut journal, || {
            sandbox.git(&tmp, &["branch", "theirs"]);
            anyhow::bail!("branch already exists")
        })
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("'theirs' now exists"),
            "{err:#}"
        );
        assert_eq!(journal.len(), 1);
    }
}
