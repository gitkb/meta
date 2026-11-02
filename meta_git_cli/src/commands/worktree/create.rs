use anyhow::{Context, Result};
use chrono::Utc;
use colored::*;
use meta_core::display::{escape, escape_path};
use meta_core::out;
use meta_core::ExecutionMode;
use meta_core::{errln, outln};
use std::collections::BTreeSet;
use std::collections::HashMap;

use meta_cli::dependency_graph::{unresolved_message, DependencyGraph, ProjectDependencies};
use meta_core::duration::format_duration;
use meta_core::set_record::{self as record, SetRecord};
use meta_git_lib::worktree::git_ops::*;
use meta_git_lib::worktree::helpers::*;
use meta_git_lib::worktree::hooks::fire_post_create;
use meta_git_lib::worktree::lease;
use meta_git_lib::worktree::plan::{Found, Index, PlannedRepo};
use meta_git_lib::worktree::safety::{check_worktree_removal, delete_checked_set};
use meta_git_lib::worktree::types::{CreateOutput, CreateRepoEntry};

use super::cli_types::CreateArgs;
use meta_core::display::shell_quote;

/// A set a create made: its directory, and its lease, held until dropped.
pub(crate) struct Created {
    pub dir: std::path::PathBuf,
    pub lease: lease::Held,
}

