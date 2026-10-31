//! meta-project library
//!
//! Provides project management commands for meta repositories.

use meta_cli::dependency_graph::DependencyGraph;
use meta_cli::worktree;
use meta_core::config::{self, MetaTreeNode};
use meta_core::display::{escape, escape_path};
use serde::Serialize;
use std::borrow::Cow;
use std::collections::HashSet;
use std::path::Path;

use meta_plugin_protocol::{has_help_token, parse_args, CommandResult};

/// Options passed to execute_command
#[derive(Debug, Default, Clone, Copy)]
pub struct ExecuteOptions {
    pub json_output: bool,
    pub recursive: bool,
    pub depth: Option<usize>,
    /// `--include` or `--exclude` was given
    pub filtered: bool,
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
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub is_meta: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<ProjectTreeNode>,
}

/// Top-level output for `meta project list --json`
#[derive(Debug, Clone, Serialize)]
pub struct ProjectListOutput {
    pub path: String,
    pub repo: String,
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
  meta --recursive project check"
)]
struct CheckArgs {}

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
        "project list" | "project ls" => {
            // It lists the whole tree, so a filter would be silently ignored
            if options.filtered {
                return CommandResult::Error(
                    "meta project list does not take --include/--exclude".to_string(),
                );
            }
            match parse_args::<ListArgs>(command, args) {
                Ok(list) => handle_project_list(
                    cwd,
                    &ExecuteOptions {
                        json_output: options.json_output || list.json,
                        recursive: options.recursive || list.recursive,
                        depth: list.depth.or(options.depth),
                        filtered: false,
                    },
                ),
                Err(result) => result,
            }
        }
        "project dependents" => match parse_args::<DependentsArgs>(command, args) {
            Ok(dependents) => handle_project_dependents(
                &dependents.project,
                options.json_output || dependents.json,
                options.recursive,
                cwd,
            ),
            Err(result) => result,
        },
        "project check" => match parse_args::<CheckArgs>(command, args) {
            Ok(CheckArgs {}) => check(options, provided_projects, cwd),
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

/// `meta project check`: every project of the workspace is cloned.
fn check(options: &ExecuteOptions, provided_projects: &[String], cwd: &Path) -> CommandResult {
    match missing_projects(options, provided_projects, cwd) {
        Err(e) => CommandResult::Error(format!("{e:#}")),
        Ok(missing) if missing.is_empty() => {
            CommandResult::Message("All projects are cloned and present.".to_string())
        }
        Ok(missing) => {
            print_missing(&missing);
            CommandResult::Message(format!(
                "{} project(s) missing. Run 'meta git update' to clone them.",
                missing.len()
            ))
        }
    }
}

/// The URLs of the projects not cloned: those of the nearest config at or
/// above cwd; with meta's --recursive, those of cwd's config and of each
/// meta repo meta_cli sent. A config that can't be read is an error, not
/// "nothing missing".
fn missing_projects(
    options: &ExecuteOptions,
    provided_projects: &[String],
    cwd: &Path,
) -> anyhow::Result<Vec<String>> {
    if !options.recursive || provided_projects.is_empty() {
        let workspace = config::Workspace::find(cwd, None)?
            .ok_or_else(|| anyhow::anyhow!("No .meta.yaml found in {}", escape_path(cwd)))?;
        return Ok(find_missing_projects(&workspace));
    }
    let mut seen = HashSet::new();
    let mut missing = Vec::new();
    for dir in
        std::iter::once(cwd.to_path_buf()).chain(provided_projects.iter().map(|p| cwd.join(p)))
    {
        let key = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        if !seen.insert(key) {
            continue;
        }
        if let Some(workspace) = config::Workspace::in_dir(&dir)? {
            missing.extend(find_missing_projects(&workspace));
        }
    }
    Ok(missing)
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
                    Err(e) => return CommandResult::Error(format!("{e:#}")),
                }
            }
            Err(e) => return CommandResult::Error(format!("{e:#}")),
            Ok(None) => {
                return CommandResult::Error(format!("No .meta.yaml found in {}", escape_path(cwd)))
            }
        }
    } else {
        cwd.to_path_buf()
    };

    let tree = match config::walk_meta_tree(&start_dir, max_depth) {
        Ok(t) => t,
        Err(e) => return CommandResult::Error(format!("{e}")),
    };

    let root_repo = meta_core::git::remote_url(&start_dir).unwrap_or_default();
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
                Err(e) => return CommandResult::Error(format!("{e:#}")),
            }
        };
        let abs_root = root_dir
            .canonicalize()
            .unwrap_or(root_dir)
            .to_string_lossy()
            .to_string();
        let linked_worktree =
            worktree::detect_worktree_context(&abs_cwd).map(|(name, task_dir, _repos)| {
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
            repo: root_repo,
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
        output.push_str(&format!(". ({})\n", remote_label(Some(&root_repo))));
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
        projects: node.children.iter().map(to_project_tree_node).collect(),
    }
}

