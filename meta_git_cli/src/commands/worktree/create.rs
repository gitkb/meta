use anyhow::{Context, Result};
use chrono::Utc;
use colored::*;
use meta_core::display::{escape, escape_path};
use meta_core::ExecutionMode;
use std::collections::{HashMap, HashSet};

use meta_cli::dependency_graph::{DependencyGraph, ProjectDependencies};
use meta_core::config::Workspace;
use meta_core::duration::format_duration;
use meta_git_lib::worktree::git_ops::*;
use meta_git_lib::worktree::helpers::*;
use meta_git_lib::worktree::hooks::fire_post_create;
use meta_git_lib::worktree::record::{self, SetRecord};
use meta_git_lib::worktree::safety::{check_worktree_removal, delete_checked_set};
use meta_git_lib::worktree::types::{CreateOutput, CreateRepoEntry};
use meta_git_lib::worktree::{lease, workspace};

use super::cli_types::CreateArgs;
use meta_core::display::shell_quote;

pub(crate) fn handle_create(
    args: CreateArgs,
    mode: ExecutionMode,
    verbose: bool,
    json: bool,
    global_strict: bool,
) -> Result<Option<lease::Held>> {
    // Merge global --strict with local --strict (either enables strict mode)
    let strict = args.strict || global_strict;

    let name = &args.name;
    validate_worktree_name(name)?;

    let branch_flag = args.branch.as_deref();
    let repo_specs = &args.repos;
    let use_all = args.all;
    let ephemeral = args.ephemeral;
    let ttl_seconds = args.ttl;
    // Parse custom metadata, collecting any invalid entries for strict mode
    let mut custom_meta = HashMap::new();
    for s in &args.custom_meta {
        if let Some((key, value)) = s.split_once('=') {
            custom_meta.insert(key.to_string(), value.to_string());
        } else {
            super::warn_or_bail(
                strict,
                format!(
                    "--meta value '{}' missing '=' separator (expected key=value), skipping",
                    escape(s)
                ),
            )?;
        }
    }
    let from_ref = args.commit_ish.as_deref();
    let from_pr_spec = args.from_pr.as_deref();

    let no_deps = args.no_deps;
    let recursive = args.recursive;

    if repo_specs.is_empty() && !use_all {
        anyhow::bail!("Specify repos with --repo <alias> or use --all");
    }
    // A repeated --repo is one worktree; two branches for it cannot both be
    // checked out there, and keeping one would drop the other unnoticed.
    // (Two aliases of one repo are caught again once resolved.)
    for (i, spec) in repo_specs.iter().enumerate() {
        let other = repo_specs
            .iter()
            .skip(i + 1)
            .find(|o| o.alias == spec.alias && o.branch != spec.branch);
        if let Some(other) = other {
            let branch = |s: &meta_git_lib::worktree::RepoSpec| {
                escape(s.branch.as_deref().unwrap_or("(default)")).into_owned()
            };
            anyhow::bail!(
                "Repo '{}' is given twice, on branches '{}' and '{}'",
                escape(&spec.alias),
                branch(spec),
                branch(other)
            );
        }
    }

    let cwd = std::env::current_dir()?;
    let configs = Configs::find(&cwd)?;
    // --recursive works from the root of the workspace tree
    let configs = match recursive {
        true => Configs::of(configs.here.root()?)?,
        false => configs,
    };
    let meta_dir = configs.here.dir.clone();
    let worktree_root = resolve_worktree_root(Some(&configs.owner), &cwd)?;

    // Check if worktree already exists
    let wt_dir = worktree_root.join(name);
    if wt_dir.exists() {
        let lease = wt_dir
            .canonicalize()
            .ok()
            .and_then(|set| lease::find(&workspace::Workspace::of(&configs.here).repos, &set));
        anyhow::bail!(
            "Worktree '{name}' already exists at {}{}. Use 'meta worktree remove {name}' first.",
            escape_path(&wt_dir),
            match lease {
                Some(found) if found.live => format!(" ({})", found.doing()),
                Some(found) => format!(" (left half-made by {}, which is gone)", found.holder()),
                None => String::new(),
            }
        );
    }

    // Parse .meta.yaml to get project list
    // When --all is specified, include root repo "." if it's a git repository
    let projects = projects_with_root(&configs.here, use_all);

    // Determine which repos to include: Vec<(alias, source_path, branch)>
    let repos_to_create: Vec<(String, std::path::PathBuf, String)> = if use_all {
        let mut all: Vec<(String, std::path::PathBuf)> = projects
            .iter()
            .filter(|p| !recursive || p.name == ".")
            .map(|p| (p.name.clone(), meta_dir.join(&p.path)))
            .collect();
        // --recursive: every repo of the nested tree, by its full path
        if recursive {
            let tree = meta_core::config::walk_projects(&meta_dir, &configs.here.projects, None)?;
            all.extend(
                meta_core::config::build_project_map(&tree, &meta_dir, "")
                    .into_iter()
                    .map(|(alias, (source, _))| (alias, source)),
            );
        }
        all.into_iter()
            .map(|(alias, source)| {
                let per_branch = repo_specs
                    .iter()
                    .find(|r| r.alias == alias)
                    .and_then(|r| r.branch.as_deref());
                let branch = resolve_branch(name, branch_flag, per_branch);
                (alias, source, branch)
            })
            .collect()
    } else if no_deps {
        // --no-deps: only include explicitly specified repos (legacy behavior)
        let mut list = Vec::new();
        for spec in repo_specs {
            if spec.alias == "." {
                list.push((
                    ".".to_string(),
                    meta_dir.clone(),
                    resolve_branch(name, branch_flag, spec.branch.as_deref()),
                ));
            } else {
                let (source, _project) = lookup_nested_project(&configs.here, &spec.alias)?;
                list.push((
                    spec.alias.clone(),
                    source,
                    resolve_branch(name, branch_flag, spec.branch.as_deref()),
                ));
            }
        }
        list
    } else {
        // Default: auto-include root repo + resolve dependencies
        resolve_repos_with_dependencies(
            &configs.here,
            &projects,
            repo_specs,
            name,
            branch_flag,
            verbose,
            recursive,
        )?
    };

    // For nested aliases like "gitkb/core", ensure intermediate meta: true
    // parents ("gitkb") get worktrees too — the thin spine from root to target.
    let repos_to_create =
        ensure_intermediate_parents(&configs.here, repos_to_create, name, branch_flag, strict)?;
    // A project not cloned yet has no repository to make a worktree of:
    // --all goes on without it, a repo asked for (or needed) is an error
    let mut cloned = Vec::with_capacity(repos_to_create.len());
    for repo in repos_to_create {
        let (alias, source, _) = &repo;
        let Some(why) = super::not_a_repository(alias, source) else {
            cloned.push(repo);
            continue;
        };
        if !use_all {
            anyhow::bail!("{why}");
        }
        if strict {
            anyhow::bail!("{why} (strict mode)");
        }
        super::warn(format!("{why}, skipping"));
    }
    let mut repos_to_create = cloned;
    // One worktree per place in the set: a repeated --repo, or a repo named
    // both by its name and by its path, would try the same add twice
    let mut branches: HashMap<String, &str> = HashMap::new();
    for (alias, source, branch) in &repos_to_create {
        let place = place_in_set(&meta_dir, alias, source);
        match branches.get(place.as_str()) {
            Some(first) if *first != branch.as_str() => anyhow::bail!(
                "Repo '{}' is given twice, on branches '{}' and '{}'",
                escape(&place),
                escape(first),
                escape(branch)
            ),
            Some(_) => {}
            None => {
                branches.insert(place, branch);
            }
        }
    }
    let mut seen = HashSet::new();
    repos_to_create
        .retain(|(alias, source, _)| seen.insert(place_in_set(&meta_dir, alias, source)));
    // A repo goes in before any repo inside it, whatever order they were named in
    repos_to_create.sort_by_key(|(alias, source, _)| {
        let place = place_in_set(&meta_dir, alias, source);
        (alias != ".", place.split('/').count())
    });

    // A preview makes the checks a run makes (all but asking GitHub for a
    // PR's branch), skips what it skips and fails where it fails
    if mode.is_preview() {
        if let Some(spec) = from_pr_spec {
            let (pr_repo, pr_num) = parse_from_pr(spec)?;
            let Some((_, _, branch)) = repos_to_create
                .iter_mut()
                .find(|(alias, source, _)| alias != "." && repo_matches_spec(source, &pr_repo))
            else {
                anyhow::bail!(
                    "No selected repo has origin '{}', so PR #{pr_num} can't be applied",
                    escape(&pr_repo)
                );
            };
            *branch = format!("<the head branch of {spec}>");
        }
        let mut kept = Vec::new();
        for repo in repos_to_create {
            match missing_ref(&repo.1, from_ref)? {
                Some(r) => super::warn_or_bail(
                    strict,
                    format!(
                        "Skipping '{}': ref '{}' not found",
                        escape(&repo.0),
                        escape(r)
                    ),
                )?,
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
        for (_, source, branch) in &kept {
            if pr_branch.as_ref() == Some(branch) {
                ensure_checkout_config_is_not_bare(source)?;
            } else {
                worktree_start(source, branch, from_ref)?;
            }
        }
        let plan = CreateDryRunPlan {
            name,
            meta_dir: &meta_dir,
            wt_dir: &wt_dir,
            repos_to_create: &kept,
            from_ref,
            from_pr_spec,
            ephemeral,
            ttl_seconds,
            custom_meta: &custom_meta,
        };
        print_create_dry_run(&plan);
        return Ok(None);
    }

    // Resolve/apply --from-pr: get PR head branch, override branch for the matching repo, and fetch
    let from_pr_info = from_pr_spec.map(resolve_from_pr).transpose()?;
    if let Some((ref pr_repo_spec, _pr_num, ref pr_branch)) = from_pr_info {
        let (alias, source, branch) = repos_to_create
            .iter_mut()
            .find(|(alias, source, _)| alias != "." && repo_matches_spec(source, pr_repo_spec))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No selected repo has origin '{}', so PR branch '{}' can't be applied",
                    escape(pr_repo_spec),
                    escape(pr_branch)
                )
            })?;
        git_fetch_branch(source, pr_branch).map_err(|e| {
            e.context(format!(
                "Cannot fetch the PR branch into '{}'",
                escape(alias)
            ))
        })?;
        *branch = pr_branch.clone();
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
        let Some((_, first, _)) = repos_to_create.first() else {
            anyhow::bail!("No repos selected; no worktree was created");
        };
        let git_dir = meta_core::gitdir::common_dir(first)?
            .ok_or_else(|| anyhow::anyhow!("{} is not a git repository", escape_path(first)))?;
        // A lease for this path left in another repository (by a create of
        // other repos, whose set was then deleted by hand) would name the
        // set this call makes: retire it first
        if let Some(found) = lease::find(&workspace::Workspace::of(&configs.here).repos, &set) {
            if found.live {
                anyhow::bail!("Worktree set {} is {}", escape_path(&set), found.doing());
            }
            found.delete()?;
        }
        lease::take(
            &git_dir,
            &lease::Lease {
                set,
                project: configs.owner.dir.clone(),
                pid: std::process::id(),
                ephemeral,
            },
        )?
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
    let owner = &configs.owner.dir;
    let built = (|| -> Result<()> {
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
        // If "." is included, create it first (it becomes the worktree root;
        // git accepts the empty directory claimed above).
        if let Some((_, source, branch)) = repos_to_create.iter().find(|(a, _, _)| a == ".") {
            if let Some(r) = missing_ref(source, from_ref)? {
                // skip the root repo if the commit-ish doesn't exist there (as for child repos)
                super::warn_or_bail(
                    strict,
                    format!("Skipping '.': ref '{}' not found", escape(r)),
                )?;
            } else {
                if verbose {
                    eprintln!(
                        "Creating meta repo worktree at {} (branch: {})",
                        escape_path(&wt_dir),
                        escape(branch)
                    );
                }
                let created_branch = journal_new_branch(source, branch, &mut new_branches, || {
                    git_worktree_add(source, &wt_dir, branch, from_ref)
                })?;
                mark(&wt_dir)?;
                created_repos.push(CreateRepoEntry {
                    alias: ".".to_string(),
                    path: wt_dir.display().to_string(),
                    branch: branch.clone(),
                    created_branch,
                });
            }
        }

        // Create child repo worktrees
        for (alias, source, branch) in &repos_to_create {
            if alias == "." {
                continue;
            }
            if let Some(r) = missing_ref(source, from_ref)? {
                // skip repos where the commit-ish doesn't exist
                super::warn_or_bail(
                    strict,
                    format!(
                        "Skipping '{}': ref '{}' not found",
                        escape(alias),
                        escape(r)
                    ),
                )?;
                continue;
            }

            // The repo goes where it is in the workspace
            let place = place_in_set(&meta_dir, alias, source);
            // A checkout already in the worktree (such as the root repo's) may
            // hold a symlink where this one goes; git must not follow it out
            let dest = dest_in_set(&wt_dir, &place).map_err(|why| {
                anyhow::anyhow!(
                    "Refusing to add the worktree for '{}': {why}",
                    escape(alias)
                )
            })?;

            // Ensure parent directories exist for nested paths (e.g., "vendor/")
            if let Some(parent) = dest.parent() {
                if parent != wt_dir {
                    std::fs::create_dir_all(parent)?;
                }
            }

            if verbose {
                eprintln!(
                    "Creating worktree for '{}' at {} (branch: {})",
                    escape(alias),
                    escape_path(&dest),
                    escape(branch)
                );
            }

            // Any failure other than a missing ref is a failed create.
            let created_branch = journal_new_branch(source, branch, &mut new_branches, || {
                git_worktree_add(source, &dest, branch, from_ref)
            })?;
            mark(&dest)?;
            created_repos.push(CreateRepoEntry {
                alias: alias.clone(),
                path: dest.display().to_string(),
                branch: branch.clone(),
                created_branch,
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
        // these are, when they are in it (`worktrees_dir` may be elsewhere)
        // as written (a `.worktrees` link to another disk is still in it), or
        // as the two resolve (an absolute worktrees_dir spelled another way)
        let plain = |rel: &std::path::Path| {
            rel.components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
        };
        let real = |path: &std::path::Path| path.canonicalize().ok();
        let inside = worktree_root
            .strip_prefix(owner)
            .ok()
            .filter(|rel| plain(rel))
            .map(std::path::Path::to_path_buf)
            .or_else(|| {
                let (root, owner) = (real(&worktree_root)?, real(owner)?);
                Some(root.strip_prefix(owner).ok()?.to_path_buf()).filter(|rel| plain(rel))
            });
        match inside.as_deref().and_then(|rel| rel.to_str()) {
            Some(rel) if !rel.is_empty() => {
                let link = worktree_root
                    .symlink_metadata()
                    .is_ok_and(|meta| meta.is_symlink());
                ensure_worktrees_in_gitignore(owner, &rel.replace('\\', "/"), link, json)
            }
            _ => Ok(()),
        }
    })();
    built.map_err(|e| roll_back(&wt_dir, &new_branches, e))?;

    // Fire post-create hook
    fire_post_create(
        name,
        &wt_dir,
        &created_repos,
        ephemeral,
        ttl_seconds,
        &custom_meta,
        Some(&configs.owner),
    );

    // Output
    if json {
        let output = CreateOutput {
            name: name.to_string(),
            root: wt_dir.display().to_string(),
            repos: created_repos,
            ephemeral,
            ttl_seconds,
            custom: custom_meta,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!(
            "{} Created worktree '{}' at {}",
            "✓".green(),
            name.bold(),
            escape_path(&wt_dir)
        );
        for r in &created_repos {
            let branch_note = if r.created_branch { " (new)" } else { "" };
            println!(
                "  {} -> {}{}",
                escape(&r.alias),
                escape(&r.branch),
                branch_note
            );
        }
        if ephemeral {
            println!("  {}", "[ephemeral]".dimmed());
        }
        if let Some(ttl) = ttl_seconds {
            println!(
                "  {}",
                format!("[TTL: {}]", format_duration(ttl as i64)).dimmed()
            );
        }
    }

    Ok(Some(lease))
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
    meta_dir: &'a std::path::Path,
    wt_dir: &'a std::path::Path,
    repos_to_create: &'a [(String, std::path::PathBuf, String)],
    from_ref: Option<&'a str>,
    from_pr_spec: Option<&'a str>,
    ephemeral: bool,
    ttl_seconds: Option<u64>,
    custom_meta: &'a HashMap<String, String>,
}

fn print_create_dry_run(plan: &CreateDryRunPlan<'_>) {
    println!(
        "[DRY RUN] Would create worktree set '{}' at {}",
        plan.name,
        escape_path(plan.wt_dir)
    );
    println!();

    if let Some(from_ref) = plan.from_ref {
        println!("Start ref: {}", escape(from_ref));
    }
    if let Some(from_pr_spec) = plan.from_pr_spec {
        println!(
            "PR source: would resolve {} and fetch the matching head branch",
            escape(from_pr_spec)
        );
    }
    if plan.ephemeral {
        println!("Ephemeral: true");
    }
    if let Some(ttl) = plan.ttl_seconds {
        println!("TTL: {}", format_duration(ttl as i64));
    }
    if !plan.custom_meta.is_empty() {
        println!(
            "Metadata: {}",
            plan.custom_meta
                .iter()
                .map(|(k, v)| format!("{}={}", escape(k), escape(v)))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    println!("Planned repo operations:");
    for (alias, source, branch) in plan.repos_to_create {
        let dest = if alias == "." {
            plan.wt_dir.to_path_buf()
        } else {
            plan.wt_dir.join(place_in_set(plan.meta_dir, alias, source))
        };
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
        println!("  {}:", escape(alias));
        println!("    source: {}", escape_path(source));
        if alias != "."
            && source
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
        {
            println!("    (a link: if the root checkout holds it, the repo goes where it leads)");
        }
        println!("    dest:   {}", escape_path(&dest));
        println!("    branch: {}", escape(branch));
        println!("    bash:   {}", escape(&cmd));
    }

    println!();
    println!("No directories, branches, records, hooks, or .gitignore changes were written.");
}

/// Resolve repos with automatic dependency resolution.
///
/// When --repo is specified without --no-deps:
/// 1. Always includes root repo "." (contains workspace Cargo.toml)
/// 2. Resolves transitive dependencies via provides/depends_on from .meta.yaml
///
/// When `recursive` is true, walks the full nested meta tree from `meta_dir`
/// (which should already be the root), building a dependency graph where all
/// project names and deps use full paths (e.g., "open-source/gitkb/core").
fn resolve_repos_with_dependencies(
    workspace: &Workspace,
    projects: &[meta_core::config::ProjectInfo],
    repo_specs: &[meta_git_lib::worktree::RepoSpec],
    worktree_name: &str,
    branch_flag: Option<&str>,
    verbose: bool,
    recursive: bool,
) -> Result<Vec<(String, std::path::PathBuf, String)>> {
    let meta_dir = workspace.dir.as_path();
    // Build dependency graph — either flat (current level) or nested (full tree)
    let graph = if recursive {
        build_nested_dep_graph(workspace)?
    } else {
        let project_deps: Vec<_> = projects.iter().map(|p| p.clone().into()).collect();
        DependencyGraph::build(project_deps)?
    };

    // Collect all repos to include (using HashSet for deduplication)
    let mut repos_to_include: HashSet<String> = HashSet::new();

    // Always include root repo "." unless user explicitly excluded it
    // (exclusion would need to be handled separately, for now always include)
    if meta_core::gitdir::common_dir(meta_dir)?.is_some() {
        repos_to_include.insert(".".to_string());
    }

    // For each explicitly requested repo, add it and its transitive dependencies
    for spec in repo_specs {
        if spec.alias == "." {
            repos_to_include.insert(".".to_string());
            continue;
        }

        // When recursive, graph keys are full paths (e.g., "open-source/gitkb/core").
        // Resolve spec.alias against the graph to handle both full-path and short aliases.
        let resolved_alias = if recursive {
            resolve_alias_in_graph(&graph, &spec.alias)?
        } else {
            spec.alias.clone()
        };

        // Add the explicitly requested repo
        repos_to_include.insert(resolved_alias.clone());

        // Get transitive dependencies
        let deps = graph.get_all_dependencies(&resolved_alias);
        log::debug!(
            "Resolved transitive dependencies for '{}': {:?}",
            resolved_alias,
            deps
        );
        for dep in deps {
            repos_to_include.insert(dep.to_string());
            if verbose {
                eprintln!(
                    "  Including '{}' (dependency of '{}')",
                    escape(dep),
                    escape(&resolved_alias)
                );
            }
        }
    }

    // Build the final list with paths and branches
    let mut list = Vec::new();

    // Add root repo first if included
    if repos_to_include.contains(".") {
        let per_branch = repo_specs
            .iter()
            .find(|r| r.alias == ".")
            .and_then(|r| r.branch.as_deref());
        list.push((
            ".".to_string(),
            meta_dir.to_path_buf(),
            resolve_branch(worktree_name, branch_flag, per_branch),
        ));
    }

    // Precompute a mapping from resolved alias to per-repo branch so we avoid
    // repeated graph scans inside the per-repo loop below.
    // Two specs naming one repo (say `core` and `group/core`) on different
    // branches can't both be honoured: refuse rather than keep one.
    let mut resolved_branch_map: HashMap<String, Option<&str>> = HashMap::new();
    for spec in repo_specs {
        let Ok(resolved) = resolve_alias_in_graph(&graph, &spec.alias) else {
            continue;
        };
        let branch = spec.branch.as_deref();
        if let Some(first) = resolved_branch_map.insert(resolved.clone(), branch) {
            if first != branch {
                anyhow::bail!(
                    "Repo '{}' is given twice, on branches '{}' and '{}'",
                    escape(&resolved),
                    escape(first.unwrap_or("(default)")),
                    escape(branch.unwrap_or("(default)"))
                );
            }
        }
    }

    // Add other repos
    for alias in &repos_to_include {
        if alias == "." {
            continue;
        }

        let (source, _project) = lookup_nested_project(workspace, alias)?;
        let per_branch = resolved_branch_map.get(alias.as_str()).copied().flatten();
        list.push((
            alias.clone(),
            source,
            resolve_branch(worktree_name, branch_flag, per_branch),
        ));
    }

    Ok(list)
}

/// Resolve a short alias (e.g., "core") to its full graph key (e.g., "open-source/gitkb/core").
///
/// If the alias directly matches a graph key, returns it as-is.
/// Otherwise, looks for a unique graph key that ends with "/<alias>".
/// Errors if no match or multiple matches are found.
fn resolve_alias_in_graph(graph: &DependencyGraph, alias: &str) -> Result<String> {
    // Direct match — already a full path
    if graph.get_project(alias).is_some() {
        return Ok(alias.to_string());
    }

    // Try suffix match: find graph keys ending with "/<alias>"
    let suffix = format!("/{}", alias);
    let matches: Vec<&str> = graph
        .all_projects()
        .iter()
        .filter(|p| p.name.ends_with(&suffix) || p.name == alias)
        .map(|p| p.name.as_str())
        .collect();

    match matches.len() {
        1 => Ok(matches[0].to_string()),
        0 => anyhow::bail!(
            "Repo '{}' not found in the recursive project graph. \
             With --recursive, use the full path from root (e.g., 'open-source/gitkb/core').",
            escape(alias)
        ),
        _ => anyhow::bail!(
            "Repo alias '{}' is ambiguous — matches: {}. \
             Use the full path to disambiguate.",
            escape(alias),
            escape(&matches.join(", "))
        ),
    }
}

/// Build a dependency graph from the full nested meta tree.
///
/// Walks `meta_dir`'s meta tree recursively, builds a project map with
/// fully-qualified paths (e.g., "open-source/gitkb/core"), then prefixes
/// each project's `depends_on` and `provides` with its parent path so
/// cross-workspace dependencies resolve correctly.
///
/// Example: project "core" at path "open-source/gitkb/core" with
/// `depends_on: ["vendor/tree-sitter-markdown"]` becomes
/// `depends_on: ["open-source/gitkb/vendor/tree-sitter-markdown"]`.
fn build_nested_dep_graph(workspace: &Workspace) -> Result<DependencyGraph> {
    let tree = meta_core::config::walk_projects(&workspace.dir, &workspace.projects, None)?;
    let project_map = meta_core::config::build_project_map(&tree, &workspace.dir, "");

    let mut project_deps = Vec::new();
    for (full_path, (_fs_path, info)) in &project_map {
        // Compute the parent prefix: strip the local path from the full path.
        // e.g., full_path="open-source/gitkb/core", info.path="core"
        //        → parent_prefix="open-source/gitkb/"
        let parent_prefix = full_path
            .strip_suffix(info.path.as_str())
            .filter(|prefix| prefix.is_empty() || prefix.ends_with('/'))
            .unwrap_or("");

        let qualify = |name: &str| {
            if project_map.contains_key(name) || name == "." {
                name.to_string()
            } else {
                format!("{}{}", parent_prefix, name)
            }
        };

        let prefixed_deps: Vec<String> = info.depends_on.iter().map(|dep| qualify(dep)).collect();
        let prefixed_provides: Vec<String> = info.provides.iter().map(|p| qualify(p)).collect();

        project_deps.push(ProjectDependencies {
            name: full_path.clone(),
            path: full_path.clone(),
            repo: info.repo.clone(),
            tags: info.tags.clone(),
            provides: prefixed_provides,
            depends_on: prefixed_deps,
        });
    }

    DependencyGraph::build(project_deps)
}

/// The source of the project at `place` in `workspace`, reading only the
/// configs of the meta repos on the way to it.
fn project_at(workspace: &Workspace, place: &str) -> Result<Option<std::path::PathBuf>> {
    for project in &workspace.projects {
        let path = workspace.dir.join(&project.path);
        if project.path == place {
            return Ok(Some(path));
        }
        if let Some(rest) = place.strip_prefix(&format!("{}/", project.path)) {
            if let Some(nested) = Workspace::in_dir(&path)? {
                if let Some(found) = project_at(&nested, rest)? {
                    return Ok(Some(found));
                }
            }
        }
    }
    Ok(None)
}

/// Ensure intermediate `meta: true` parent repos get worktrees for nested aliases.
///
/// When `repos_to_create` contains a nested alias like `"gitkb/core"`, the
/// intermediate parent `"gitkb"` needs a worktree too — `git worktree add`
/// for `gitkb/core` requires `gitkb/` to exist as a checkout, not an empty dir.
///
/// This creates the "thin spine" from root to each target: only the ancestors
/// that are needed, not siblings. Asking for `gitkb/core` adds `gitkb` but
/// NOT `gitkb/ui`, `gitkb/docs`, etc.
fn ensure_intermediate_parents(
    workspace: &Workspace,
    repos: Vec<(String, std::path::PathBuf, String)>,
    worktree_name: &str,
    branch_flag: Option<&str>,
    strict: bool,
) -> Result<Vec<(String, std::path::PathBuf, String)>> {
    let meta_dir = workspace.dir.as_path();
    // Where the selected repos go: a parent is there when a repo is at its
    // path, whatever name selected it
    let existing: HashSet<String> = repos
        .iter()
        .map(|(a, s, _)| place_in_set(meta_dir, a, s))
        .collect();

    let mut to_add: Vec<(String, std::path::PathBuf, String)> = Vec::new();
    let mut added: HashSet<String> = HashSet::new();

    for (alias, source, _branch) in &repos {
        // Only a repo nested in the set needs its parents: they follow from
        // where it goes, not from its name
        let place = place_in_set(meta_dir, alias, source);
        if !place.contains('/') {
            continue;
        }

        // Walk up through path segments: "gitkb/core" → check "gitkb"
        // "a/b/c" → check "a", "a/b"
        let parts: Vec<&str> = place.split('/').collect();
        for i in 1..parts.len() {
            let parent_alias = parts[..i].join("/");

            // Skip if already in the list or already added
            if existing.contains(&parent_alias) || !added.insert(parent_alias.clone()) {
                continue;
            }

            // A parent is the project at that path, not one by that name; a
            // repo the workspace doesn't list is not added (remove would
            // refuse it as foreign)
            let Some(parent_source) = project_at(workspace, &parent_alias)? else {
                // A plain directory on the way is no parent; a repo that
                // isn't a project is worth saying so
                if meta_core::gitdir::common_dir(&meta_dir.join(&parent_alias))?.is_none() {
                    continue;
                }
                eprintln!(
                    "{} Intermediate parent '{}' for nested repo '{}' is not a project of the workspace, skipping",
                    "warning:".yellow().bold(),
                    escape(&parent_alias),
                    escape(alias)
                );
                continue;
            };
            if let Some(problem) = meta_core::gitdir::Checkout::of(&parent_source).problem() {
                let why = format!(
                    "Intermediate parent '{}' for nested repo '{}': {}",
                    escape(&parent_alias),
                    escape(alias),
                    escape(&problem)
                );
                if strict {
                    anyhow::bail!("{why} (strict mode)");
                }
                super::warn(format!("{why}, skipping"));
                continue;
            }

            let branch = resolve_branch(worktree_name, branch_flag, None);
            log::info!(
                "Adding intermediate parent '{}' for nested repo '{}'",
                escape(&parent_alias),
                escape(alias)
            );
            to_add.push((parent_alias, parent_source, branch));
        }
    }

    if to_add.is_empty() {
        // Still sort so parent-before-child order is guaranteed even when
        // all intermediate parents were already in the explicit list.
        let mut sorted = repos;
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        return Ok(sorted);
    }

    // Insert parents before the repos list so they're created first
    // (the worktree loop creates "." first, then children in order)
    let mut result = Vec::with_capacity(repos.len() + to_add.len());
    // Add "." first if present
    let mut rest = Vec::new();
    for entry in repos {
        if entry.0 == "." {
            result.push(entry);
        } else {
            rest.push(entry);
        }
    }
    // Parents next (sorted to ensure parents come before children)
    to_add.sort_by(|a, b| a.0.cmp(&b.0));
    result.extend(to_add);
    // Then the originally requested repos — also sorted so nested explicit
    // requests (e.g. --repo a/b --repo a/b/c) respect parent-before-child order
    rest.sort_by(|a, b| a.0.cmp(&b.0));
    result.extend(rest);

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &std::path::Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn roll_back_deletes_a_set_whose_records_were_not_all_written() {
        let tmp = tempfile::tempdir().unwrap();
        let set = tmp.path().join("set");
        for (repo, wt) in [("root", set.clone()), ("leaf", set.join("leaf"))] {
            let source = tmp.path().join(repo);
            std::fs::create_dir(&source).unwrap();
            git(&source, &["init", "-q"]);
            git(&source, &["commit", "-q", "--allow-empty", "-m", "init"]);
            git(
                &source,
                &["worktree", "add", "-q", "-b", repo, wt.to_str().unwrap()],
            );
        }
        // The root was recorded; the leaf's record write failed
        let set_record = SetRecord {
            set: set.canonicalize().unwrap(),
            project: tmp.path().to_path_buf(),
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
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "-q"]);
        git(tmp.path(), &["commit", "-q", "--allow-empty", "-m", "init"]);
        let mut journal = Vec::new();

        // Our add created the branch: journaled.
        let created = journal_new_branch(tmp.path(), "ours", &mut journal, || {
            git(tmp.path(), &["branch", "ours"]);
            Ok(Some("start".to_string()))
        });
        assert!(created.unwrap());
        assert_eq!(journal.len(), 1);

        // Another process created the branch while our add failed: kept and named.
        let err = journal_new_branch(tmp.path(), "theirs", &mut journal, || {
            git(tmp.path(), &["branch", "theirs"]);
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
