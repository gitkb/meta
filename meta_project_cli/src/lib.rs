//! meta-project library
//!
//! Provides project management commands for meta repositories.

use indexmap::IndexMap;
use meta_core::config::{self, MetaTreeNode};
use meta_core::dependency_graph::{DependencyGraph, ProjectDependencies};
use meta_core::repo_filter::repo_path;
use meta_core::worktree;
use meta_plugin::display::escape;
use meta_plugin::{
    has_help_token, outln, parse_args, usage, Ctx, ErrorCode, ErrorInfo, PluginHelp, PluginInfo,
    Project, ProjectResult, RepoUrl, Request, Response,
};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What the plugin answers to `--meta-plugin-info`.
pub fn info() -> PluginInfo {
    let mut commands = IndexMap::new();
    commands.insert(
        "list".to_string(),
        "List all projects defined in .meta.yaml (alias: ls)".to_string(),
    );
    commands.insert(
        "check".to_string(),
        "Verify all projects are cloned and consistent".to_string(),
    );
    commands.insert(
        "dependents".to_string(),
        "List projects that depend on a given project".to_string(),
    );
    PluginInfo {
        name: "project".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        commands: vec![
            "project list".to_string(),
            "project ls".to_string(),
            "project check".to_string(),
            "project dependents".to_string(),
        ],
        description: Some("Project inspection for meta repositories".to_string()),
        help: Some(PluginHelp {
            usage: "meta project <command> [args...]".to_string(),
            commands,
            runs_as_typed: None,
            examples: vec![
                "meta project list".to_string(),
                "meta --json project list".to_string(),
                "meta -r project list".to_string(),
                "meta project check".to_string(),
            ],
            note: Some("To clone missing projects, use: meta git clone-missing".to_string()),
        }),
    }
}

// ============================================================================
// Project List Types
// ============================================================================

/// One row of `meta --json project list`: a project, or the workspace the
/// list is of (the first row).
#[derive(Debug, Clone, Serialize)]
pub struct ListedProject {
    pub name: String,
    /// From the workspace root, as the row's `project` (`.` for the root)
    pub path: String,
    /// Git remote URL. Should be present for all normal projects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<RepoUrl>,
    pub tags: Vec<String>,
    pub is_meta: bool,
    /// Why its folder couldn't be looked into (its own projects may be
    /// missing from the list); absent when it could.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The directory the command ran from (the first row only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// The worktree set it ran in (the first row only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<ListedWorktree>,
}

/// Linked worktree context for project list output.
#[derive(Debug, Clone, Serialize)]
pub struct ListedWorktree {
    pub linked: bool,
    pub physical_root: String,
    pub canonical_root: String,
    pub name: String,
}

// ============================================================================
// Command Execution
// ============================================================================

/// `meta project list`
#[derive(clap::Parser)]
#[command(
    about = "Inspect the meta repo graph",
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
struct ListArgs {}

/// `meta project check`
#[derive(clap::Parser)]
#[command(
    about = "Verify configured projects are present (meta --recursive checks nested meta repos as well)",
    after_help = "Agent hints:
  Run this when commands fail because a child checkout is missing.
  Missing repos can usually be cloned with `meta git clone-missing`.

Examples:
  meta project check
  meta --json project check
  meta -r project check

meta's --json emits one record per project that is not a clone of its configured repository."
)]
struct CheckArgs {}

/// `meta project dependents`
#[derive(clap::Parser)]
#[command(
    about = "Find projects that depend on another project",
    after_help = "Agent hints:
  Use this before changing shared libraries or APIs to identify likely blast radius.
  Pair with `meta -r --json project list` when working across nested meta repos.

Examples:
  meta project dependents meta_core
  meta --json project dependents auth
  meta -A project dependents auth

meta's -r also looks in the meta repos nested under the nearest .meta.yaml, -A in the whole \
workspace from its root; --json emits one record per dependent project."
)]
struct DependentsArgs {
    /// Project name or provided capability to search for
    project: String,
}

/// What a command found: its text, and its records for `--json`.
struct Found {
    text: String,
    records: Vec<ProjectResult>,
}

