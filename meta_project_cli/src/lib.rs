//! meta-project library
//!
//! Provides project management commands for meta repositories.

use meta_cli::dependency_graph::{DependencyGraph, ProjectDependencies};
use meta_cli::worktree;
use meta_core::config::{self, MetaTreeNode};
use meta_core::display::{escape, escape_path};
use meta_core::git::GitUrl;
use meta_core::outln;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use meta_plugin_protocol::{has_help_token, parse_args, CommandResult};

/// Options passed to execute_command
#[derive(Debug, Default, Clone)]
pub struct ExecuteOptions {
    pub json_output: bool,
    pub recursive: bool,
    pub depth: Option<usize>,
    /// The project directories meta's `--include`, `--exclude` or `--tag`
    /// chose, when one was given: the commands act on these only
    pub selected: Option<Vec<PathBuf>>,
}

impl ExecuteOptions {
    /// `selected` from the directories meta sent
    pub fn selection_from(projects: &[String]) -> Vec<PathBuf> {
        projects
            .iter()
            .map(|dir| comparable(Path::new(dir)))
            .collect()
    }

    /// Whether the command acts on the project at `dir`
    fn acts_on(&self, dir: &Path) -> bool {
        self.selected
            .as_ref()
            .is_none_or(|selected| selected.contains(&comparable(dir)))
    }
}

/// `dir` resolved, so two spellings of one directory compare equal; for a
/// project not cloned yet, its parent resolved.
fn comparable(dir: &Path) -> PathBuf {
    dir.canonicalize().unwrap_or_else(|_| {
        match (
            dir.parent().and_then(|p| p.canonicalize().ok()),
            dir.file_name(),
        ) {
            (Some(parent), Some(name)) => parent.join(name),
            _ => dir.to_path_buf(),
        }
    })
}

// ============================================================================
// Project List Types
// ============================================================================

/// A project node in the hierarchical tree
#[derive(Debug, Clone, Serialize)]
pub struct ProjectTreeNode {
    pub name: String,
    pub path: String,
    /// Git remote URL. Should be present for all normal projects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<GitUrl>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub is_meta: bool,
    /// Why its folder couldn't be looked into (its own projects may be
    /// missing from `projects`); absent when it could.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<ProjectTreeNode>,
}

/// Top-level output for `meta project list --json`
#[derive(Debug, Clone, Serialize)]
pub struct ProjectListOutput {
    pub path: String,
    /// The root's remote, `""` when it has none.
    pub repo: GitUrl,
    pub root: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<ProjectListWorktree>,
    pub projects: Vec<ProjectTreeNode>,
}

/// Linked worktree context for project list output.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectListWorktree {
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
    after_help = "Agent hints:
  Use `meta project list --recursive --json` before broad changes to discover the full repo graph.
  Use non-recursive output for a quick human-readable view of the current meta repo.

Examples:
  meta project list
  meta project list --recursive
  meta project list --recursive --json"
)]
struct ListArgs {
    /// Emit a structured tree with root, cwd, project paths, repo URLs, tags, and nested projects
    #[arg(long)]
    json: bool,
    /// Include projects from nested meta repos
    #[arg(short, long)]
    recursive: bool,
    /// Limit recursive traversal depth
    #[arg(long, value_name = "N")]
    depth: Option<usize>,
}

/// `meta project check`
#[derive(clap::Parser)]
#[command(
    about = "Verify configured projects are present (meta --recursive checks nested meta repos as well)",
    after_help = "Agent hints:
  Run this when commands fail because a child checkout is missing.
  Missing repos can usually be cloned with `meta git update`.

Examples:
  meta project check
  meta project check --json
  meta --recursive project check"
)]
struct CheckArgs {
    /// Emit one record per project that is not a clone of its configured
    /// repository
    #[arg(long)]
    json: bool,
}

/// `meta project dependents`
#[derive(clap::Parser)]
#[command(
    about = "Find projects that depend on another project",
    after_help = "Agent hints:
  Use this before changing shared libraries or APIs to identify likely blast radius.
  Pair with `meta project list --recursive --json` when working across nested meta repos.

Examples:
  meta project dependents meta_core
  meta project dependents auth --json"
)]
struct DependentsArgs {
    /// Project name or provided capability to search for
    project: String,
    /// Emit machine-readable dependent project records
    #[arg(long)]
    json: bool,
    /// Look in the root workspace of nested meta repos
    #[arg(short, long)]
    recursive: bool,
}