/// Make the set `args` asks for; `None` for a preview, which makes none.
pub(crate) fn handle_create(
    args: CreateArgs,
    mode: ExecutionMode,
    verbose: bool,
    json: bool,
    global_strict: bool,
) -> Result<Option<Created>> {
    // Merge global --strict with local --strict (either enables strict mode)
    let strict = args.strict || global_strict;

    let name = &args.name;
    validate_worktree_name(name)?;

    let ephemeral = args.ephemeral;
    let ttl_seconds = args.ttl;
    // Parse custom metadata, collecting any invalid entries for strict mode
    let mut custom_meta = HashMap::new();
    for s in &args.custom_meta {
        if let Some((key, value)) = s.split_once('=') {
            custom_meta.insert(key.to_string(), value.to_string());
        } else {
            out::warn_or_bail(
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

    let recursive = args.recursive;

    if args.repos.is_empty() && !args.all {
        anyhow::bail!("Specify repos with --repo <alias> or use --all");
    }
    let cwd = std::env::current_dir()?;
    let configs = Configs::find(&cwd)?;
    // --recursive works from the root of the workspace tree
    let configs = match recursive {
        true => Configs::of(configs.here.root()?)?,
        false => configs,
    };
    let worktree_root = resolve_worktree_root(Some(&configs.owner), &cwd)?;

    // Check if worktree already exists
    let wt_dir = worktree_root.join(name);
    if wt_dir.exists() {
        let lease = wt_dir.canonicalize().ok().and_then(|set| lease::find(&set));
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

    // The repos of the set, each resolved once: through one index of the
    // projects (the whole tree, walked once, with --recursive)
    let index = match recursive {
        true => Index::recursive(&configs.here)?,
        false => Index::new(&configs.here),
    };
    let mut repos_to_create = plan_repos(&index, &args, strict, verbose)?;

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
                Some(r) => out::warn_or_bail(
                    strict,
                    format!(
                        "Skipping '{}': ref '{}' not found",
                        escape(&repo.project),
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
        return Ok(None);
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
        lease::take(&lease::Lease::new(
            set,
            configs.owner.dir.clone(),
            ephemeral,
        ))?
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
        // Each repo where it is in the workspace, the root first (it becomes
        // the set's root: git accepts the empty directory claimed above),
        // then each repo before those inside it
        for repo in &repos_to_create {
            if let Some(r) = missing_ref(&repo.source, from_ref)? {
                // skip repos where the commit-ish doesn't exist
                out::warn_or_bail(
                    strict,
                    format!(
                        "Skipping '{}': ref '{}' not found",
                        escape(&repo.project),
                        escape(r)
                    ),
                )?;
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
            let created_branch =
                journal_new_branch(&repo.source, &repo.branch, &mut new_branches, || {
                    git_worktree_add(&repo.source, &dest, &repo.branch, from_ref)
                })?;
            mark(&dest)?;
            created_repos.push(CreateRepoEntry {
                alias: repo.project.clone(),
                path: dest.display().to_string(),
                branch: repo.branch.clone(),
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
    if json {
        outln!("{}", serde_json::to_string_pretty(&output)?)?;
    } else {
        outln!(
            "{} Created worktree '{}' at {}",
            "✓".green(),
            name.bold(),
            escape_path(&wt_dir)
        )?;
        for r in &output.repos {
            let branch_note = if r.created_branch { " (new)" } else { "" };
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
        Some(&configs.owner),
    )
    .or_else(|e| out::warn_or_bail(strict, format!("{e:#}")));
    if let Err(e) = hook {
        // the set is made: an ephemeral one is the next prune's
        if ephemeral {
            lease.leave();
        }
        return Err(e);
    }
    Ok(Some(Created { dir: wt_dir, lease }))
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
    for PlannedRepo {
        project: alias,
        source,
        place,
        branch,
    } in plan.repos_to_create
    {
        let dest = if place == "." {
            plan.wt_dir.to_path_buf()
        } else {
            plan.wt_dir.join(place)
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

/// Why a repo is in a set being made. A project that isn't cloned is an
/// error when it was asked for or is needed; with `--all`, or as a parent
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
fn plan_repos(
    index: &Index,
    args: &CreateArgs,
    strict: bool,
    verbose: bool,
) -> Result<Vec<PlannedRepo>> {
    let workspace = index.workspace;
    let meta_dir = workspace.dir.as_path();
    let branch = |asked: Option<&str>| resolve_branch(&args.name, args.branch.as_deref(), asked);
    let mut plan = Plan {
        meta_dir,
        repos: Vec::new(),
        asked: HashMap::new(),
    };

    if args.all {
        // the root, when something is there for git (one git can't use is
        // skipped with the rest, saying why), and every project: by name,
        // or with --recursive, every place of the tree
        // (exec --ephemeral --all may give a repo its branch: --repo name:branch)
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
        match index.tree() {
            Some(tree) => {
                let mut places: Vec<&String> = tree.keys().collect();
                places.sort();
                for place in places {
                    all(index.place(place)?)?;
                }
            }
            None => {
                for project in &workspace.projects {
                    all(index.find(&project.name)?)?;
                }
            }
        }
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
            let graph = match index.tree() {
                Some(tree) => nested_dep_graph(tree)?,
                None => DependencyGraph::build(
                    workspace
                        .projects
                        .iter()
                        .map(|p| p.clone().into())
                        .collect(),
                ),
            };
            graph.warn_duplicates(strict)?;
            // a dependency the set would need and can't have
            let mut needed: BTreeSet<&str> = BTreeSet::new();
            for key in asked_keys.iter().filter(|key| *key != ".") {
                needed.insert(key);
                needed.extend(graph.get_all_dependencies(key));
            }
            for (project, dep) in graph.unresolved() {
                if needed.contains(project.as_str()) {
                    let hint = match index.tree() {
                        None => " (nested projects count with -r)",
                        Some(_) => "",
                    };
                    out::warn_or_bail(
                        strict,
                        format!("{}{hint}", unresolved_message(project, dep)),
                    )?;
                }
            }
            for key in asked_keys.iter().filter(|key| *key != ".") {
                let deps = graph.get_all_dependencies(key);
                log::debug!("Resolved transitive dependencies for '{key}': {deps:?}");
                for dep in deps {
                    if verbose {
                        errln!(
                            "  Including '{}' (dependency of '{}')",
                            escape(dep),
                            escape(key)
                        );
                    }
                    // the graph names projects as the index keys them
                    let found = match index.tree() {
                        Some(_) => index.place(dep)?,
                        None => index.find(dep)?,
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
            let Some(source) = index.at_place(&parent)? else {
                // A plain directory on the way is no parent; a repo that
                // isn't a project is worth saying so
                if meta_core::gitdir::common_dir(&meta_dir.join(&parent))?.is_some() {
                    out::warn(format!(
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

    // A project not cloned yet has no repository to make a worktree of
    let mut repos = Vec::with_capacity(plan.repos.len());
    for (repo, why) in plan.repos {
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
                Some(problem) if why != Why::All => anyhow::bail!("{problem}"),
                problem => problem,
            },
        };
        let Some(why) = why else {
            repos.push(repo);
            continue;
        };
        if strict {
            anyhow::bail!("{why} (strict mode)");
        }
        out::warn(format!("{why}, skipping"));
    }
    repos.sort_by(|a, b| {
        let order = |r: &PlannedRepo| (r.place != ".", r.place.split('/').count());
        order(a).cmp(&order(b)).then_with(|| a.place.cmp(&b.place))
    });
    Ok(repos)
}

/// The dependency graph of the whole tree (`tree`, every project by its
/// place), named by place. A project's `depends_on` and `provides` are
/// qualified with the place of the meta repo that lists it, so they name
/// places too: `depends_on: [vendor/x]` of `open-source/gitkb/core` is
/// `open-source/gitkb/vendor/x`.
fn nested_dep_graph(
    tree: &HashMap<String, (std::path::PathBuf, meta_core::config::ProjectInfo)>,
) -> Result<DependencyGraph> {
    let mut project_deps = Vec::new();
    for (full_path, (_, info)) in tree {
        // full_path="open-source/gitkb/core", info.path="core" → "open-source/gitkb/"
        let parent_prefix = full_path
            .strip_suffix(info.path.as_str())
            .filter(|prefix| prefix.is_empty() || prefix.ends_with('/'))
            .unwrap_or("");
        let qualify = |name: &str| {
            if tree.contains_key(name) || name == "." {
                name.to_string()
            } else {
                format!("{parent_prefix}{name}")
            }
        };
        project_deps.push(ProjectDependencies {
            name: full_path.clone(),
            path: full_path.clone(),
            repo: info.repo.clone(),
            tags: info.tags.clone(),
            provides: info.provides.iter().map(|p| qualify(p)).collect(),
            depends_on: info.depends_on.iter().map(|d| qualify(d)).collect(),
        });
    }
    Ok(DependencyGraph::build(project_deps))
}

#[cfg(test)]
#[expect(
    clippy::disallowed_methods,
    reason = "tests start the programs they test with"
)]
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
