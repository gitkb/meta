//! `meta project`: the workspace's projects, as its `.meta.yaml` files list
//! them.

use crate::invoke::{self, Target};
use crate::report::Report;
use crate::Cli;
use anyhow::Result;
use meta_core::config::{self, MetaTreeNode};
use meta_core::dependency_graph::{DependencyGraph, ProjectDependencies};
use meta_core::repo_filter::{repo_path, Candidate};
use meta_plugin::display::escape;
use meta_plugin::{outln, ErrorCode, ErrorInfo, ProjectResult, RepoUrl};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Arguments for `meta project`
#[derive(clap::Parser)]
#[command(
    name = "project",
    subcommand_required = true,
    arg_required_else_help = true,
    after_help = "Agent hints:
  Prefer `meta -r --json project list` for codebase discovery.
  Use `meta -r project check` before assuming a repo is intentionally absent.

Examples:
  meta -r --json project list
  meta project check
  meta project dependents meta_core

To clone missing projects, use: meta git clone-missing"
)]
pub struct ProjectArgs {
    #[command(subcommand)]
    pub command: ProjectCommands,
}

#[derive(clap::Subcommand)]
pub enum ProjectCommands {
    /// List the projects defined in .meta.yaml
    #[command(
        visible_alias = "ls",
        after_help = "meta's options go before the command: -r includes the projects of nested meta \
repos (--depth N limits how deep); --json emits one record per project, the workspace's own \
first: its path from the workspace root, repo URL, tags, and with -r the nested projects too.

Agent hints:
  Use `meta -r --json project list` before broad changes to discover the full repo graph.
  Use non-recursive output for a quick human-readable view of the current meta repo.

Examples:
  meta project list
  meta -r project list
  meta -r --json project list"
    )]
    List,
    /// Verify configured projects are cloned (meta --recursive checks nested meta repos as well)
    #[command(after_help = "Agent hints:
  Run this when commands fail because a child checkout is missing.
  Missing repos can usually be cloned with `meta git clone-missing`.

Examples:
  meta project check
  meta --json project check
  meta -r project check

meta's --json emits one record per project that is not a clone of its configured repository.")]
    Check,
    /// List the projects that depend on a project or capability
    #[command(after_help = "Agent hints:
  Use this before changing shared libraries or APIs to identify likely blast radius.
  Pair with `meta -r --json project list` when working across nested meta repos.

Examples:
  meta project dependents meta_core
  meta --json project dependents auth
  meta -A project dependents auth

meta's -r also looks in the meta repos nested under the nearest .meta.yaml, -A in the whole \
workspace from its root; --json emits one record per dependent project.")]
    Dependents {
        /// Project name or provided capability to search for
        project: String,
    },
    /// A word that is none of these: a usage error naming the one it is near
    #[command(external_subcommand)]
    Other(Vec<String>),
}

impl ProjectCommands {
    /// Its words, as the Envelope names the command.
    pub fn name(&self) -> &'static str {
        match self {
            ProjectCommands::List => "project list",
            ProjectCommands::Check => "project check",
            ProjectCommands::Dependents { .. } => "project dependents",
            ProjectCommands::Other(_) => "project",
        }
    }
}

/// Whether `meta project <name>` finds its projects itself, in the
/// `.meta.yaml` files: it takes none of meta's filters. `check` checks the
/// projects they select.
fn finds_its_repos(name: &str) -> bool {
    name != "check"
}

/// meta's filter options, by their long spellings.
const FILTERS: [&str; 3] = ["--include", "--exclude", "--tag"];

/// Run `meta project …`, into `report`.
pub fn run(cli: &Cli, command: &ProjectCommands, report: &mut Report) -> Result<()> {
    let start = cli.start_dir()?;
    let found = match command {
        ProjectCommands::Other(words) => return Err(unrecognized(words, &crate::meta_options())),
        ProjectCommands::Check => return check(cli, &start, report),
        ProjectCommands::List => list(&workspace_target(cli, &start, "list")?, &start)?,
        ProjectCommands::Dependents { project } => {
            dependents(&workspace_target(cli, &start, "dependents")?, project)?
        }
    };
    if !cli.json {
        outln!("{}", found.text)?;
    }
    report.results.extend(found.records);
    Ok(())
}