/// Execute a project command and return the result
///
/// If `provided_projects` is not empty, it will be used instead of reading from .meta.yaml file.
/// This allows meta_cli to pass in the full project list when --recursive is used.
pub fn execute_command(
    command: &str,
    args: &[String],
    options: &ExecuteOptions,
    provided_projects: &[String],
    cwd: &Path,
) -> CommandResult {
    match command {
        // These read the workspace tree themselves, with their own
        // --recursive, so meta's selection isn't theirs to apply
        "project list" | "project ls" | "project dependents" if options.selected.is_some() => {
            CommandResult::Error(format!(
                "meta {command} does not take --include/--exclude/--tag; it reads the .meta.yaml \
                 tree itself"
            ))
        }
        "project list" | "project ls" => match parse_args::<ListArgs>(command, args) {
            Ok(list) => handle_project_list(
                cwd,
                &ExecuteOptions {
                    json_output: meta_core::out::set_json(options.json_output || list.json),
                    recursive: options.recursive || list.recursive,
                    depth: list.depth.or(options.depth),
                    selected: None,
                },
            ),
            Err(result) => result,
        },
        "project dependents" => match parse_args::<DependentsArgs>(command, args) {
            Ok(dependents) => handle_project_dependents(
                &dependents.project,
                meta_core::out::set_json(options.json_output || dependents.json),
                options.recursive || dependents.recursive,
                cwd,
            ),
            Err(result) => result,
        },
        "project check" => match parse_args::<CheckArgs>(command, args) {
            Ok(CheckArgs { json }) => check(
                meta_core::out::set_json(options.json_output || json),
                options,
                provided_projects,
                cwd,
            )
            .unwrap_or_else(Into::into),
            Err(result) => result,
        },
        // meta_cli sends an unclaimed `project <word>` as `project` plus args
        _ if has_help_token(command, args) => CommandResult::Message(PROJECT_HELP.to_string()),
        _ => {
            let typed: Vec<&str> = std::iter::once(command)
                .chain(args.iter().map(String::as_str))
                .collect();
            CommandResult::ShowHelp(Some(format!(
                "unrecognized command '{}'. Use 'meta git update' to sync projects.",
                escape(&typed.join(" "))
            )))
        }
    }
}

const PROJECT_HELP: &str = "meta project - Inspect and validate meta workspace projects

Usage: meta project <COMMAND>

Commands:
  list          List projects defined in .meta.yaml (alias: ls)
  check         Verify configured projects are cloned
  dependents    List projects that depend on a project or capability

Agent hints:
  Prefer `meta project list --recursive --json` for codebase discovery.
  Use `meta --recursive project check` before assuming a repo is intentionally absent.

Examples:
  meta project list --recursive --json
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
    repo: Option<GitUrl>,
    /// The checkout's `origin`, for `wrong_remote` (`null` without one).
    #[serde(skip_serializing_if = "Option::is_none")]
    actual: Option<Option<GitUrl>>,
}

/// `meta project check`: every project of the workspace is a clone of its
/// configured repository; each that isn't is named, and the command fails.
fn check(
    json: bool,
    options: &ExecuteOptions,
    provided_projects: &[String],
    cwd: &Path,
) -> anyhow::Result<CommandResult> {
    let problems = check_projects(options, provided_projects, cwd)?;
    if json {
        let report = serde_json::to_string_pretty(&serde_json::json!({ "problems": problems }))
            .map_err(|e| anyhow::anyhow!("Failed to serialize JSON: {e}"))?;
        if problems.is_empty() {
            return Ok(CommandResult::Message(report));
        }
        // the report on stdout, the failure in the exit code
        outln!("{report}")?;
    } else if problems.is_empty() {
        return Ok(CommandResult::Message(
            "All projects are cloned and present.".to_string(),
        ));
    } else {
        for problem in &problems {
            outln!("✗ {}: {}", escape(&problem.path), escape(&problem.reason))?;
        }
    }
    let missing = problems.iter().any(|p| p.state == "missing");
    Ok(CommandResult::Error(format!(
        "{} project(s) are not clones of their configured repository{}",
        problems.len(),
        if missing {
            "; run 'meta git update' to clone the missing ones"
        } else {
            ""
        }
    )))
}

/// What is wrong with the projects of the nearest config at or above cwd;
/// with meta's --recursive, of cwd's config and of each meta repo meta_cli
/// sent; with meta's filters, of the projects they selected. A config that
/// can't be read is an error, not "nothing wrong".
fn check_projects(
    options: &ExecuteOptions,
    provided_projects: &[String],
    cwd: &Path,
) -> anyhow::Result<Vec<CheckProblem>> {
    if options.selected.is_some() {
        return check_selected(options, cwd);
    }
    let mut workspaces = Vec::new();
    if !options.recursive || provided_projects.is_empty() {
        workspaces.push(
            config::Workspace::find(cwd, None)?
                .ok_or_else(|| anyhow::anyhow!("No .meta.yaml found in {}", escape_path(cwd)))?,
        );
    } else {
        let mut seen = HashSet::new();
        for dir in
            std::iter::once(cwd.to_path_buf()).chain(provided_projects.iter().map(|p| cwd.join(p)))
        {
            let key = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            if seen.insert(key) {
                workspaces.extend(config::Workspace::in_dir(&dir)?);
            }
        }
    }
    Ok(workspaces
        .iter()
        .flat_map(|workspace| {
            workspace
                .projects
                .iter()
                .filter_map(|project| check_project(&workspace.dir, project, cwd))
        })
        .collect())
}

