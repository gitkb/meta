//! A command's invocation, resolved once (the workspace, the set it runs
//! in, the selected projects), the Request a plugin gets from it, and what
//! meta does with the plugin's answer.

use crate::report::Report;
use crate::subprocess_plugins::SubprocessPlugin;
use crate::{run_list, Cli};
use anyhow::{Context, Result};
use meta_core::config::{self, Workspace as Config};
use meta_core::repo_filter::{repo_path, Candidate, Reach};
use meta_core::worktree;
use meta_plugin::display::{escape, escape_lines, escape_path};
use meta_plugin::{
    errln, outln, Answer, ErrorCode, ErrorInfo, Plan, Project, ProjectResult, Request, Response,
    Scope, Text,
};
use std::path::{Path, PathBuf};

/// The repos a command runs in, chosen once
pub struct Target {
    /// The folder they are named from (`.`): the workspace's (with `-r` or
    /// `-A`, the outermost's), or the worktree set's
    pub root: PathBuf,
    /// The selected ones, in order
    pub repos: Vec<Candidate>,
    /// `--parallel` or `--sequential`, else the config's default
    pub parallel: bool,
    /// The workspace, as a plugin is told it
    pub workspace: Option<meta_plugin::Workspace>,
    /// The worktree set it runs in
    pub set: Option<meta_plugin::Set>,
    /// How far into nested meta repos the selection reaches
    pub depth: Option<usize>,
}

impl Target {
    /// The selected repos' folders.
    pub fn dirs(&self) -> Vec<PathBuf> {
        self.repos.iter().map(|repo| repo.dir.clone()).collect()
    }

    /// The selected repos as a plugin is told them.
    pub fn projects(&self) -> Vec<Project> {
        projects(&self.repos, &self.root)
    }
}

/// `repos` as a plugin is told them, named from `root`; each meta repo with
/// the meta repos among them under it.
fn projects(repos: &[Candidate], root: &Path) -> Vec<Project> {
    let mut projects: Vec<Project> = repos
        .iter()
        .map(|repo| {
            let info = repo.info.as_ref();
            Project {
                name: repo.name().unwrap_or(".").to_string(),
                path: repo_path(&repo.dir, root),
                abs: repo.dir.clone(),
                repo: info
                    .and_then(|i| i.repo.as_ref())
                    .map(|r| r.raw().to_string()),
                tags: repo.tags().to_vec(),
                depends_on: info.map(|i| i.depends_on.clone()).unwrap_or_default(),
                level: None,
                present: repo.dir.exists(),
                nested: Vec::new(),
            }
        })
        .collect();
    let metas: Vec<String> = repos
        .iter()
        .zip(&projects)
        .filter(|(repo, _)| repo.info.as_ref().is_some_and(|i| i.meta))
        .map(|(_, project)| project.path.clone())
        .collect();
    for project in &mut projects {
        let under = |path: &String| match project.path.as_str() {
            "." => path != ".",
            parent => path.starts_with(&format!("{parent}/")),
        };
        project.nested = metas.iter().filter(|path| under(path)).cloned().collect();
    }
    projects
}

/// The workspace of `config` as a plugin is told it: its repos are
/// `members`, named from `root`.
fn workspace(config: &Config, root: &Path, members: &[Candidate]) -> meta_plugin::Workspace {
    let owner = worktree::Configs::of(config.clone())
        .map(|configs| configs.owner)
        .unwrap_or_else(|_| config.clone());
    meta_plugin::Workspace {
        root: root.to_path_buf(),
        config: config.config_path.clone(),
        listed_root: config::listed_root(&config.dir),
        members: projects(members, root),
        worktree: meta_plugin::Worktree {
            // refused here, the worktree commands say why
            dir: worktree::resolve_worktree_root(Some(&owner), &config.dir).ok(),
            hooks: owner
                .worktree
                .hooks
                .iter()
                .map(|(event, command)| (event.name().to_string(), command.clone()))
                .collect(),
        },
    }
}