/// The usage error for clap's `error` about meta's line when its command is
/// `meta project <name> …` and `name` takes none of meta's filters: as the
/// command's own parse gives it, with those filters refused (a tip that
/// moved one before the command would give a line refused there too). None
/// for any other line, or for help and version.
pub fn usage_error(error: &clap::Error) -> Option<anyhow::Error> {
    use clap::error::ErrorKind;
    if matches!(
        error.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        return None;
    }
    let typed: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let meta = crate::meta_options();
    let args = match typed.get(meta.typed.len()..)?.split_first()? {
        (word, args) if word == "project" => args,
        _ => return None,
    };
    let verb = args.first()?;
    let command = <ProjectArgs as clap::CommandFactory>::command();
    if !finds_its_repos(command.find_subcommand(verb)?.get_name()) {
        return None;
    }
    // filters typed before the command are refused first, as when it runs
    let filtered = FILTERS.iter().any(|filter| meta.typed_has(filter));
    let error = match crate::refuse_filters(filtered, &["project", verb]) {
        Err(refused) => refused,
        Ok(()) => {
            let refusing = meta.refusing(&FILTERS);
            let response =
                meta_plugin::parse_args::<ProjectArgs>("project", args, &refusing).err()?;
            match response.answer {
                meta_plugin::Answer::Error(error) => error.into(),
                _ => return None,
            }
        }
    };
    // with --json, the error in the Envelope
    if meta
        .typed
        .iter()
        .any(|word| word == "--json" || word == "--json-stream")
    {
        meta_plugin::out::set_json(true);
    }
    Some(error)
}

/// `meta project <word> …` (`words`) for a word that is none of its
/// commands: the one it is near, as the line typed with meta's options
/// (`meta`'s) before the command, and the commands there are.
fn unrecognized(words: &[String], meta: &meta_plugin::args::MetaOptions) -> anyhow::Error {
    let mut cli = <Cli as clap::CommandFactory>::command();
    cli.build();
    let project = cli.find_subcommand_mut("project");
    let names: Vec<String> = project
        .iter()
        .flat_map(|project| project.get_subcommands())
        .flat_map(|sub| std::iter::once(sub.get_name()).chain(sub.get_visible_aliases()))
        .map(str::to_string)
        .collect();
    let near = words
        .first()
        .and_then(|word| meta_plugin::args::nearest(word, names.iter().map(String::as_str)));
    // meta's options go before the command: the ones typed there, then the
    // ones typed after it
    let line = near.and_then(|word| {
        let (moved, rest) = meta.lift(words.get(1..).unwrap_or_default())?;
        let line: Vec<String> = std::iter::once("meta".to_string())
            .chain(meta.typed.iter().cloned())
            .chain(moved)
            .chain(["project".to_string(), word.to_string()])
            .chain(rest)
            .collect();
        Some(meta_plugin::display::display_command(&line))
    });
    let suggestion = match line {
        Some(line) => format!(
            "\n\nA command with a similar name exists:\n    {}",
            escape(&line)
        ),
        None => String::new(),
    };
    let help = project
        .map(|project| project.render_help().to_string())
        .unwrap_or_default();
    let typed: Vec<&str> = std::iter::once("project")
        .chain(words.iter().map(String::as_str))
        .collect();
    meta_plugin::exit::Usage(format!(
        "unrecognized command '{}'{suggestion}\n\n{}",
        escape(&typed.join(" ")),
        help.trim_end()
    ))
    .into()
}

/// The repos of the workspace for `meta project <name>`, which finds its
/// projects itself: meta's filters are refused, and outside a workspace it
/// says so.
fn workspace_target(cli: &Cli, start: &Path, name: &str) -> Result<Target> {
    // named as typed: `ls` too
    let typed: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let typed = typed
        .get(crate::meta_options().typed.len() + 1)
        .map_or(name, String::as_str);
    crate::refuse_filters(!cli.filters().is_empty(), &["project", typed])?;
    let target = match invoke::target(cli, start) {
        Ok(target) => Some(target),
        Err(e) if meta_plugin::envelope::code_of(&e) == ErrorCode::NotFound => None,
        Err(e) => return Err(e),
    };
    target
        .filter(|target| target.workspace.is_some())
        .ok_or_else(|| {
            ErrorInfo::new(
                ErrorCode::NotFound,
                "Not a meta workspace (no .meta.yaml found)",
            )
            .into()
        })
}