/// Answer a `meta project` call.
pub fn execute(_ctx: &Ctx, request: Request) -> meta_plugin::Result<Response> {
    let (command, args) = (request.command.as_str(), &request.args);
    let meta = &request.meta_options;
    match command {
        "project list" | "project ls" => match parse_args::<ListArgs>(command, args, meta) {
            Ok(ListArgs {}) => shown(&request, list(&request)?),
            Err(response) => Ok(*response),
        },
        "project dependents" => match parse_args::<DependentsArgs>(command, args, meta) {
            Ok(DependentsArgs { project }) => shown(&request, dependents(&request, &project)?),
            Err(response) => Ok(*response),
        },
        "project check" => match parse_args::<CheckArgs>(command, args, meta) {
            Ok(CheckArgs {}) => check(&request),
            Err(response) => Ok(*response),
        },
        // meta_cli sends an unclaimed `project <word>` as `project` plus
        // args: help for `project`, but not for a word it doesn't have
        _ if args.first().is_none_or(|word| word.starts_with('-'))
            && has_help_token(command, args) =>
        {
            Ok(Response::message(PROJECT_HELP))
        }
        _ => {
            let typed: Vec<&str> = std::iter::once(command)
                .chain(args.iter().map(String::as_str))
                .collect();
            let near = args
                .first()
                .and_then(|word| meta_plugin::args::nearest(word, SUBCOMMANDS.iter().copied()));
            // meta's options go before the command: the ones typed there,
            // then the ones typed after it
            let line = near.and_then(|word| {
                let (moved, rest) = meta.lift(args.get(1..).unwrap_or_default())?;
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
            Err(usage(format!(
                "unrecognized command '{}'{suggestion}\n\n{}",
                escape(&typed.join(" ")),
                meta_plugin::help_text("project", &info()).trim_end()
            )))
        }
    }
}

/// `found`'s text shown (but not under `--json`), its records for `--json`.
fn shown(request: &Request, found: Found) -> meta_plugin::Result<Response> {
    if !request.meta.json {
        outln!("{}", found.text)?;
    }
    Ok(Response::printed(found.records))
}

/// The words after `meta project` it answers.
const SUBCOMMANDS: &[&str] = &["list", "ls", "check", "dependents"];

const PROJECT_HELP: &str = "Inspect and validate meta workspace projects

Usage: meta project <COMMAND>

Commands:
  list          List projects defined in .meta.yaml (alias: ls)
  check         Verify configured projects are cloned
  dependents    List projects that depend on a project or capability

Agent hints:
  Prefer `meta -r --json project list` for codebase discovery.
  Use `meta -r project check` before assuming a repo is intentionally absent.

Examples:
  meta -r --json project list
  meta project check
  meta project dependents meta_core";

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
fn check(request: &Request) -> meta_plugin::Result<Response> {
    let problems = check_projects(request)?;
    if problems.is_empty() {
        return Ok(Response::message("All projects are cloned and present."));
    }
    if request.meta.json {
        let records = problems
            .into_iter()
            .map(|(project, problem)| {
                let code = match problem.state {
                    "missing" => ErrorCode::NotFound,
                    "wrong_remote" => ErrorCode::Conflict,
                    _ => ErrorCode::Failed,
                };
                let error = ErrorInfo::new(
                    code,
                    format!("{}: {}", escape(&project.path), escape(&problem.reason)),
                );
                ProjectResult {
                    error: Some(error),
                    ..ProjectResult::record(&project.path, problem).at(&project.abs)
                }
            })
            .collect();
        return Ok(Response::printed(records));
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

/// What is wrong with the selected projects (meta's -r, -A and filters
/// applied), in their order. The workspace's own repo is not one of them.
fn check_projects(request: &Request) -> anyhow::Result<Vec<(&Project, CheckProblem)>> {
    request.workspace()?;
    Ok(request
        .projects
        .iter()
        .filter(|project| project.name != ".")
        .filter_map(|project| Some((project, check_project(project, &request.cwd)?)))
        .collect())
}

/// What is wrong with `project`, if anything.
fn check_project(project: &Project, cwd: &Path) -> Option<CheckProblem> {
    use meta_core::gitdir::Checkout;
    let dir = &project.abs;
    let repo = project.repo.clone().map(RepoUrl::from);
    let problem = |state, reason: String| CheckProblem {
        name: project.name.clone(),
        path: dir
            .strip_prefix(cwd)
            .unwrap_or(dir)
            .to_string_lossy()
            .into_owned(),
        state,
        reason,
        repo: repo.clone(),
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
    let expected = repo.as_ref()?;
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

// ============================================================================
// Project List Implementation
// ============================================================================

/// `meta project list`: the projects of the config the command runs from
/// (with -A, the outermost one's), and with -r those of the meta repos
/// nested in it, as a tree; each a record, in the tree's order.
fn list(request: &Request) -> anyhow::Result<Found> {
    let workspace = request.workspace()?;
    let config = config::Workspace::load(&workspace.config)?;
    let dir = listed_in(request, &config);
    let tree = config::walk_projects(&dir, &config.projects, request.depth)?;

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

    let worktree = request.set.as_ref().map(|set| {
        let canonical_root = worktree::discover_worktree_repos(&set.dir)
            .ok()
            .and_then(|repos| {
                repos
                    .into_iter()
                    .find(|repo| repo.alias == ".")
                    .map(|repo| repo.source_path)
            })
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
    let cwd = request
        .cwd
        .canonicalize()
        .unwrap_or_else(|_| request.cwd.clone());
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
fn listed_in(request: &Request, config: &config::Workspace) -> PathBuf {
    match &request.set {
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

// ============================================================================
// Project Dependents
// ============================================================================

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
fn dependents(request: &Request, project_name: &str) -> anyhow::Result<Found> {
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
    let root = &request.workspace()?.root;
    let workspace = config::Workspace::load(&request.workspace()?.config)?;
    let dir = listed_in(request, &workspace);
    let mut projects = Vec::new();
    let mut dirs = HashMap::new();
    let top = match request.depth {
        Some(0) => workspace
            .projects
            .iter()
            .cloned()
            .map(|info| (info, Vec::new()))
            .collect::<Vec<_>>(),
        depth => config::walk_projects(&dir, &workspace.projects, depth)?
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
    use meta_core::repo_filter::{Filters, Reach};
    use meta_plugin::envelope::info_of;
    use meta_plugin::Answer;

    /// meta's options that reach into the workspace, and --json
    #[derive(Default, Clone, Copy)]
    struct Options {
        recursive: bool,
        all: bool,
        depth: Option<usize>,
        json: bool,
    }

    const RECURSIVE: Options = Options {
        recursive: true,
        all: false,
        depth: None,
        json: false,
    };
    const ALL: Options = Options {
        all: true,
        ..RECURSIVE
    };

    /// The request meta sends for `command args` from `cwd`: the
    /// workspace of the nearest config, and every repo `options` reach.
    fn request(command: &str, args: &[&str], cwd: &Path, options: Options) -> Request {
        let mut request = Request::new(command, cwd);
        request.args = args.iter().map(|a| a.to_string()).collect();
        request.meta.json = options.json;
        let Some(config) = config::Workspace::find(cwd, None).unwrap() else {
            return request;
        };
        let reach = Reach::of(config, options.recursive, options.all, options.depth).unwrap();
        let repos = reach.select(&Filters::default(), String::new).unwrap();
        request.projects = repos
            .iter()
            .map(|repo| Project {
                name: repo.name().unwrap_or(".").to_string(),
                path: repo_path(&repo.dir, &reach.root),
                abs: repo.dir.clone(),
                repo: repo
                    .info
                    .as_ref()
                    .and_then(|i| i.repo.as_ref())
                    .map(ToString::to_string),
                tags: repo.tags().to_vec(),
                present: repo.dir.exists(),
                ..Project::default()
            })
            .collect();
        request.workspace = Some(meta_plugin::Workspace {
            root: reach.root.clone(),
            config: reach.workspace.config_path.clone(),
            listed_root: config::listed_root(&reach.workspace.dir),
            members: request.projects.clone(),
            worktree: Default::default(),
        });
        request.depth = match options.recursive || options.all {
            true => options.depth,
            false => Some(0),
        };
        request
    }

    fn run(request: Request) -> Response {
        execute(&Ctx::new(request.meta.clone()), request)
            .unwrap_or_else(|e| Response::error(info_of(&e)))
    }

    /// The words of a `Message` answer.
    fn message(response: Response) -> Option<String> {
        match response.answer {
            Answer::Message(msg) => Some(msg),
            _ => None,
        }
    }

    /// The message of an `Error` answer.
    fn error(response: Response) -> Option<String> {
        match response.answer {
            Answer::Error(error) => Some(error.message),
            _ => None,
        }
    }

    /// The text `meta project list` shows.
    fn listed(cwd: &Path, options: Options) -> String {
        list(&request("project list", &[], cwd, options))
            .unwrap()
            .text
    }

    /// The rows of `meta --json project list`: each record's `project` and
    /// `data`.
    fn rows(cwd: &Path, options: Options) -> Vec<(String, serde_json::Value)> {
        let response = run(request(
            "project list",
            &[],
            cwd,
            Options {
                json: true,
                ..options
            },
        ));
        records(response)
            .into_iter()
            .map(|r| (r.project.unwrap(), r.data.unwrap()))
            .collect()
    }

    /// The records of a `Results` answer (none for any other).
    fn records(response: Response) -> Vec<ProjectResult> {
        match response.answer {
            Answer::Results(results) => results.records,
            _ => Vec::new(),
        }
    }

    fn comparable_path(path: &Path) -> String {
        comparable_path_str(&path.to_string_lossy())
    }

    fn comparable_path_str(path: &str) -> String {
        path.trim_start_matches("\\\\?\\").replace('\\', "/")
    }

    #[test]
    fn test_execute_command_no_meta_file() {
        let sandbox = meta_plugin::testing::Sandbox::new();
        let temp_dir = sandbox.work();
        for command in ["project check", "project list"] {
            let msg = error(run(Request::new(command, &temp_dir))).unwrap();
            assert!(msg.contains("no .meta.yaml"), "{msg}");
        }
    }

    #[test]
    fn test_unknown_command() {
        let sandbox = meta_plugin::testing::Sandbox::new();
        let temp_dir = sandbox.work();
        let msg = error(run(request(
            "project",
            &["unknown"],
            &temp_dir,
            Options::default(),
        )));
        assert!(msg
            .unwrap()
            .contains("unrecognized command 'project unknown'"));
    }

    #[test]
    fn a_typo_of_a_project_command_names_the_command_and_gets_no_help() {
        let sandbox = meta_plugin::testing::Sandbox::new();
        let temp_dir = sandbox.work();
        let typo = request(
            "project",
            &["lsit", "--json"],
            &temp_dir,
            Options::default(),
        );
        let msg = error(run(typo)).unwrap();
        assert!(msg.contains("meta project list --json\n\n"), "{msg}");
        let asked = request(
            "project",
            &["lsit", "--help"],
            &temp_dir,
            Options::default(),
        );
        let msg = error(run(asked)).unwrap();
        assert!(msg.contains("unrecognized command"), "{msg}");
    }

    #[test]
    fn test_project_check_all_present() {
        let sandbox = meta_plugin::testing::Sandbox::new();
        let temp_dir = sandbox.work();
        std::fs::write(temp_dir.join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();
        let checked = run(request("project check", &[], &temp_dir, Options::default()));
        assert!(message(checked)
            .unwrap()
            .contains("All projects are cloned"));
    }

    #[test]
    fn meta_options_reach_list_and_dependents() {
        let sandbox = meta_plugin::testing::isolated!();
        let temp_dir = sandbox.work();
        let root = temp_dir.canonicalize().unwrap();
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
        let checking = request("project check", &[], &root, RECURSIVE);
        let mut missing: Vec<_> = check_projects(&checking)
            .unwrap()
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
        let extra = request("project check", &["x"], &root, Options::default());
        assert!(error(run(extra)).unwrap().contains("unexpected argument"));

        // meta's -r and --depth
        let depth = |depth| Options { depth, ..RECURSIVE };
        assert!(!listed(&root, Options::default()).contains(" g "));
        assert!(listed(&root, depth(None)).contains(" g "));
        assert!(!listed(&root, depth(Some(0))).contains(" g "));
        // they are meta's: after the command, a usage error
        let after = request("project list", &["-r"], &root, Options::default());
        assert!(error(run(after)).is_some());

        // meta's -A: the root workspace, from inside a nested one; -r
        // goes down from there only
        let from_nested = |options| run(request("project dependents", &["c"], &nested, options));
        assert!(error(from_nested(Options::default())).is_some());
        assert!(error(from_nested(RECURSIVE)).is_some());
        let all = dependents(&request("project dependents", &["c"], &nested, ALL), "c").unwrap();
        assert_eq!(all.text, "user");
        assert_eq!(all.records[0].project.as_deref(), Some("user"));
    }

    #[test]
    fn recursive_dependents_cover_nested_projects_by_path() {
        let sandbox = meta_plugin::testing::isolated!();
        let temp_dir = sandbox.work();
        let root = temp_dir.canonicalize().unwrap();
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
        let found = |project: &str, cwd: &Path| {
            dependents(
                &request("project dependents", &[project], cwd, ALL),
                project,
            )
            .unwrap()
        };
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
        let temp_dir = sandbox.work();
        let root = temp_dir.canonicalize().unwrap();
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

        let checking = request("project check", &[], &root, Options::default());
        let problems = check_projects(&checking).unwrap();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(
            (problems[0].1.name.as_str(), problems[0].1.state),
            ("web", "wrong_remote")
        );
        let msg = error(run(request(
            "project check",
            &[],
            &root,
            Options::default(),
        )));
        assert!(msg.unwrap().contains("1 project(s)"));
        // under --json, a failed record for the project
        let json = Options {
            json: true,
            ..Options::default()
        };
        let failed = records(run(request("project check", &[], &root, json)));
        assert_eq!(failed.len(), 1, "{failed:?}");
        let record = &failed[0];
        assert_eq!(record.project.as_deref(), Some("apps/web"));
        assert_eq!(
            record.error.as_ref().map(|e| e.code),
            Some(ErrorCode::Conflict)
        );
        assert_eq!(record.data.as_ref().unwrap()["state"], "wrong_remote");

        // listed from inside the child, the root is still the workspace's
        let rows = rows(&web, Options::default());
        assert_eq!(rows[0].1["repo"], "https://example.invalid/ws.git");
    }

    #[test]
    fn test_project_sync_removed() {
        let sandbox = meta_plugin::testing::Sandbox::new();
        let sync = request("project", &["sync"], &sandbox.work(), Options::default());
        // gone, not pointed elsewhere: its help lists what there is
        let msg = error(run(sync)).unwrap();
        assert!(
            msg.starts_with("unrecognized command 'project sync'\n\n"),
            "{msg}"
        );
        assert!(!msg.contains("similar name"), "{msg}");
    }

    #[test]
    fn test_project_list_basic() {
        let sandbox = meta_plugin::testing::isolated!();
        let temp_dir = sandbox.work();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git", "repo2": "git@github.com:org/repo2.git"}}"#,
        )
        .unwrap();
        let msg = listed(&temp_dir, Options::default());
        assert!(msg.contains("repo1"));
        assert!(msg.contains("repo2"));
        assert!(msg.starts_with(". (")); // root indicator
    }

    #[test]
    fn test_project_list_json() {
        let sandbox = meta_plugin::testing::isolated!();
        let temp_dir = sandbox.work();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git"}}"#,
        )
        .unwrap();
        let rows = rows(&temp_dir, Options::default());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, ".");
        assert_eq!(rows[0].1["path"], ".");
        assert_eq!(rows[1].0, "repo1");
        assert_eq!(rows[1].1["name"], "repo1");
        assert_eq!(rows[1].1["path"], "repo1");
        assert_eq!(rows[1].1["repo"], "git@github.com:org/repo1.git");
    }

    #[test]
    fn test_project_ls_alias() {
        let sandbox = meta_plugin::testing::isolated!();
        let temp_dir = sandbox.work();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git"}}"#,
        )
        .unwrap();
        let json = Options {
            json: true,
            ..Options::default()
        };
        let ls = run(request("project ls", &[], &temp_dir, json));
        assert!(
            matches!(&ls.answer, Answer::Results(r) if r.records.len() == 2),
            "{ls:?}"
        );
    }

    #[test]
    fn test_project_list_recursive() {
        let sandbox = meta_plugin::testing::isolated!();
        let temp_dir = sandbox.work();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"child": "git@github.com:org/child.git"}}"#,
        )
        .unwrap();
        let child_dir = temp_dir.join("child");
        std::fs::create_dir(&child_dir).unwrap();
        std::fs::write(
            child_dir.join(".meta.yaml"),
            r#"{"projects": {"grandchild": "git@github.com:org/grandchild.git"}}"#,
        )
        .unwrap();

        // flat, every level, named from the root
        let rows = rows(&temp_dir, RECURSIVE);
        let paths: Vec<_> = rows.iter().map(|(p, d)| (p.as_str(), &d["path"])).collect();
        assert_eq!(
            paths,
            [
                (".", &".".into()),
                ("child", &"child".into()),
                ("child/grandchild", &"child/grandchild".into())
            ]
        );
        assert_eq!(rows[1].1["is_meta"], true);
        assert_eq!(rows[2].1["name"], "grandchild");
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
    fn test_format_project_tree() {
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
        ];
        let mut output = String::new();
        format_project_tree(&nodes, &mut output, "");
        assert_tree_properties(&nodes, &output);
    }

    #[test]
    fn test_format_project_tree_escapes_control_characters() {
        let nodes = vec![node(
            "x\u{1b}]0;PWNED\u{7}",
            "t\u{1b}[1A",
            Some("u\u{9b}2J"),
            &["n\nfake"],
            None,
            vec![],
        )];
        let mut output = String::new();
        format_project_tree(&nodes, &mut output, "");
        assert_tree_properties(&nodes, &output);
    }

    #[test]
    fn test_format_project_tree_without_remote() {
        let nodes = vec![node("scratch", "scratch", None, &[], None, vec![])];
        let mut output = String::new();
        format_project_tree(&nodes, &mut output, "");
        assert_tree_properties(&nodes, &output);
    }

    #[test]
    fn test_project_list_json_includes_cwd() {
        let sandbox = meta_plugin::testing::isolated!();
        let temp_dir = sandbox.work();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git"}}"#,
        )
        .unwrap();
        let rows = rows(&temp_dir, Options::default());
        let cwd = rows[0].1["cwd"]
            .as_str()
            .expect("cwd field should be a string");
        assert!(
            Path::new(cwd).is_absolute(),
            "cwd should be absolute, got: {cwd}"
        );
        assert!(rows[1].1.get("cwd").is_none());
    }

    #[test]
    fn test_project_list_json_includes_linked_worktree_context() {
        let sandbox = meta_plugin::testing::isolated!();
        let temp_dir = sandbox.work();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git"}}"#,
        )
        .unwrap();

        sandbox.git(&temp_dir, &["init", "-q"]);
        sandbox.git(
            &temp_dir,
            &["remote", "add", "origin", "git@github.com:org/root.git"],
        );
        sandbox.git(&temp_dir, &["add", ".meta.yaml"]);
        sandbox.git(&temp_dir, &["commit", "-q", "-m", "initial meta config"]);

        let worktrees = temp_dir.join(".worktrees");
        std::fs::create_dir_all(&worktrees).unwrap();
        let linked = worktrees.join("feature");
        sandbox.git(
            &temp_dir,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feature",
                linked.to_str().unwrap(),
            ],
        );

        // meta sends the set it runs in
        let mut listing = request(
            "project list",
            &[],
            &linked,
            Options {
                json: true,
                ..Options::default()
            },
        );
        listing.set = Some(meta_plugin::Set {
            name: "feature".to_string(),
            dir: linked.clone(),
            members: Vec::new(),
        });
        let root = &records(run(listing))[0];
        let data = root.data.as_ref().unwrap();
        let expected_root = comparable_path(&temp_dir.canonicalize().unwrap());
        let expected_linked = comparable_path(&linked.canonicalize().unwrap());
        let worktree = data["worktree"]
            .as_object()
            .expect("worktree field should be present");
        assert_eq!(worktree["linked"], true);
        assert_eq!(
            worktree["physical_root"].as_str().map(comparable_path_str),
            Some(expected_linked.clone())
        );
        assert_eq!(
            worktree["canonical_root"].as_str().map(comparable_path_str),
            Some(expected_root)
        );
        assert_eq!(worktree["name"].as_str(), Some("feature"));
        assert_eq!(
            data["cwd"].as_str().map(comparable_path_str),
            Some(expected_linked)
        );
    }

    #[test]
    fn test_project_list_recursive_from_nested() {
        // root has child, child has grandchild: -A from child lists the
        // root's tree, and the root row's folder is the root's, not cwd
        let sandbox = meta_plugin::testing::isolated!();
        let temp_dir = sandbox.work();
        let child = temp_dir.join("child");
        let grandchild = child.join("grandchild");
        std::fs::create_dir_all(&grandchild).unwrap();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"child": {"repo": "git@github.com:org/child.git", "meta": true}}}"#,
        )
        .unwrap();
        std::fs::write(
            child.join(".meta.yaml"),
            r#"{"projects": {"grandchild": "git@github.com:org/grandchild.git"}}"#,
        )
        .unwrap();

        let all = run(request(
            "project list",
            &[],
            &child,
            Options { json: true, ..ALL },
        ));
        let results = records(all);
        let rows: Vec<_> = results
            .iter()
            .map(|r| r.project.as_deref().unwrap())
            .collect();
        assert_eq!(rows, [".", "child", "child/grandchild"]);
        let root = &results[0];
        assert_eq!(
            root.path.as_deref().map(comparable_path_str),
            Some(comparable_path(&temp_dir))
        );
        assert_ne!(
            root.path,
            root.data.as_ref().unwrap()["cwd"]
                .as_str()
                .map(String::from)
        );

        // without -r or -A, from child: child's projects only, named from it
        let local = rows_of(&child);
        assert_eq!(local, [".", "grandchild"]);
    }

    /// The `project` of each row of `meta --json project list` from `cwd`.
    fn rows_of(cwd: &Path) -> Vec<String> {
        rows(cwd, Options::default())
            .into_iter()
            .map(|(project, _)| project)
            .collect()
    }

    #[test]
    fn test_project_dependents_command() {
        let sandbox = meta_plugin::testing::isolated!();
        let temp_dir = sandbox.work();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"
projects:
  loop_lib:
    repo: git@github.com:org/loop_lib.git
    provides: [loop-lib]
  meta_cli:
    repo: git@github.com:org/meta_cli.git
    depends_on: [loop-lib]
  other:
    repo: git@github.com:org/other.git
"#,
        )
        .unwrap();
        // from the root, and from a subdirectory: the nearest .meta.yaml
        let sub_dir = temp_dir.join("loop_lib");
        std::fs::create_dir(&sub_dir).unwrap();
        for cwd in [&temp_dir, &sub_dir] {
            let asked = request("project dependents", &["loop_lib"], cwd, Options::default());
            assert_eq!(dependents(&asked, "loop_lib").unwrap().text, "meta_cli");
        }
        let asked = request(
            "project dependents",
            &["other"],
            &temp_dir,
            Options::default(),
        );
        assert_eq!(
            dependents(&asked, "other").unwrap().text,
            "No projects depend on 'other'."
        );
    }

    #[test]
    fn test_project_dependents_json() {
        let sandbox = meta_plugin::testing::isolated!();
        let temp_dir = sandbox.work();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"
projects:
  a:
    repo: git@github.com:org/a.git
    provides: [lib-a]
  b:
    repo: git@github.com:org/b.git
    depends_on: [lib-a]
  c:
    repo: git@github.com:org/c.git
    depends_on: [lib-a]
"#,
        )
        .unwrap();
        let json = Options {
            json: true,
            ..Options::default()
        };
        let results = records(run(request("project dependents", &["a"], &temp_dir, json)));
        let projects: Vec<_> = results
            .iter()
            .map(|r| r.project.as_deref().unwrap())
            .collect();
        assert_eq!(projects, ["b", "c"]);
        // meta's --json after the command is a usage error
        let after = run(request(
            "project dependents",
            &["a", "--json"],
            &temp_dir,
            Options::default(),
        ));
        assert!(matches!(after.answer, Answer::Error(e) if e.code == ErrorCode::Usage));
    }

    #[test]
    fn test_project_dependents_rejects_what_it_does_not_take() {
        let sandbox = meta_plugin::testing::Sandbox::new();
        let temp_dir = sandbox.work();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            "projects:\n  a:\n    repo: git@github.com:org/a.git\n",
        )
        .unwrap();
        for args in [
            &["a", "b"][..],
            &["--depth", "1", "a"],
            &["a", "--bogus\x1b[2J"],
        ] {
            let asked = request("project dependents", args, &temp_dir, Options::default());
            let msg = error(run(asked)).unwrap();
            assert!(msg.contains("unexpected argument"), "{args:?}: {msg}");
            assert!(!msg.contains('\x1b'), "{msg}");
        }
    }

    #[test]
    fn test_project_dependents_unknown_project_error() {
        // querying a nonexistent project name should error, not say "no dependents"
        let sandbox = meta_plugin::testing::Sandbox::new();
        let temp_dir = sandbox.work();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            "projects:\n  a:\n    repo: git@github.com:org/a.git\n",
        )
        .unwrap();
        let asked = request(
            "project dependents",
            &["nonexistent"],
            &temp_dir,
            Options::default(),
        );
        let msg = error(run(asked)).unwrap();
        assert!(msg.contains("Unknown project or alias"), "{msg}");
    }
}