/// The repos `cli`'s filters select from `start`: the checkouts of the
/// worktree set it is in (unless `--primary`), else the repos of the
/// workspace whose config is nearest at or above it (`--config`'s, if
/// given; with `--recursive`, of the nested meta repos too).
pub fn target(cli: &Cli, start: &Path) -> Result<Target> {
    target_with(cli, start, &cli.filters())
}

/// [`target`], selecting with `filters`.
fn target_with(
    cli: &Cli,
    start: &Path,
    filters: &meta_core::repo_filter::Filters,
) -> Result<Target> {
    // --parallel wins, then --sequential, then the default of the config
    // the command runs from (parallel without one)
    let parallel = |workspace: Option<&Config>| {
        cli.parallel || !cli.sequential && workspace.is_none_or(|w| w.defaults.parallel)
    };

    // Inside a set, the command runs in the set's checkouts
    let location = match cli.primary {
        true => None,
        false => worktree::detect_worktree_location(start)?,
    };
    if let Some((name, set_dir)) = location {
        let members: Vec<_> = worktree::discover_worktree_members(&set_dir)?
            .into_iter()
            .map(|member| member.path)
            .collect();
        // Inside a set, a command never falls back to the workspace's own
        // checkouts: they aren't the ones the caller is in. A plain clone
        // that is merely under a `.worktrees` directory is no set.
        if members.is_empty() && worktree::holds_meta_checkout(&set_dir) {
            anyhow::bail!(
                "No checkouts of worktree set '{}' found in {}. Run from a set meta made, or pass --primary to run in the workspace.",
                escape(&name),
                escape_path(&set_dir)
            );
        }
        if !members.is_empty() {
            if cli.verbose {
                errln!(
                    "Detected worktree context: '{}' ({} repos)",
                    escape(&name),
                    members.len()
                );
            }
            let config = worktree::set_config(&set_dir, &members, cli.config.as_ref())?
                .map(|config| Config::load(&config))
                .transpose()?;
            if cli.verbose {
                match &config {
                    Some(w) => errln!("Using config {}", escape_path(&w.config_path)),
                    None => meta_plugin::out::warn(format!(
                        "No .meta.yaml found for worktree '{}'. Tags, plugins, and dependency features unavailable.",
                        escape(&name)
                    )),
                }
            }
            let all = worktree::select_in_set(
                &set_dir,
                &members,
                config.as_ref(),
                &meta_core::repo_filter::Filters::default(),
            )?;
            let repos = worktree::select_in_set(&set_dir, &members, config.as_ref(), filters)?;
            let set = meta_plugin::Set {
                name,
                dir: set_dir.clone(),
                members: projects(&all, &set_dir),
            };
            return Ok(Target {
                parallel: parallel(config.as_ref()),
                workspace: config.as_ref().map(|c| workspace(c, &set_dir, &all)),
                set: Some(set),
                root: set_dir,
                repos,
                depth: Some(0),
            });
        }
    }

    let Some(config) = Config::find(start, cli.config.as_ref())? else {
        let config_name = cli
            .config
            .as_ref()
            .map(|p| escape_path(p))
            .unwrap_or_else(|| config::META_CONFIG_NAME.to_string());
        return Err(ErrorInfo::new(
            ErrorCode::NotFound,
            format!(
                "Could not find meta config file '{config_name}'\nSearched from {} up to root",
                escape_path(start)
            ),
        )
        .into());
    };
    if cli.verbose {
        errln!(
            "Resolved config file path: {}",
            escape_path(&config.config_path)
        );
    }
    // From the config already loaded (the default or --config's): -r into
    // the meta repos nested under it, -A from the outermost one
    let parallel = parallel(Some(&config));
    let reach = Reach::of(config, cli.recursive, cli.all, cli.depth)?;
    let members = reach.scope().repos;
    let repos = reach.select(filters, crate::with_all)?;
    let depth = match cli.recursive || cli.all {
        true => cli.depth,
        false => Some(0),
    };
    Ok(Target {
        workspace: Some(workspace(&reach.workspace, &reach.root, &members)),
        repos,
        parallel,
        root: reach.root,
        set: None,
        depth,
    })
}