/// What a command found: its text, and its records for `--json`.
struct Found {
    text: String,
    records: Vec<ProjectResult>,
}

/// One row of `meta --json project list`: a project, or the workspace the
/// list is of (the first row).
#[derive(Debug, Clone, Serialize)]
struct ListedProject {
    name: String,
    /// From the workspace root, as the row's `project` (`.` for the root)
    path: String,
    /// Git remote URL. Should be present for all normal projects.
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<RepoUrl>,
    tags: Vec<String>,
    is_meta: bool,
    /// Why its folder couldn't be looked into (its own projects may be
    /// missing from the list); absent when it could.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// The directory the command ran from (the first row only)
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    /// The worktree set it ran in (the first row only)
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<ListedWorktree>,
}

/// Linked worktree context for project list output.
#[derive(Debug, Clone, Serialize)]
struct ListedWorktree {
    linked: bool,
    physical_root: String,
    canonical_root: String,
    name: String,
}

/// One project that is not a clone of what its config lists.
#[derive(Debug, Serialize, PartialEq)]
struct CheckProblem {
    name: String,
    /// From the directory the command runs in.
    path: String,
    /// `missing`, `not_a_repo` or `wrong_remote`.
    state: &'static str,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    repo: Option<RepoUrl>,
    /// The checkout's `origin`, for `wrong_remote` (`null` without one).
    #[serde(skip_serializing_if = "Option::is_none")]
    actual: Option<Option<RepoUrl>>,
}

/// `meta project check`: every selected project is a clone of its
/// configured repository; each that isn't is named, and the command fails.
fn check(cli: &Cli, start: &Path, report: &mut Report) -> Result<()> {
    let target = invoke::target(cli, start)?;
    let problems = check_projects(&target, start);
    if problems.is_empty() {
        let said = "All projects are cloned and present.";
        match cli.json {
            true => report.message = Some(said.to_string()),
            false => outln!("{said}")?,
        }
        return Ok(());
    }
    if cli.json {
        let records: Vec<ProjectResult> = problems
            .into_iter()
            .map(|(repo, problem)| {
                let code = match problem.state {
                    "missing" => ErrorCode::NotFound,
                    "wrong_remote" => ErrorCode::Conflict,
                    _ => ErrorCode::Failed,
                };
                let path = repo_path(&repo.dir, &target.root);
                let error = ErrorInfo::new(
                    code,
                    format!("{}: {}", escape(&path), escape(&problem.reason)),
                );
                ProjectResult {
                    error: Some(error),
                    ..ProjectResult::record(path, problem).at(&repo.dir)
                }
            })
            .collect();
        let first = records.first().and_then(|r| r.error.clone());
        if let Some(first) = first {
            let error = match records.len() {
                1 => first,
                n => ErrorInfo::new(first.code, format!("{n} of {n} records are failures")),
            };
            report.fail(error, false);
        }
        report.results.extend(records);
        return Ok(());
    }
    for (_, problem) in &problems {
        outln!("✗ {}: {}", escape(&problem.path), escape(&problem.reason))?;
    }
    let missing = problems.iter().any(|(_, p)| p.state == "missing");
    anyhow::bail!(
        "{} project(s) are not clones of their configured repository{}",
        problems.len(),
        if missing {
            "; run 'meta git clone-missing' to clone the missing ones"
        } else {
            ""
        }
    )
}

/// What is wrong with the selected projects of `target` (meta's -r, -A
/// and filters applied), in their order. The workspace's own repo is not
/// one of them.
fn check_projects<'a>(target: &'a Target, cwd: &Path) -> Vec<(&'a Candidate, CheckProblem)> {
    target
        .repos
        .iter()
        .filter_map(|repo| Some((repo, check_project(repo, cwd)?)))
        .collect()
}