/// A project's remote URL as shown in `meta project list`.
fn remote_label(repo: Option<&str>) -> Cow<'_, str> {
    repo.filter(|r| !r.is_empty())
        .map_or(Cow::Borrowed("no remote"), escape)
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
            remote_label(node.repo.as_deref()),
            tags_str
        ));

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
        Err(e) => CommandResult::Error(format!("{e:#}")),
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

/// The projects of the nearest workspace (with meta's --recursive, of the
/// root workspace) that depend directly on `project_name`.
fn dependents(project_name: &str, recursive: bool, cwd: &Path) -> anyhow::Result<Vec<String>> {
    let mut workspace = config::Workspace::find(cwd, None)?
        .ok_or_else(|| anyhow::anyhow!("No .meta.yaml found in {}", escape_path(cwd)))?;
    if recursive {
        let root = config::find_root_meta_dir(&workspace.dir)?;
        if root != workspace.dir {
            workspace = config::Workspace::in_dir(&root)?
                .ok_or_else(|| anyhow::anyhow!("No .meta.yaml found in {}", escape_path(&root)))?;
        }
    }
    let graph = DependencyGraph::build(workspace.projects.into_iter().map(Into::into).collect())?;
    let dependents = graph
        .dependents_of(project_name)
        .ok_or_else(|| anyhow::anyhow!("Unknown project or alias: {}", escape(project_name)))?;
    Ok(dependents.into_iter().map(str::to_string).collect())
}

// ============================================================================
// Helper Functions
// ============================================================================

/// The URLs of the projects of `workspace` that aren't cloned.
fn find_missing_projects(workspace: &config::Workspace) -> Vec<String> {
    workspace
        .projects
        .iter()
        .filter(|p| !workspace.dir.join(&p.path).is_dir())
        .filter_map(|p| p.repo.clone())
        .collect()
}