/// The worktree set `name` of the workspace at or above `start`, and the
/// checkouts in it `cli`'s filters select. None outside a workspace; no
/// set when there is none of that name (the command says so).
pub fn named_set(cli: &Cli, start: &Path, name: &str) -> Result<Option<Target>> {
    let Some(config) = Config::find(start, cli.config.as_ref())? else {
        return Ok(None);
    };
    // the workspace alone: the filters select in the set
    let mut target = target_with(cli, start, &meta_core::repo_filter::Filters::default())?;
    let dir = target
        .workspace
        .as_ref()
        .and_then(|w| w.worktree.dir.clone());
    let set_dir = match dir {
        Some(dir) if meta_core::worktree::validate_name(name).is_ok() => dir.join(name),
        _ => {
            return Ok(Some(Target {
                repos: Vec::new(),
                ..target
            }))
        }
    };
    let members: Vec<PathBuf> = match set_dir.is_dir() {
        true => worktree::discover_worktree_members(&set_dir)?
            .into_iter()
            .map(|member| member.path)
            .collect(),
        false => Vec::new(),
    };
    if members.is_empty() {
        target.repos = Vec::new();
        return Ok(Some(target));
    }
    let set_config = worktree::set_config(&set_dir, &members, cli.config.as_ref())?
        .map(|config| Config::load(&config))
        .transpose()?
        .unwrap_or(config);
    let all = worktree::select_in_set(
        &set_dir,
        &members,
        Some(&set_config),
        &meta_core::repo_filter::Filters::default(),
    )?;
    let repos = worktree::select_in_set(&set_dir, &members, Some(&set_config), &cli.filters())?;
    Ok(Some(Target {
        set: Some(meta_plugin::Set {
            name: name.to_string(),
            dir: set_dir.clone(),
            members: projects(&all, &set_dir),
        }),
        root: set_dir,
        repos,
        ..target
    }))
}

/// The Request for `args` (the command as typed after meta's options,
/// starting with the plugin's word) served by `plugin`, from `start`, in
/// `target` (none for a command that needs no workspace).
pub fn request(
    cli: &Cli,
    plugin: &SubprocessPlugin,
    args: &[String],
    start: &Path,
    scope: Scope,
    target: Option<&Target>,
) -> Result<Request> {
    let (command, rest) = args
        .split_at_checked(plugin.command_len(args))
        .unwrap_or((args, &[]));
    let mut request = Request::new(command.join(" "), start);
    request.args = rest.to_vec();
    request.scope = scope;
    if let Some(target) = target {
        request.workspace = target.workspace.clone();
        request.set = target.set.clone();
        request.depth = target.depth;
        request.recursive = cli.recursive || cli.all;
        if matches!(scope, Scope::Selection | Scope::NamedSet) {
            request.projects = target.projects();
        }
    } else if cli.recursive || cli.all {
        request.depth = cli.depth;
        request.recursive = true;
    }
    request.plugin_dir = plugin
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    request.data_dir = meta_core::data_dir::data_subdir(&format!("data/{}", plugin.word))?;
    request.meta = meta_plugin::Meta {
        json: cli.json,
        dry_run: cli.dry_run,
        strict: cli.strict,
        verbose: cli.verbose,
        silent: cli.silent,
        color: meta_plugin::out::color(),
        interactive: std::io::IsTerminal::is_terminal(&std::io::stdin()),
        jobs: match target.map_or(cli.parallel, |t| t.parallel) {
            true => None,
            false => Some(1),
        },
    };
    request.meta_options = crate::meta_options();
    Ok(request)
}