/// What is wrong with `repo`, if anything (nothing for a root, which is no
/// project).
fn check_project(repo: &Candidate, cwd: &Path) -> Option<CheckProblem> {
    use meta_core::gitdir::Checkout;
    let info = repo.info.as_ref()?;
    let dir = &repo.dir;
    let problem = |state, reason: String| CheckProblem {
        name: info.name.clone(),
        path: dir
            .strip_prefix(cwd)
            .unwrap_or(dir)
            .to_string_lossy()
            .into_owned(),
        state,
        reason,
        repo: info.repo.clone(),
        actual: None,
    };
    let checkout = Checkout::of(dir);
    if let Some(why) = checkout.problem() {
        let state = match checkout {
            Checkout::Missing => "missing",
            _ => "not_a_repo",
        };
        return Some(problem(state, why));
    }
    let expected = info.repo.as_ref()?;
    match meta_core::git::remote_url(dir) {
        Ok(Some(actual)) if actual.same_repository(expected) => None,
        Ok(actual) => Some(CheckProblem {
            actual: Some(actual.clone()),
            ..problem(
                "wrong_remote",
                match actual {
                    Some(actual) => format!("origin is {actual}, but .meta.yaml lists {expected}"),
                    None => format!("has no origin; .meta.yaml lists {expected}"),
                },
            )
        }),
        Err(e) => Some(problem(
            "not_a_repo",
            format!("can't read its origin: {e:#}"),
        )),
    }
}

/// The workspace `target` is in, and its config.
fn workspace(target: &Target) -> Result<(&meta_plugin::Workspace, config::Workspace)> {
    let workspace = target.workspace.as_ref().ok_or_else(|| {
        ErrorInfo::new(
            ErrorCode::NotFound,
            "Not a meta workspace (no .meta.yaml found)",
        )
    })?;
    Ok((workspace, config::Workspace::load(&workspace.config)?))
}

/// `meta project list`: the projects of the config the command runs from
/// (`start`; with -A, the outermost one's), and with -r those of the meta
/// repos nested in it, as a tree; each a record, in the tree's order.
fn list(target: &Target, start: &Path) -> Result<Found> {
    let (workspace, config) = workspace(target)?;
    let dir = listed_in(target, &config);
    let tree = config::walk_projects(&dir, &config.projects, target.depth)?;

    // The remote of the workspace whose projects these are, not of the
    // repository the command runs in (a project's, from inside it); none
    // when the workspace is no repository
    let root_repo = Some(&config.dir)
        .filter(|dir| matches!(meta_core::gitdir::common_dir(dir), Ok(Some(_))))
        .and_then(|dir| meta_core::git::remote_url(dir).ok().flatten());

    let mut text = format!(". ({})\n", remote_label(root_repo.as_ref()));
    format_project_tree(&tree, &mut text, "");
    if text.ends_with('\n') {
        text.pop();
    }

    let worktree = target.set.as_ref().map(|set| {
        // the workspace the set was made from, as its owner file says
        let canonical_root = config::set_source(&set.dir)
            .ok()
            .flatten()
            .unwrap_or_else(|| config.dir.clone());
        ListedWorktree {
            linked: true,
            // the set's folder as the file system names it, as cwd is
            physical_root: set
                .dir
                .canonicalize()
                .unwrap_or_else(|_| set.dir.clone())
                .to_string_lossy()
                .into_owned(),
            canonical_root: canonical_root.to_string_lossy().into_owned(),
            name: set.name.clone(),
        }
    });
    let cwd = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let root_path = repo_path(&dir, &workspace.root);
    let mut records = vec![ProjectResult::record(
        &root_path,
        ListedProject {
            name: ".".to_string(),
            path: root_path.clone(),
            repo: root_repo,
            tags: Vec::new(),
            is_meta: true,
            error: None,
            cwd: Some(cwd.to_string_lossy().into_owned()),
            worktree,
        },
    )
    .at(&dir)];
    list_records(&tree, &dir, &workspace.root, &mut records);
    Ok(Found { text, records })
}