fn print_missing(urls: &[String]) {
    if !urls.is_empty() {
        for url in urls {
            meta_git_lib::print_missing_repo(url);
        }
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meta_plugin_protocol::{ExecutionPlan, PlanResponse, PlannedCommand};
    use tempfile::TempDir;

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
            CommandResult::Error(msg) => Some(msg),
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
        let temp_dir = TempDir::new().unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command("project check", &[], &options, &[], temp_dir.path());

        let msg = error(result).unwrap();
        assert!(msg.contains("No .meta.yaml"), "{msg}");
    }

    #[test]
    fn test_unknown_command() {
        let temp_dir = TempDir::new().unwrap();

        // Create a .meta.yaml file
        std::fs::write(temp_dir.path().join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();

        let options = ExecuteOptions::default();
        let args = ["unknown".to_string()];
        let result = execute_command("project", &args, &options, &[], temp_dir.path());

        assert!(matches!(result, CommandResult::ShowHelp(Some(msg))
            if msg.contains("unrecognized command 'project unknown'")));
    }

    #[test]
    fn test_project_check_all_present() {
        let temp_dir = TempDir::new().unwrap();

        // Create a .meta.yaml file with no projects
        std::fs::write(temp_dir.path().join(".meta.yaml"), r#"{"projects": {}}"#).unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command("project check", &[], &options, &[], temp_dir.path());

        let msg = message(result).unwrap();
        assert!(msg.contains("All projects are cloned"));
    }

    #[test]
    fn test_project_sync_removed() {
        let temp_dir = TempDir::new().unwrap();

        // Create a .meta.yaml file
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
            r#"{"projects": {"missing-repo": "https://github.com/test/repo.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command("project sync", &[], &options, &[], temp_dir.path());

        // sync command should now return an error directing users to meta git update
        assert!(
            matches!(result, CommandResult::ShowHelp(Some(msg)) if msg.contains("meta git update"))
        );
    }

    #[test]
    fn test_execution_plan_serialization() {
        let commands = vec![PlannedCommand {
            dir: ".".to_string(),
            argv: ["git", "clone", "https://example.com/repo.git", "repo"]
                .map(str::to_string)
                .to_vec(),
            env: None,
        }];
        let plan = ExecutionPlan {
            pre_commands: vec![],
            commands,
            post_commands: vec![],
            parallel: Some(false),
            max_parallel: None,
            spawn_stagger_ms: None,
        };
        let response = PlanResponse { plan };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"plan\""));
        assert!(json.contains("\"commands\""));
        assert!(json.contains(r#""argv":["git","clone""#));
    }

    #[test]
    fn test_project_list_basic() {
        let temp_dir = TempDir::new().unwrap();

        // Create a .meta.yaml file with projects
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git", "repo2": "git@github.com:org/repo2.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command("project list", &[], &options, &[], temp_dir.path());

        let msg = message(result).unwrap();
        assert!(msg.contains("repo1"));
        assert!(msg.contains("repo2"));
        assert!(msg.contains(".")); // root indicator
    }

    #[test]
    fn test_project_list_json() {
        let temp_dir = TempDir::new().unwrap();

        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions {
            json_output: true,
            ..Default::default()
        };
        let result = execute_command("project list", &[], &options, &[], temp_dir.path());

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
        let temp_dir = TempDir::new().unwrap();

        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions::default();
        let result = execute_command("project ls", &[], &options, &[], temp_dir.path());

        let msg = message(result).unwrap();
        assert!(msg.contains("repo1"));
    }

    #[test]
    fn test_project_list_recursive() {
        let temp_dir = TempDir::new().unwrap();

        // Create root .meta.yaml
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
            r#"{"projects": {"child": "git@github.com:org/child.git"}}"#,
        )
        .unwrap();

        // Create child directory with its own .meta.yaml
        let child_dir = temp_dir.path().join("child");
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
        let result = execute_command("project list", &[], &options, &[], temp_dir.path());

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

    #[test]
    fn test_format_project_tree() {
        let nodes = vec![
            ProjectTreeNode {
                name: "api".to_string(),
                path: "services/api".to_string(),
                repo: Some("git@github.com:org/api.git".to_string()),
                tags: vec!["backend".to_string()],
                is_meta: false,
                projects: vec![],
            },
            ProjectTreeNode {
                name: "frontend".to_string(),
                path: "frontend".to_string(),
                repo: Some("git@github.com:org/frontend.git".to_string()),
                tags: vec![],
                is_meta: false,
                projects: vec![],
            },
        ];

        let mut output = String::new();
        format_project_tree(&nodes, &mut output, "");
        assert_eq!(
            output,
            "\u{251c}\u{2500}\u{2500} api \u{2192} services/api (git@github.com:org/api.git) [backend]\n\
             \u{2514}\u{2500}\u{2500} frontend (git@github.com:org/frontend.git)\n"
        );
    }

    #[test]
    fn test_format_project_tree_escapes_control_characters() {
        let nodes = vec![ProjectTreeNode {
            name: "x\u{1b}]0;PWNED\u{7}".to_string(),
            path: "t\u{1b}[1A".to_string(),
            repo: Some("u\u{9b}2J".to_string()),
            tags: vec!["n\nfake".to_string()],
            is_meta: false,
            projects: vec![],
        }];

        let mut output = String::new();
        format_project_tree(&nodes, &mut output, "");
        assert_eq!(
            output,
            "\u{2514}\u{2500}\u{2500} x\\u{1b}]0;PWNED\\u{7} \u{2192} t\\u{1b}[1A (u\\u{9b}2J) [n\\nfake]\n"
        );
    }

    #[test]
    fn test_format_project_tree_without_remote() {
        let nodes = vec![ProjectTreeNode {
            name: "scratch".to_string(),
            path: "scratch".to_string(),
            repo: None,
            tags: vec![],
            is_meta: false,
            projects: vec![],
        }];

        let mut output = String::new();
        format_project_tree(&nodes, &mut output, "");
        assert_eq!(output, "\u{2514}\u{2500}\u{2500} scratch (no remote)\n");
    }

    #[test]
    fn test_project_list_json_includes_cwd() {
        let temp_dir = TempDir::new().unwrap();

        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git"}}"#,
        )
        .unwrap();

        let options = ExecuteOptions {
            json_output: true,
            ..Default::default()
        };
        let result = execute_command("project list", &[], &options, &[], temp_dir.path());

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
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
            r#"{"projects": {"repo1": "git@github.com:org/repo1.git"}}"#,
        )
        .unwrap();

        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(temp_dir.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        git(&["remote", "add", "origin", "git@github.com:org/root.git"]);
        git(&["add", ".meta.yaml"]);
        git(&["commit", "-q", "-m", "initial meta config"]);

        let worktrees = temp_dir.path().join(".worktrees");
        std::fs::create_dir_all(&worktrees).unwrap();
        let linked = worktrees.join("feature");
        git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ]);

        let options = ExecuteOptions {
            json_output: true,
            ..Default::default()
        };
        let result = execute_command("project list", &[], &options, &[], &linked);

        let msg = message(result).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        let expected_root = comparable_path(&temp_dir.path().canonicalize().unwrap());
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
        let temp_dir = TempDir::new().unwrap();
        let child = temp_dir.path().join("child");
        std::fs::create_dir_all(&child).unwrap();

        // Root .meta.yaml
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
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
        let temp_dir = TempDir::new().unwrap();
        let child = temp_dir.path().join("child");
        let grandchild = child.join("grandchild");
        std::fs::create_dir_all(&grandchild).unwrap();

        // Root .meta.yaml
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
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
        let temp_dir = TempDir::new().unwrap();
        let child = temp_dir.path().join("child");
        let grandchild = child.join("grandchild");
        std::fs::create_dir_all(&grandchild).unwrap();

        // Root .meta.yaml
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
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
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
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
            temp_dir.path(),
        );

        let msg = message(result).unwrap();
        assert_eq!(msg.trim(), "meta_cli");
    }

    #[test]
    fn test_project_dependents_from_subdirectory() {
        // T8: running from a subdirectory should climb to the nearest .meta.yaml ancestor
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
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
        let sub_dir = temp_dir.path().join("lib_a");
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
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
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
            temp_dir.path(),
        );

        let msg = message(result).unwrap();
        let parsed: Vec<String> = serde_json::from_str(&msg).unwrap();
        assert_eq!(parsed, vec!["b", "c"]);
    }

    #[test]
    fn test_project_dependents_json_flag_in_args() {
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
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
                temp_dir.path(),
            );

            let msg = message(result).unwrap();
            let parsed: Vec<String> = serde_json::from_str(&msg).unwrap();
            assert_eq!(parsed, vec!["b"], "for args: {args:?}");
        }
    }

    #[test]
    fn test_project_dependents_rejects_what_it_does_not_take() {
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
            "projects:\n  a:\n    repo: git@github.com:org/a.git\n",
        )
        .unwrap();
        for args in [&["a", "b"][..], &["--depth", "1", "a"], &["a", "--bogus\x1b[2J"]] {
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            let result = execute_command(
                "project dependents",
                &args,
                &ExecuteOptions::default(),
                &[],
                temp_dir.path(),
            );
            let msg = error(result).unwrap();
            assert!(msg.contains("unexpected argument"), "{args:?}: {msg}");
            assert!(!msg.contains('\x1b'), "{msg}");
        }
    }

    #[test]
    fn test_project_dependents_unknown_project_error() {
        // T13: querying a nonexistent project name should error, not say "no dependents"
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
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
            temp_dir.path(),
        );
        let msg = error(result).unwrap();
        assert!(msg.contains("Unknown project or alias"), "{msg}");
    }
}