/// Call `plugin` with `request` and do what it answers, into `report`.
/// A delegate is returned: the command line to run instead.
pub fn call(
    cli: &Cli,
    plugin: &SubprocessPlugin,
    request: &Request,
    target: Option<&Target>,
    report: &mut Report,
) -> Result<Option<Vec<String>>> {
    // a command run as typed is named with its verb: `git status`
    report.command = match (request.command.contains(' '), request.args.first()) {
        (false, Some(verb)) if !verb.starts_with('-') => format!("{} {verb}", request.command),
        _ => request.command.clone(),
    };
    crate::running(&report.command);
    let (response, received) = plugin.call(request, cli.show())?;
    // what it streamed came first; a record that is a failure fails the call
    if let Some(error) = received.records.iter().find_map(|r| r.error.clone()) {
        report.fail(error, !cli.json);
    }
    report.results.extend(received.records);
    report.warnings.extend(received.warnings);
    answer(cli, response, target, report)
}

/// Do what a plugin answered: run its plan, show its results or words, or
/// report its error; warnings and skipped projects are said first.
pub fn answer(
    cli: &Cli,
    response: Response,
    target: Option<&Target>,
    report: &mut Report,
) -> Result<Option<Vec<String>>> {
    let text = !cli.json;
    if text {
        for warning in &response.warnings {
            meta_plugin::out::warn(&warning.message);
        }
        let said: Vec<String> = response
            .skipped
            .iter()
            .filter(|s| !s.quiet)
            .map(|s| format!("{} ({})", escape(&s.project), escape(&s.reason)))
            .collect();
        if !said.is_empty() && !cli.silent {
            errln!("skipped: {}", said.join(", "));
        }
    }
    report.warnings.extend(response.warnings);
    report.skipped.extend(response.skipped);
    match response.answer {
        Answer::Plan(plan) => run_plan(cli, &plan, target, report)?,
        Answer::Results(results) => {
            if text && results.text == Text::Host {
                show(&results.records)?;
            }
            let failed: Vec<&ProjectResult> =
                results.records.iter().filter(|r| r.is_failure()).collect();
            if let Some(first) = failed.first().and_then(|r| r.error.clone()) {
                let error = match failed.len() {
                    1 => first,
                    n => ErrorInfo::new(
                        first.code,
                        format!("{n} of {} records are failures", results.records.len()),
                    ),
                };
                // told already when the host showed the records
                report.fail(error, text && results.text == Text::Host);
            }
            report.results.extend(results.records);
        }
        Answer::Message(message) => match text {
            true if message.is_empty() => {}
            true => outln!("{}", message.trim_end())?,
            false => report.message = Some(message),
        },
        Answer::Delegate(args) => return Ok(Some(args)),
        Answer::Error(error) => report.fail(error, false),
    }
    Ok(None)
}