/// Where the projects of `config` are checked out: in a worktree set, the
/// set's folder (whose config may be the workspace's, when the set holds no
/// checkout of the root); elsewhere the config's own.
fn listed_in(target: &Target, config: &config::Workspace) -> PathBuf {
    match &target.set {
        Some(set) => set.dir.clone(),
        None => config.dir.clone(),
    }
}

/// A record for each of `nodes` (listed in `dir`) and the nodes under it,
/// in the order the tree shows them, named from `root`.
fn list_records(nodes: &[MetaTreeNode], dir: &Path, root: &Path, out: &mut Vec<ProjectResult>) {
    for node in nodes {
        let abs = dir.join(&node.info.path);
        let path = repo_path(&abs, root);
        out.push(
            ProjectResult::record(
                &path,
                ListedProject {
                    name: node.info.name.clone(),
                    path: path.clone(),
                    repo: node.info.repo.clone(),
                    tags: node.info.tags.clone(),
                    is_meta: node.is_meta,
                    error: node.error.clone(),
                    cwd: None,
                    worktree: None,
                },
            )
            .at(&abs),
        );
        list_records(&node.children, &abs, root, out);
    }
}

/// A project's remote URL as shown in `meta project list`.
fn remote_label(repo: Option<&RepoUrl>) -> String {
    repo.map_or_else(|| "no remote".to_string(), RepoUrl::to_string)
}

/// Format a project tree with box-drawing characters: each project's name, its
/// path when that differs from the name, and its remote URL
fn format_project_tree(nodes: &[MetaTreeNode], output: &mut String, prefix: &str) {
    for (i, node) in nodes.iter().enumerate() {
        let info = &node.info;
        let is_last = i + 1 == nodes.len();
        let connector = if is_last {
            "\u{2514}\u{2500}\u{2500} "
        } else {
            "\u{251c}\u{2500}\u{2500} "
        };

        let tags_str = if info.tags.is_empty() {
            String::new()
        } else {
            let tags: Vec<_> = info.tags.iter().map(|t| escape(t)).collect();
            format!(" [{}]", tags.join(", "))
        };

        let path_str = if info.path == info.name {
            String::new()
        } else {
            format!(" \u{2192} {}", escape(&info.path))
        };

        output.push_str(&format!(
            "{}{}{}{} ({}){}\n",
            prefix,
            connector,
            escape(&info.name),
            path_str,
            remote_label(info.repo.as_ref()),
            tags_str
        ));
        if let Some(error) = &node.error {
            output.push_str(&format!("{prefix}    ✗ {}\n", escape(error)));
        }

        if !node.children.is_empty() {
            let child_prefix = if is_last {
                format!("{prefix}    ")
            } else {
                format!("{prefix}\u{2502}   ")
            };
            format_project_tree(&node.children, output, &child_prefix);
        }
    }
}

/// One project `meta project dependents` found.
#[derive(Debug, Serialize)]
struct Dependent<'a> {
    /// As the text output names it: by name in the workspace the command
    /// runs from, by its path from there in a nested one
    name: &'a str,
}