/// What is wrong with `project`, if anything.
fn check_project(
    config_dir: &Path,
    project: &config::ProjectInfo,
    cwd: &Path,
) -> Option<CheckProblem> {
    use meta_core::gitdir::Checkout;
    let dir = config_dir.join(&project.path);
    let problem = |state, reason: String| CheckProblem {
        name: project.name.clone(),
        path: dir
            .strip_prefix(cwd)
            .unwrap_or(&dir)
            .to_string_lossy()
            .into_owned(),
        state,
        reason,
        repo: project.repo.clone(),
        actual: None,
    };
    let checkout = Checkout::of(&dir);
    if let Some(why) = checkout.problem() {
        let state = match checkout {
            Checkout::Missing => "missing",
            _ => "not_a_repo",
        };
        return Some(problem(state, why));
    }
    let expected = project.repo.as_ref()?;
    match meta_core::git::remote_url(&dir) {
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

/// What is wrong with the projects meta's filters selected: the nearest
/// workspace's, with meta's --recursive its nested ones too, in project order.
fn check_selected(options: &ExecuteOptions, cwd: &Path) -> anyhow::Result<Vec<CheckProblem>> {
    fn walk(
        nodes: &[MetaTreeNode],
        dir: &Path,
        options: &ExecuteOptions,
        cwd: &Path,
        out: &mut Vec<CheckProblem>,
    ) {
        for node in nodes {
            let path = dir.join(&node.info.path);
            if options.acts_on(&path) {
                out.extend(check_project(dir, &node.info, cwd));
            }
            walk(&node.children, &path, options, cwd, out);
        }
    }
    let workspace = config::Workspace::find(cwd, None)?
        .ok_or_else(|| anyhow::anyhow!("No .meta.yaml found in {}", escape_path(cwd)))?;
    let depth = if options.recursive {
        options.depth
    } else {
        Some(0)
    };
    let tree = config::walk_projects(&workspace.dir, &workspace.projects, depth)?;
    let mut problems = Vec::new();
    walk(&tree, &workspace.dir, options, cwd, &mut problems);
    Ok(problems)
}

// ============================================================================
// Project List Implementation
// ============================================================================

/// Handle `meta project list` / `meta project ls`
fn handle_project_list(cwd: &Path, options: &ExecuteOptions) -> CommandResult {
    let max_depth = if options.recursive {
        options.depth
    } else {
        Some(0)
    };

    // When recursive, walk up to the root ancestor meta-repo
    let start_dir = if options.recursive {
        match config::find_meta_config(cwd, None) {
            Ok(Some(config_path)) => {
                let nearest_meta_dir = config_path
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .to_path_buf();
                match config::find_root_meta_dir(&nearest_meta_dir) {
                    Ok(root) => root,
                    Err(e) => return CommandResult::from(e),
                }
            }
            Err(e) => return CommandResult::from(e),
            Ok(None) => {
                return CommandResult::Error(format!("No .meta.yaml found in {}", escape_path(cwd)))
            }
        }
    } else {
        cwd.to_path_buf()
    };

    let tree = match config::walk_meta_tree(&start_dir, max_depth) {
        Ok(t) => t,
        Err(e) => return CommandResult::from(e),
    };

    // The remote of the workspace whose projects these are, not of the
    // repository the command runs in (a project's, from inside it); none
    // when the workspace is no repository
    let root_repo = match config::find_meta_config(&start_dir, None) {
        Ok(found) => found
            .and_then(|config| config.parent().map(Path::to_path_buf))
            .filter(|dir| matches!(meta_core::gitdir::common_dir(dir), Ok(Some(_))))
            .and_then(|dir| meta_core::git::remote_url(&dir).ok().flatten()),
        Err(e) => return CommandResult::from(e),
    };
    let project_nodes: Vec<ProjectTreeNode> = tree.iter().map(to_project_tree_node).collect();
    let abs_cwd = cwd
        .canonicalize()
        .unwrap_or_else(|_| cwd.to_path_buf())
        .to_path_buf();
    let abs_cwd_string = abs_cwd.to_string_lossy().to_string();

    if options.json_output {
        // `root` = the .meta.yaml directory relevant to this invocation:
        //   - recursive: the outermost workspace root (same as start_dir)
        //   - non-recursive: the nearest ancestor .meta.yaml dir
        // In both cases start_dir is wrong for non-recursive (it equals cwd).
        let root_dir = if options.recursive {
            start_dir.clone()
        } else {
            match config::find_meta_config(cwd, None) {
                Ok(Some(config_path)) => config_path.parent().unwrap_or(cwd).to_path_buf(),
                Ok(None) => start_dir.clone(),
                Err(e) => return CommandResult::from(e),
            }
        };
        let abs_root = root_dir
            .canonicalize()
            .unwrap_or(root_dir)
            .to_string_lossy()
            .to_string();
        let context = match worktree::detect_worktree_context(&abs_cwd) {
            Ok(context) => context,
            Err(e) => return CommandResult::from(e),
        };
        let linked_worktree = context.map(|(name, task_dir, _repos)| {
            let canonical_root = worktree::discover_worktree_repos(&task_dir)
                .ok()
                .and_then(|repos| {
                    repos
                        .into_iter()
                        .find(|repo| repo.alias == ".")
                        .map(|repo| repo.source_path)
                })
                .unwrap_or_else(|| Path::new(&abs_root).to_path_buf());

            ProjectListWorktree {
                linked: true,
                physical_root: task_dir.to_string_lossy().to_string(),
                canonical_root: canonical_root.to_string_lossy().to_string(),
                name,
            }
        });
        let abs_root = linked_worktree
            .as_ref()
            .map(|wt| wt.canonical_root.clone())
            .unwrap_or(abs_root);
        let output = ProjectListOutput {
            path: ".".to_string(),
            repo: root_repo.unwrap_or_default(),
            root: abs_root,
            cwd: abs_cwd_string,
            worktree: linked_worktree,
            projects: project_nodes,
        };
        let json = match serde_json::to_string_pretty(&output) {
            Ok(j) => j,
            Err(e) => return CommandResult::Error(format!("Failed to serialize JSON: {e}")),
        };
        CommandResult::Message(json)
    } else {
        let mut output = String::new();
        output.push_str(&format!(". ({})\n", remote_label(root_repo.as_ref())));
        format_project_tree(&project_nodes, &mut output, "");
        if output.ends_with('\n') {
            output.pop();
        }
        CommandResult::Message(output)
    }
}

fn to_project_tree_node(node: &MetaTreeNode) -> ProjectTreeNode {
    ProjectTreeNode {
        name: node.info.name.clone(),
        path: node.info.path.clone(),
        repo: node.info.repo.clone(),
        tags: node.info.tags.clone(),
        is_meta: node.is_meta,
        error: node.error.clone(),
        projects: node.children.iter().map(to_project_tree_node).collect(),
    }
}

/// A project's remote URL as shown in `meta project list`.
fn remote_label(repo: Option<&GitUrl>) -> String {
    repo.map_or_else(|| "no remote".to_string(), GitUrl::to_string)
}

/// Format a project tree with box-drawing characters: each project's name, its
/// path when that differs from the name, and its remote URL
fn format_project_tree(nodes: &[ProjectTreeNode], output: &mut String, prefix: &str) {
    for (i, node) in nodes.iter().enumerate() {
        let is_last = i == nodes.len() - 1;
        let connector = if is_last {
            "\u{2514}\u{2500}\u{2500} "
        } else {
            "\u{251c}\u{2500}\u{2500} "
        };

        let tags_str = if node.tags.is_empty() {
            String::new()
        } else {
            let tags: Vec<_> = node.tags.iter().map(|t| escape(t)).collect();
            format!(" [{}]", tags.join(", "))
        };

        let path_str = if node.path == node.name {
            String::new()
        } else {
            format!(" \u{2192} {}", escape(&node.path))
        };

        output.push_str(&format!(
            "{}{}{}{} ({}){}\n",
            prefix,
            connector,
            escape(&node.name),
            path_str,
            remote_label(node.repo.as_ref()),
            tags_str
        ));
        if let Some(error) = &node.error {
            output.push_str(&format!("{prefix}    ✗ {}\n", escape(error)));
        }

        if !node.projects.is_empty() {
            let child_prefix = if is_last {
                format!("{prefix}    ")
            } else {
                format!("{prefix}\u{2502}   ")
            };
            format_project_tree(&node.projects, output, &child_prefix);
        }
    }
}

// ============================================================================
// Project Dependents
// ============================================================================

/// Handle `meta project dependents <name>`
///
/// Finds all projects that directly depend on the given project (via `depends_on`
/// matching the project's name or any of its `provides` entries).
fn handle_project_dependents(
    project_name: &str,
    json: bool,
    recursive: bool,
    cwd: &Path,
) -> CommandResult {
    match dependents(project_name, recursive, cwd) {
        Err(e) => CommandResult::from(e),
        Ok(dependents) if json => match serde_json::to_string(&dependents) {
            Ok(json) => CommandResult::Message(json),
            Err(e) => CommandResult::Error(format!("Failed to serialize JSON: {e}")),
        },
        Ok(dependents) if dependents.is_empty() => {
            CommandResult::Message(format!("No projects depend on '{}'.", escape(project_name)))
        }
        Ok(dependents) => {
            let names: Vec<_> = dependents.iter().map(|d| escape(d)).collect();
            CommandResult::Message(names.join("\n"))
        }
    }
}

/// The projects that depend directly on `project_name`, among those of the
/// nearest workspace by name; with `recursive`, among every project in the
/// tree of the root workspace, its own by name and nested ones by their path
/// from it (two nested repos may share a name).
fn dependents(project_name: &str, recursive: bool, cwd: &Path) -> anyhow::Result<Vec<String>> {
    fn walk(nodes: Vec<MetaTreeNode>, prefix: &str, out: &mut Vec<ProjectDependencies>) {
        for node in nodes {
            let path = format!("{prefix}/{}", node.info.path);
            let mut project = ProjectDependencies::from(node.info);
            // `depends_on` names it, so its name stands for it as well
            project
                .provides
                .push(std::mem::replace(&mut project.name, path.clone()));
            out.push(project);
            walk(node.children, &path, out);
        }
    }
    let mut workspace = config::Workspace::find(cwd, None)?
        .ok_or_else(|| anyhow::anyhow!("No .meta.yaml found in {}", escape_path(cwd)))?;
    let mut projects = Vec::new();
    if recursive {
        workspace = workspace.root()?;
        for node in config::walk_projects(&workspace.dir, &workspace.projects, None)? {
            walk(node.children, &node.info.path, &mut projects);
            projects.push(node.info.into());
        }
    } else {
        projects.extend(workspace.projects.into_iter().map(Into::into));
    }
    let graph = DependencyGraph::build(projects);
    let dependents = graph
        .dependents_of(project_name)
        .ok_or_else(|| anyhow::anyhow!("Unknown project or alias: {}", escape(project_name)))?;
    Ok(dependents.into_iter().map(str::to_string).collect())
}

// ============================================================================
// Helper Functions
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of a `Message` result, or `None` for any other result.
    fn message(result: CommandResult) -> Option<String> {
        match result {
            CommandResult::Message(msg) => Some(msg),
            _ => None,
        }
    }

    /// The text of an `Error` result, or `None` for any other result.
    fn error(result: CommandResult) -> Option<String> {
        match result {
            CommandResult::Error(msg) | CommandResult::Usage(msg) => Some(msg),
            _ => None,
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
        let sandbox = meta_test_support::Sandbox::new();
        let temp_dir = sandbox.work();

        let options = ExecuteOptions::default();
        let result = execute_command("project check", &[], &options, &[], &temp_dir);

        let msg = error(result).unwrap();
        assert!(msg.contains("No .meta.yaml"), "{msg}");
    }

    #[test]
    fn test_unknown_command() {
        let sandbox = meta_test_support::Sandbox::new();
        let temp_dir = sandbox.work();

        // Create a .meta.yaml file
        std::fs::write(temp_dir.join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();

        let options = ExecuteOptions::default();
        let args = ["unknown".to_string()];
        let result = execute_command("project", &args, &options, &[], &temp_dir);

        assert!(matches!(result, CommandResult::ShowHelp(Some(msg))
            if msg.contains("unrecognized command 'project unknown'")));
    }

    #[test]
    fn test_project_check_all_present() {
        let sandbox = meta_test_support::Sandbox::new();
        let temp_dir = sandbox.work();

        // Create a .meta.yaml file with no projects
        std::fs::write(temp_dir.join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command("project check", &[], &options, &[], &temp_dir);

        let msg = message(result).unwrap();
        assert!(msg.contains("All projects are cloned"));
    }

    #[test]
    fn arguments_after_the_command_and_each_config_once() {
        let sandbox = meta_test_support::isolated!();
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
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let run = |command: &str, a: &[&str], cwd: &Path| {
            execute_command(command, &args(a), &ExecuteOptions::default(), &[], cwd)
        };

        // meta_cli sends the root among the repos: each config counts once,
        // and what is missing comes in project order
        let recursive = ExecuteOptions {
            recursive: true,
            ..Default::default()
        };
        let provided = [root.to_string_lossy().into_owned(), "c".to_string()];
        let missing: Vec<_> = check_projects(&recursive, &provided, &root)
            .unwrap()
            .into_iter()
            .map(|p| (p.name, p.state))
            .collect();
        assert_eq!(
            missing,
            [
                ("alpha", "missing"),
                ("c", "not_a_repo"),
                ("user", "missing"),
                ("zed", "missing"),
                ("g", "not_a_repo")
            ]
            .map(|(n, s)| (n.to_string(), s))
        );
        assert!(error(run("project check", &["x"], &root))
            .unwrap()
            .contains("unexpected argument"));

        // list's own -r and --depth
        let listed = |a: &[&str]| message(run("project list", a, &root)).unwrap();
        assert!(!listed(&[]).contains(" g "), "{}", listed(&[]));
        assert!(listed(&["-r"]).contains(" g "), "{}", listed(&["-r"]));
        assert!(!listed(&["-r", "--depth", "0"]).contains(" g "));

        // dependents' own -r: the root workspace, from inside a nested one
        assert!(error(run("project dependents", &["c"], &nested)).is_some());
        assert_eq!(
            message(run("project dependents", &["c", "-r"], &nested)).unwrap(),
            "user"
        );
    }

    #[test]
    fn recursive_dependents_cover_nested_projects_by_path() {
        let sandbox = meta_test_support::isolated!();
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
        let dependents = |project: &str, cwd: &Path| {
            let args = [project.to_string(), "-r".to_string()];
            message(execute_command(
                "project dependents",
                &args,
                &ExecuteOptions::default(),
                &[],
                cwd,
            ))
            .unwrap()
        };
        // nested projects depend on the root's, and on each other, by name
        assert_eq!(dependents("utils-api", &root), "api\ntools/cli");
        assert_eq!(dependents("terraform", &root), "infra/k8s");
        assert_eq!(dependents("terraform", &root.join("infra")), "infra/k8s");
    }

    #[test]
    fn check_names_a_wrong_remote_and_list_shows_the_roots_from_a_child() {
        let sandbox = meta_test_support::isolated!();
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

        let problems = check_projects(&ExecuteOptions::default(), &[], &root).unwrap();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(
            (problems[0].name.as_str(), problems[0].state),
            ("web", "wrong_remote")
        );
        assert!(matches!(
            check(true, &ExecuteOptions::default(), &[], &root).unwrap(),
            CommandResult::Error(e) if e.contains("1 project(s)")
        ));

        // listed from inside the child, the root is still the workspace's
        let options = ExecuteOptions {
            json_output: true,
            ..Default::default()
        };
        let listed = message(execute_command("project list", &[], &options, &[], &web)).unwrap();
        let listed: serde_json::Value = serde_json::from_str(&listed).unwrap();
        assert_eq!(listed["repo"], "https://example.invalid/ws.git", "{listed}");
    }

    #[test]
    fn test_project_sync_removed() {
        let sandbox = meta_test_support::Sandbox::new();
        let temp_dir = sandbox.work();

        // Create a .meta.yaml file
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"missing-repo": "https://github.com/test/repo.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command("project sync", &[], &options, &[], &temp_dir);

        // sync command should now return an error directing users to meta git update
        assert!(
            matches!(result, CommandResult::ShowHelp(Some(msg)) if msg.contains("meta git update"))
        );
    }

    #[test]
    fn test_project_list_basic() {
        let sandbox = meta_test_support::isolated!();
        let temp_dir = sandbox.work();

        // Create a .meta.yaml file with projects
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git", "repo2": "git@github.com:org/repo2.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command("project list", &[], &options, &[], &temp_dir);

        let msg = message(result).unwrap();
        assert!(msg.contains("repo1"));
        assert!(msg.contains("repo2"));
        assert!(msg.contains(".")); // root indicator
    }

    #[test]
    fn test_project_list_json() {
        let sandbox = meta_test_support::isolated!();
        let temp_dir = sandbox.work();

        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions {
            json_output: true,
            ..Default::default()
        };
        let result = execute_command("project list", &[], &options, &[], &temp_dir);

        let msg = message(result).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed["path"], ".");
        assert!(parsed["projects"].is_array());
        let projects = parsed["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["name"], "repo1");
        assert_eq!(projects[0]["repo"], "git@github.com:org/repo1.git");
    }

    #[test]
    fn test_project_ls_alias() {
        let sandbox = meta_test_support::isolated!();
        let temp_dir = sandbox.work();

        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command("project ls", &[], &options, &[], &temp_dir);

        let msg = message(result).unwrap();
        assert!(msg.contains("repo1"));
    }

    #[test]
    fn test_project_list_recursive() {
        let sandbox = meta_test_support::isolated!();
        let temp_dir = sandbox.work();

        // Create root .meta.yaml
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"child": "git@github.com:org/child.git"}}"#,
        )
        .unwrap();

        // Create child directory with its own .meta.yaml
        let child_dir = temp_dir.join("child");
        std::fs::create_dir(&child_dir).unwrap();
        std::fs::write(
            child_dir.join(".meta.yaml"),
            r#"{"projects": {"grandchild": "git@github.com:org/grandchild.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions {
            recursive: true,
            json_output: true,
            ..Default::default()
        };
        let result = execute_command("project list", &[], &options, &[], &temp_dir);

        let msg = message(result).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        let projects = parsed["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["name"], "child");
        assert_eq!(projects[0]["is_meta"], true);
        let sub_projects = projects[0]["projects"].as_array().unwrap();
        assert_eq!(sub_projects.len(), 1);
        assert_eq!(sub_projects[0]["name"], "grandchild");
    }

    /// Every node has one line of its own, including nested and error nodes.
    fn assert_tree_properties(nodes: &[ProjectTreeNode], output: &str) {
        assert!(
            output.chars().all(|c| c == '\n' || !c.is_control()),
            "{output:?}"
        );
        // each level one step further in; the path only where it isn't the name
        fn check<'a>(
            nodes: &[ProjectTreeNode],
            depth: usize,
            lines: &mut impl Iterator<Item = &'a str>,
        ) {
            for (i, node) in nodes.iter().enumerate() {
                let line = lines.next().expect("a line for each node");
                let connector = if i + 1 == nodes.len() { '└' } else { '├' };
                let at = line.chars().position(|c| c == connector);
                assert_eq!(at, Some(depth * 4), "{line}");
                assert_eq!(line.contains('→'), node.path != node.name, "{line}");
                let mut fields = vec![
                    escape(&node.name).into_owned(),
                    escape(&node.path).into_owned(),
                ];
                fields.push(node.repo.as_ref().map_or_else(
                    || "no remote".into(),
                    |r| escape(&r.to_string()).into_owned(),
                ));
                fields.extend(node.tags.iter().map(|t| escape(t).into_owned()));
                for field in fields {
                    assert!(line.contains(&field), "{field:?} missing on {line:?}");
                }
                if let Some(error) = &node.error {
                    let line = lines.next().expect("the node's error line");
                    assert!(line.contains(escape(error).as_ref()), "{line}");
                }
                check(&node.projects, depth + 1, lines);
            }
        }
        let mut lines = output.lines();
        check(nodes, 0, &mut lines);
        assert_eq!(lines.next(), None, "extra lines: {output}");
    }

    #[test]
    fn test_format_project_tree() {
        let nodes = vec![
            ProjectTreeNode {
                error: None,
                name: "api".to_string(),
                path: "services/api".to_string(),
                repo: Some("git@github.com:org/api.git".into()),
                tags: vec!["backend".to_string()],
                is_meta: false,
                projects: vec![],
            },
            ProjectTreeNode {
                error: None,
                name: "frontend".to_string(),
                path: "frontend".to_string(),
                repo: Some("git@github.com:org/frontend.git".into()),
                tags: vec![],
                is_meta: false,
                projects: vec![
                    ProjectTreeNode {
                        error: None,
                        name: "widget".into(),
                        path: "ui/widget".into(),
                        repo: Some("git@github.com:org/widget.git".into()),
                        tags: vec!["ui".into(), "client".into()],
                        is_meta: false,
                        projects: vec![],
                    },
                    ProjectTreeNode {
                        error: Some("cannot read config".into()),
                        name: "broken".into(),
                        path: "ui/broken".into(),
                        repo: None,
                        tags: vec![],
                        is_meta: false,
                        projects: vec![],
                    },
                ],
            },
        ];

        let mut output = String::new();
        format_project_tree(&nodes, &mut output, "");
        assert_tree_properties(&nodes, &output);
    }

    #[test]
    fn test_format_project_tree_escapes_control_characters() {
        let nodes = vec![ProjectTreeNode {
            error: None,
            name: "x\u{1b}]0;PWNED\u{7}".to_string(),
            path: "t\u{1b}[1A".to_string(),
            repo: Some("u\u{9b}2J".into()),
            tags: vec!["n\nfake".to_string()],
            is_meta: false,
            projects: vec![],
        }];

        let mut output = String::new();
        format_project_tree(&nodes, &mut output, "");
        assert_tree_properties(&nodes, &output);
    }

    #[test]
    fn test_format_project_tree_without_remote() {
        let nodes = vec![ProjectTreeNode {
            error: None,
            name: "scratch".to_string(),
            path: "scratch".to_string(),
            repo: None,
            tags: vec![],
            is_meta: false,
            projects: vec![],
        }];

        let mut output = String::new();
        format_project_tree(&nodes, &mut output, "");
        assert_tree_properties(&nodes, &output);
    }

    #[test]
    fn test_project_list_json_includes_cwd() {
        let sandbox = meta_test_support::isolated!();
        let temp_dir = sandbox.work();

        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions {
            json_output: true,
            ..Default::default()
        };
        let result = execute_command("project list", &[], &options, &[], &temp_dir);

        let msg = message(result).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        // cwd field should be present and be an absolute path
        let cwd = parsed["cwd"]
            .as_str()
            .expect("cwd field should be a string");
        assert!(
            std::path::Path::new(cwd).is_absolute(),
            "cwd should be absolute, got: {cwd}"
        );
        // root field should be present and be an absolute path
        let root = parsed["root"]
            .as_str()
            .expect("root field should be a string");
        assert!(
            std::path::Path::new(root).is_absolute(),
            "root should be absolute, got: {root}"
        );
    }

    #[test]
    fn test_project_list_json_includes_linked_worktree_context() {
        let sandbox = meta_test_support::isolated!();
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

        let options = ExecuteOptions {
            json_output: true,
            ..Default::default()
        };
        let result = execute_command("project list", &[], &options, &[], &linked);

        let msg = message(result).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        let expected_root = comparable_path(&temp_dir.canonicalize().unwrap());
        let expected_linked = comparable_path(&linked.canonicalize().unwrap());
        let worktree = parsed["worktree"]
            .as_object()
            .expect("worktree field should be present");
        assert_eq!(worktree["linked"], true);
        assert_eq!(
            worktree["physical_root"].as_str().map(comparable_path_str),
            Some(expected_linked.clone())
        );
        assert_eq!(
            worktree["canonical_root"].as_str().map(comparable_path_str),
            Some(expected_root.clone())
        );
        assert_eq!(worktree["name"].as_str(), Some("feature"));
        assert_eq!(
            parsed["root"].as_str().map(comparable_path_str),
            Some(expected_root)
        );
        assert_eq!(
            parsed["cwd"].as_str().map(comparable_path_str),
            Some(expected_linked)
        );
    }

    #[test]
    fn test_project_list_json_root_differs_from_cwd_in_nested_recursive() {
        // Regression: when invoked from a child directory with --recursive,
        // `root` must point to the outermost workspace root, NOT to cwd.
        let sandbox = meta_test_support::isolated!();
        let temp_dir = sandbox.work();
        let child = temp_dir.join("child");
        std::fs::create_dir_all(&child).unwrap();

        // Root .meta.yaml
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"child": {"repo": "git@github.com:org/child.git", "meta": true}}}"#,
        )
        .unwrap();

        // Child .meta.yaml
        std::fs::write(
            child.join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions {
            recursive: true,
            json_output: true,
            ..Default::default()
        };
        // Run from child (nested) directory
        let result = execute_command("project list", &[], &options, &[], &child);

        let msg = message(result).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        let cwd = parsed["cwd"]
            .as_str()
            .expect("cwd field should be a string");
        let root = parsed["root"]
            .as_str()
            .expect("root field should be a string");
        assert!(
            std::path::Path::new(root).is_absolute(),
            "root should be absolute, got: {root}"
        );
        // root must be the outermost workspace root, which is the parent of child
        assert_ne!(
            root, cwd,
            "root should differ from cwd when invoked from a nested meta workspace"
        );
    }

    #[test]
    fn test_project_list_recursive_from_nested() {
        // root has child, child has grandchild
        // Running recursive from child should show root's tree
        let sandbox = meta_test_support::isolated!();
        let temp_dir = sandbox.work();
        let child = temp_dir.join("child");
        let grandchild = child.join("grandchild");
        std::fs::create_dir_all(&grandchild).unwrap();

        // Root .meta.yaml
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"child": {"repo": "git@github.com:org/child.git", "meta": true}}}"#,
        )
        .unwrap();

        // Child .meta.yaml
        std::fs::write(
            child.join(".meta.yaml"),
            r#"{"projects": {"grandchild": "git@github.com:org/grandchild.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions {
            recursive: true,
            json_output: true,
            ..Default::default()
        };
        // Run from child directory
        let result = execute_command("project list", &[], &options, &[], &child);

        let msg = message(result).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        // Should show root's projects (child), not child's projects (grandchild) at top level
        let projects = parsed["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["name"], "child");
        // And child should have grandchild as nested
        let sub_projects = projects[0]["projects"].as_array().unwrap();
        assert_eq!(sub_projects.len(), 1);
        assert_eq!(sub_projects[0]["name"], "grandchild");
    }

    #[test]
    fn test_project_list_non_recursive_stays_local() {
        // Running non-recursive from child should only show child's projects
        let sandbox = meta_test_support::isolated!();
        let temp_dir = sandbox.work();
        let child = temp_dir.join("child");
        let grandchild = child.join("grandchild");
        std::fs::create_dir_all(&grandchild).unwrap();

        // Root .meta.yaml
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"{"projects": {"child": {"repo": "git@github.com:org/child.git", "meta": true}}}"#,
        )
        .unwrap();

        // Child .meta.yaml
        std::fs::write(
            child.join(".meta.yaml"),
            r#"{"projects": {"grandchild": "git@github.com:org/grandchild.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions {
            recursive: false,
            json_output: true,
            ..Default::default()
        };
        // Run from child directory (non-recursive)
        let result = execute_command("project list", &[], &options, &[], &child);

        let msg = message(result).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        // Should show child's projects only (grandchild)
        let projects = parsed["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["name"], "grandchild");
    }

    #[test]
    fn test_project_dependents_command() {
        let sandbox = meta_test_support::isolated!();
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

        let options = ExecuteOptions::default();
        let result = execute_command(
            "project dependents",
            &["loop_lib".to_string()],
            &options,
            &[],
            &temp_dir,
        );

        let msg = message(result).unwrap();
        assert_eq!(msg.trim(), "meta_cli");
    }

    #[test]
    fn test_project_dependents_from_subdirectory() {
        // T8: running from a subdirectory should climb to the nearest .meta.yaml ancestor
        let sandbox = meta_test_support::isolated!();
        let temp_dir = sandbox.work();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"
projects:
  lib_a:
    repo: git@github.com:org/lib_a.git
    provides: [lib-a]
  app_b:
    repo: git@github.com:org/app_b.git
    depends_on: [lib-a]
"#,
        )
        .unwrap();

        // Create a subdirectory to invoke from
        let sub_dir = temp_dir.join("lib_a");
        std::fs::create_dir(&sub_dir).unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command(
            "project dependents",
            &["lib_a".to_string()],
            &options,
            &[],
            &sub_dir,
        );

        let msg = message(result).unwrap();
        assert_eq!(msg.trim(), "app_b");
    }

    #[test]
    fn test_project_dependents_json() {
        let sandbox = meta_test_support::isolated!();
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

        let options = ExecuteOptions {
            json_output: true,
            ..Default::default()
        };
        let result = execute_command(
            "project dependents",
            &["a".to_string()],
            &options,
            &[],
            &temp_dir,
        );

        let msg = message(result).unwrap();
        let parsed: Vec<String> = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed, vec!["b", "c"]);
    }

    #[test]
    fn test_project_dependents_json_flag_in_args() {
        let sandbox = meta_test_support::isolated!();
        let temp_dir = sandbox.work();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"
projects:
  a:
    repo: git@github.com:org/a.git
  b:
    repo: git@github.com:org/b.git
    depends_on: [a]
"#,
        )
        .unwrap();

        for args in [
            vec!["a".to_string(), "--json".to_string()],
            vec!["--json".to_string(), "a".to_string()],
        ] {
            let result = execute_command(
                "project dependents",
                &args,
                &ExecuteOptions::default(),
                &[],
                &temp_dir,
            );

            let msg = message(result).unwrap();
            let parsed: Vec<String> = serde_json::from_str(&msg).unwrap();
            assert_eq!(parsed, vec!["b"], "for args: {args:?}");
        }
    }

    #[test]
    fn test_project_dependents_rejects_what_it_does_not_take() {
        let sandbox = meta_test_support::Sandbox::new();
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
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            let result = execute_command(
                "project dependents",
                &args,
                &ExecuteOptions::default(),
                &[],
                &temp_dir,
            );
            let msg = error(result).unwrap();
            assert!(msg.contains("unexpected argument"), "{args:?}: {msg}");
            assert!(!msg.contains('\x1b'), "{msg}");
        }
    }

    #[test]
    fn test_project_dependents_unknown_project_error() {
        // T13: querying a nonexistent project name should error, not say "no dependents"
        let sandbox = meta_test_support::Sandbox::new();
        let temp_dir = sandbox.work();
        std::fs::write(
            temp_dir.join(".meta.yaml"),
            r#"
projects:
  a:
    repo: git@github.com:org/a.git
"#,
        )
        .unwrap();

        let result = execute_command(
            "project dependents",
            &["nonexistent".to_string()],
            &ExecuteOptions::default(),
            &[],
            &temp_dir,
        );
        let msg = error(result).unwrap();
        assert!(msg.contains("Unknown project or alias"), "{msg}");
    }
}