/// The records a command returned, in text: each under its project, its
/// data as text, escaped (the data came from a repo or a plugin).
pub fn show(records: &[ProjectResult]) -> Result<()> {
    for record in records {
        let header = record.project.as_deref().map(escape);
        if let Some(error) = &record.error {
            let at = header.map(|h| format!("{h}: ")).unwrap_or_default();
            meta_plugin::out::warn(format!("✗ {at}{}", error.message));
            continue;
        }
        let body = match &record.data {
            None => continue,
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(data) => serde_json::to_string_pretty(data).unwrap_or_default(),
        };
        match header {
            Some(header) => outln!("{header}:\n{}", indent(&escape_lines(&body)))?,
            None => outln!("{}", escape_lines(&body))?,
        }
    }
    Ok(())
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Run `plan` with the loop engine, its steps' outcome into `report`.
pub fn run_plan(
    cli: &Cli,
    plan: &Plan,
    target: Option<&Target>,
    report: &mut Report,
) -> Result<()> {
    use loop_lib::{DirCommand, LoopConfig};
    // Checked before any step runs, so a plan never stops half done over it
    let max_parallel = plan
        .max_parallel
        .map(|max| {
            std::num::NonZeroUsize::new(max).ok_or_else(|| {
                ErrorInfo::new(
                    ErrorCode::PluginFailed,
                    "The plugin's plan has max_parallel 0",
                )
            })
        })
        .transpose()?;
    let commands: Vec<DirCommand> = plan
        .steps
        .iter()
        .map(|step| DirCommand {
            project: step.project.clone(),
            env: (!step.env.is_empty()).then(|| {
                step.env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            }),
            ok_codes: step.ok_codes.clone(),
            ..DirCommand::new(step.dir.clone(), step.argv.clone())
        })
        .collect();
    // only in the selected projects that are repositories
    let dirs = target.map(Target::dirs).unwrap_or_default();
    let root = target.map(|t| t.root.clone());
    let commands = run_list::in_repos(commands, &dirs, root.as_deref());
    // Repos are named by their path from the root (shown as "."). Without
    // a workspace the plugin lists the root first.
    let root_dir = root.or_else(|| commands.first().map(|c| c.dir.clone()));
    let config = LoopConfig {
        verbose: cli.verbose,
        silent: cli.silent,
        parallel: plan
            .parallel
            .unwrap_or_else(|| target.map_or(cli.parallel, |t| t.parallel)),
        dry_run: cli.dry_run,
        json_output: cli.json,
        on_done: steps(cli),
        spawn_stagger_ms: plan.stagger_ms.unwrap_or(0),
        max_parallel,
        aliases: loop_lib::aliases()?,
        root_dir,
        outer: Some(crate::outer(&[])),
        ..LoopConfig::default()
    };
    let outcome = loop_lib::run_commands(&config, &commands)?;
    report.results.extend(outcome.rows());
    if let Some(error) = outcome.error() {
        report.fail(error, !cli.json);
    }
    Ok(())
}

/// `meta exec`: `argv` as typed in each selected repo.
pub fn exec(cli: &Cli, argv: &[String], report: &mut Report) -> Result<()> {
    let start = cli.start_dir()?;
    let target = target(cli, &start)?;
    if cli.verbose {
        errln!(
            "Executing command: {}",
            escape(&loop_lib::display_command(argv))
        );
    }
    let projects = target.projects();
    let commands = target
        .repos
        .iter()
        .zip(&projects)
        .map(|(repo, project)| loop_lib::DirCommand {
            project: Some(project.path.clone()),
            ..loop_lib::DirCommand::new(repo.dir.clone(), argv.to_vec())
        })
        .collect();
    let dirs = target.dirs();
    let commands = run_list::in_repos(commands, &dirs, Some(&target.root));
    let config = loop_lib::LoopConfig {
        verbose: cli.verbose,
        silent: cli.silent,
        parallel: target.parallel,
        dry_run: cli.dry_run,
        json_output: cli.json,
        on_done: steps(cli),
        aliases: loop_lib::aliases()?,
        root_dir: Some(target.root.clone()),
        outer: Some(crate::outer(&["exec", "--"])),
        ..Default::default()
    };
    let outcome = loop_lib::run_commands(&config, &commands)
        .with_context(|| format!("Cannot run {}", escape(&loop_lib::display_command(argv))))?;
    report.results.extend(outcome.rows());
    if let Some(error) = outcome.error() {
        report.fail(error, !cli.json);
    }
    Ok(())
}

/// Under `--json-stream`, a `step` event as each step of a plan ends.
fn steps(cli: &Cli) -> loop_lib::OnDone {
    match cli.show() {
        crate::events::Show::Stream => loop_lib::OnDone::call(|result| {
            crate::events::show(
                crate::events::Show::Stream,
                &meta_plugin::Event::Step(result.row()),
            )
        }),
        _ => loop_lib::OnDone::default(),
    }
}