/// `meta project dependents <name>`: the projects that depend directly on
/// `project_name` (via `depends_on` naming the project or one of its
/// `provides` entries), among those of the config the command runs from by
/// name; with meta's -r, among every project in its tree (`depth` levels of
/// it), its own by name and nested ones by their path from it (two nested
/// repos may share a name); with -A, in the tree of the outermost workspace.
fn dependents(target: &Target, project_name: &str) -> Result<Found> {
    fn walk(
        nodes: Vec<MetaTreeNode>,
        prefix: &str,
        dir: &Path,
        out: &mut Vec<ProjectDependencies>,
        dirs: &mut HashMap<String, PathBuf>,
    ) {
        for node in nodes {
            let path = format!("{prefix}/{}", node.info.path);
            let abs = dir.join(&node.info.path);
            let mut project = ProjectDependencies::from(node.info);
            // `depends_on` names it, so its name stands for it as well
            project
                .provides
                .push(std::mem::replace(&mut project.name, path.clone()));
            out.push(project);
            dirs.insert(path.clone(), abs.clone());
            walk(node.children, &path, &abs, out, dirs);
        }
    }
    let (workspace, config) = workspace(target)?;
    let root = &workspace.root;
    let dir = listed_in(target, &config);
    let mut projects = Vec::new();
    let mut dirs = HashMap::new();
    let top = match target.depth {
        Some(0) => config
            .projects
            .iter()
            .cloned()
            .map(|info| (info, Vec::new()))
            .collect::<Vec<_>>(),
        depth => config::walk_projects(&dir, &config.projects, depth)?
            .into_iter()
            .map(|node| (node.info, node.children))
            .collect(),
    };
    for (info, children) in top {
        let abs = dir.join(&info.path);
        walk(children, &info.path, &abs, &mut projects, &mut dirs);
        dirs.insert(info.name.clone(), abs);
        projects.push(info.into());
    }
    let graph = DependencyGraph::build(projects);
    let found = graph
        .dependents_of(project_name)
        .ok_or_else(|| anyhow::anyhow!("Unknown project or alias: {}", escape(project_name)))?;
    let text = match found.is_empty() {
        true => format!("No projects depend on '{}'.", escape(project_name)),
        false => found
            .iter()
            .map(|name| escape(name))
            .collect::<Vec<_>>()
            .join("\n"),
    };
    let records = found
        .iter()
        .map(|name| match dirs.get(*name) {
            Some(dir) => ProjectResult::record(repo_path(dir, root), Dependent { name }).at(dir),
            None => ProjectResult::record(*name, Dependent { name }),
        })
        .collect();
    Ok(Found { text, records })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The repos `meta <options> project …` reaches from `cwd`.
    fn target(cwd: &Path, options: &[&str]) -> Target {
        let line = std::iter::once("meta")
            .chain(options.iter().copied())
            .chain(["project", "list"]);
        invoke::target(&Cli::parse_from(line), cwd).unwrap()
    }

    /// The text `meta <options> project list` shows from `cwd`.
    fn listed(cwd: &Path, options: &[&str]) -> String {
        list(&target(cwd, options), cwd).unwrap().text
    }

    /// The text `meta <options> project dependents <project>` shows from `cwd`.
    fn depending(cwd: &Path, options: &[&str], project: &str) -> Result<Found> {
        dependents(&target(cwd, options), project)
    }

    #[test]
    fn a_word_near_a_project_command_is_suggested_and_one_that_is_not_gets_none() {
        let meta = meta_plugin::args::MetaOptions::default();
        let words = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        let msg = unrecognized(&words(&["lsit", "--help"]), &meta).to_string();
        assert!(msg.contains("meta project list --help\n\n"), "{msg}");
        // gone, not pointed elsewhere: its help lists what there is
        let msg = unrecognized(&words(&["sync"]), &meta).to_string();
        assert!(
            msg.starts_with("unrecognized command 'project sync'\n\n"),
            "{msg}"
        );
        assert!(!msg.contains("similar name"), "{msg}");
        assert!(msg.contains("dependents"), "{msg}");
    }

    #[test]
    fn meta_options_reach_list_check_and_dependents() {
        let sandbox = meta_plugin::testing::isolated!();
        let root = sandbox.work().canonicalize().unwrap();
        std::fs::write(
            root.join(".meta.yaml"),
            "projects:\n  zed: git@x:o/zed.git\n  c:\n    repo: git@x:o/c.git\n    meta: true\n  \
             user:\n    repo: git@x:o/user.git\n    depends_on: [c]\n  alpha: git@x:o/alpha.git\n",
        )
        .unwrap();
        let nested = root.join("c");
        std::fs::create_dir_all(nested.join("g")).unwrap();
        std::fs::write(nested.join(".meta.yaml"), "projects:\n  g: git@x:o/g.git\n").unwrap();

        // every selected project is checked, the nested meta repo's too
        let reached = target(&root, &["-r"]);
        let mut missing: Vec<_> = check_projects(&reached, &root)
            .into_iter()
            .map(|(_, p)| (p.name, p.state))
            .collect();
        missing.sort();
        assert_eq!(
            missing,
            [
                ("alpha", "missing"),
                ("c", "not_a_repo"),
                ("g", "not_a_repo"),
                ("user", "missing"),
                ("zed", "missing"),
            ]
            .map(|(n, s)| (n.to_string(), s))
        );

        // meta's -r and --depth
        assert!(!listed(&root, &[]).contains(" g "));
        assert!(listed(&root, &["-r"]).contains(" g "));
        assert!(!listed(&root, &["-r", "--depth", "0"]).contains(" g "));

        // meta's -A: the root workspace, from inside a nested one; -r
        // goes down from there only
        assert!(depending(&nested, &[], "c").is_err());
        assert!(depending(&nested, &["-r"], "c").is_err());
        let all = depending(&nested, &["-A"], "c").unwrap();
        assert_eq!(all.text, "user");
        assert_eq!(all.records[0].project.as_deref(), Some("user"));
        // a name nothing lists is an error, not "no dependents"
        let unknown = depending(&root, &[], "nonexistent").err().unwrap();
        assert!(unknown.to_string().contains("Unknown project or alias"));
        assert_eq!(
            depending(&root, &[], "alpha").unwrap().text,
            "No projects depend on 'alpha'."
        );
    }

    #[test]
    fn recursive_dependents_cover_nested_projects_by_path() {
        let sandbox = meta_plugin::testing::isolated!();
        let root = sandbox.work().canonicalize().unwrap();
        let config = |dir: &Path, yaml: &str| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(".meta.yaml"), yaml).unwrap();
        };
        config(
            &root,
            "projects:\n  utils:\n    repo: git@x:o/utils.git\n    provides: [utils-api]\n  \
             api:\n    repo: git@x:o/api.git\n    depends_on: [utils-api]\n  \
             infra:\n    repo: git@x:o/infra.git\n    meta: true\n  \
             tools:\n    repo: git@x:o/tools.git\n    meta: true\n",
        );
        config(
            &root.join("infra"),
            "projects:\n  terraform: git@x:o/tf.git\n  \
             k8s:\n    repo: git@x:o/k8s.git\n    depends_on: [terraform]\n",
        );
        config(
            &root.join("tools"),
            "projects:\n  cli:\n    repo: git@x:o/cli.git\n    depends_on: [utils-api]\n",
        );
        let found = |project: &str, cwd: &Path| depending(cwd, &["-A"], project).unwrap();
        // nested projects depend on the root's, and on each other, by name
        assert_eq!(found("utils-api", &root).text, "api\ntools/cli");
        assert_eq!(found("terraform", &root).text, "infra/k8s");
        let from_infra = found("terraform", &root.join("infra"));
        assert_eq!(from_infra.text, "infra/k8s");
        // each record names its project from the workspace root
        let projects: Vec<_> = from_infra
            .records
            .iter()
            .map(|r| (r.project.clone(), r.data.clone()))
            .collect();
        assert_eq!(
            projects,
            [(
                Some("infra/k8s".to_string()),
                Some(serde_json::json!({"name": "infra/k8s"}))
            )]
        );
    }

    #[test]
    fn check_names_a_wrong_remote_and_list_shows_the_roots_from_a_child() {
        let sandbox = meta_plugin::testing::isolated!();
        let root = sandbox.work().canonicalize().unwrap();
        std::fs::write(
            root.join(".meta.yaml"),
            "projects:\n  web:\n    repo: https://example.invalid/web.git\n    path: apps/web\n",
        )
        .unwrap();
        sandbox.git(&root, &["init", "-q"]);
        sandbox.git(
            &root,
            &["remote", "add", "origin", "https://example.invalid/ws.git"],
        );
        let web = root.join("apps/web");
        std::fs::create_dir_all(&web).unwrap();
        sandbox.git(&web, &["init", "-q"]);
        sandbox.git(
            &web,
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/docs.git",
            ],
        );

        // under --json, a failed record for the project
        let cli = Cli::parse_from(["meta", "--json", "project", "check"]);
        let mut report = Report::new("project check");
        check(&cli, &root, &mut report).unwrap();
        let envelope = report.envelope();
        assert_eq!(envelope.results.len(), 1, "{:?}", envelope.results);
        let record = &envelope.results[0];
        assert_eq!(record.project.as_deref(), Some("apps/web"));
        assert_eq!(
            record.error.as_ref().map(|e| e.code),
            Some(ErrorCode::Conflict)
        );
        assert_eq!(record.data.as_ref().unwrap()["state"], "wrong_remote");
        assert_eq!(envelope.error.map(|e| e.code), Some(ErrorCode::Conflict));

        // listed from inside the child, the root is still the workspace's
        let rows = list(&target(&web, &[]), &web).unwrap().records;
        assert_eq!(
            rows[0].data.as_ref().unwrap()["repo"],
            "https://example.invalid/ws.git"
        );
    }

    /// A node of a meta tree.
    fn node(
        name: &str,
        path: &str,
        repo: Option<&str>,
        tags: &[&str],
        error: Option<&str>,
        children: Vec<MetaTreeNode>,
    ) -> MetaTreeNode {
        MetaTreeNode {
            info: config::ProjectInfo {
                name: name.to_string(),
                path: path.to_string(),
                repo: repo.map(Into::into),
                tags: tags.iter().map(|t| t.to_string()).collect(),
                provides: Vec::new(),
                depends_on: Vec::new(),
                meta: false,
            },
            is_meta: false,
            error: error.map(str::to_string),
            children,
        }
    }

    /// Every node has one line of its own, including nested and error nodes.
    fn assert_tree_properties(nodes: &[MetaTreeNode], output: &str) {
        assert!(
            output.chars().all(|c| c == '\n' || !c.is_control()),
            "{output:?}"
        );
        // each level one step further in; the path only where it isn't the name
        fn check<'a>(
            nodes: &[MetaTreeNode],
            depth: usize,
            lines: &mut impl Iterator<Item = &'a str>,
        ) {
            for (i, node) in nodes.iter().enumerate() {
                let info = &node.info;
                let line = lines.next().expect("a line for each node");
                let connector = if i + 1 == nodes.len() { '└' } else { '├' };
                let at = line.chars().position(|c| c == connector);
                assert_eq!(at, Some(depth * 4), "{line}");
                assert_eq!(line.contains('→'), info.path != info.name, "{line}");
                let mut fields = vec![
                    escape(&info.name).into_owned(),
                    escape(&info.path).into_owned(),
                ];
                fields.push(info.repo.as_ref().map_or_else(
                    || "no remote".into(),
                    |r| escape(&r.to_string()).into_owned(),
                ));
                fields.extend(info.tags.iter().map(|t| escape(t).into_owned()));
                for field in fields {
                    assert!(line.contains(&field), "{field:?} missing on {line:?}");
                }
                if let Some(error) = &node.error {
                    let line = lines.next().expect("the node's error line");
                    assert!(line.contains(escape(error).as_ref()), "{line}");
                }
                check(&node.children, depth + 1, lines);
            }
        }
        let mut lines = output.lines();
        check(nodes, 0, &mut lines);
        assert_eq!(lines.next(), None, "extra lines: {output}");
    }

    #[test]
    fn the_project_tree_shows_every_node_on_lines_of_its_own() {
        let nodes = vec![
            node(
                "api",
                "services/api",
                Some("git@github.com:org/api.git"),
                &["backend"],
                None,
                vec![],
            ),
            node(
                "frontend",
                "frontend",
                Some("git@github.com:org/frontend.git"),
                &[],
                None,
                vec![
                    node(
                        "widget",
                        "ui/widget",
                        Some("git@github.com:org/widget.git"),
                        &["ui", "client"],
                        None,
                        vec![],
                    ),
                    node(
                        "broken",
                        "ui/broken",
                        None,
                        &[],
                        Some("cannot read config"),
                        vec![],
                    ),
                ],
            ),
            // control characters from .meta.yaml are shown escaped
            node(
                "x\u{1b}]0;PWNED\u{7}",
                "t\u{1b}[1A",
                Some("u\u{9b}2J"),
                &["n\nfake"],
                None,
                vec![],
            ),
        ];
        let mut output = String::new();
        format_project_tree(&nodes, &mut output, "");
        assert_tree_properties(&nodes, &output);
    }
}
