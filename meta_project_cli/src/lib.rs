//! meta-project library
//!
//! Provides project management commands for meta repositories.

use meta_cli::worktree;
use meta_core::config::{self, MetaTreeNode, ProjectInfo};
use meta_core::display::{escape, escape_path};
use serde::Serialize;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;

pub use meta_plugin_protocol::{
    output_execution_plan, CommandResult, ExecutionPlan, PlanResponse, PlannedCommand,
};

/// Options passed to execute_command
#[derive(Debug, Default, Clone, Copy)]
pub struct ExecuteOptions {
    pub dry_run: bool,
    pub json_output: bool,
    pub recursive: bool,
    pub depth: Option<usize>,
    pub verbose: bool,
    pub parallel: bool,
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
    if has_help_token(command, args) {
        return project_command_help(command);
    }

    // meta_cli sends an unclaimed `project <word>` as `project` plus args
    if command == "project" {
        let typed: Vec<&str> = std::iter::once(command)
            .chain(args.iter().map(String::as_str))
            .collect();
        return CommandResult::ShowHelp(Some(format!(
            "unrecognized command '{}'. Use 'meta git update' to sync projects.",
            escape(&typed.join(" "))
        )));
    }

    // project dependents reads the dependency graph directly
    if command == "project dependents" {
        return handle_project_dependents(args, options, cwd);
    }

    // project list/ls handles its own config discovery
    if command == "project list" || command == "project ls" {
        // It lists the whole tree, so a filter would be silently ignored
        let trailing_filter = args.iter().any(|a| {
            a.starts_with("--include")
                || a.starts_with("--exclude")
                || (a.starts_with("-i") || a.starts_with("-e")) && !a.starts_with("--")
        });
        if options.filtered || trailing_filter {
            return CommandResult::Error(
                "meta project list does not take --include/--exclude".to_string(),
            );
        }
        // Check if --json was passed as a trailing arg (not extracted by meta_cli
        // because it could be intended for a subcommand in non-plugin contexts)
        let json_from_args = args.iter().any(|a| a == "--json");
        let effective_options = if json_from_args && !options.json_output {
            ExecuteOptions {
                json_output: true,
                ..*options
            }
        } else {
            ExecuteOptions { ..*options }
        };
        return handle_project_list(cwd, &effective_options);
    }

    // With --recursive, check each project directory meta_cli provided for
    // missing repos in its own .meta.yaml (meta_cli sends the list either way;
    // without --recursive only the root config counts)
    if options.recursive && !provided_projects.is_empty() {
        return execute_command_recursive(command, options, provided_projects, cwd);
    }

    // The workspace's own config: the nearest one at or above cwd, as meta
    // finds it (an unreadable config is an error)
    let meta_path = match config::find_meta_config(cwd, None) {
        Ok(Some(path)) => path,
        Ok(None) => {
            return CommandResult::Error(format!("No .meta.yaml found in {}", escape_path(cwd)))
        }
        Err(e) => return CommandResult::Error(format!("{e:#}")),
    };
    let projects = match parse_meta_projects(&meta_path) {
        Ok(projects) => projects,
        Err(e) => return CommandResult::Error(format!("Failed to parse meta config: {e:#}")),
    };
    let root = meta_path.parent().unwrap_or(cwd);
    let missing = find_missing_projects(&projects, root);

    match command {
        "project check" => {
            if missing.is_empty() {
                CommandResult::Message("All projects are cloned and present.".to_string())
            } else {
                // Print missing repos (uses visual formatting)
                print_missing(&missing);
                CommandResult::Message(format!(
                    "{} project(s) missing. Run 'meta git update' to clone them.",
                    missing.len()
                ))
            }
        }
        _ => CommandResult::ShowHelp(Some(format!(
            "unrecognized command '{}'. Use 'meta git update' to sync projects.",
            escape(command)
        ))),
    }
}

/// A `--help`/`-h` before `--`; after it, they are arguments for the command.
fn has_help_token(command: &str, args: &[String]) -> bool {
    command
        .split_whitespace()
        .chain(
            args.iter()
                .map(String::as_str)
                .take_while(|arg| *arg != "--"),
        )
        .any(|arg| arg == "--help" || arg == "-h")
}

fn project_command_help(command: &str) -> CommandResult {
    let words: Vec<&str> = command
        .split_whitespace()
        .filter(|word| *word != "--help" && *word != "-h")
        .collect();
    let subcommand = words.get(1).copied();

    let help = match subcommand {
        Some("list") | Some("ls") => {
            r#"meta project list - Inspect the meta repo graph

Usage: meta project list [OPTIONS]

Options:
  --json          Emit a structured tree with root, cwd, project paths, repo URLs, tags, and nested projects
  --recursive    Include projects from nested meta repos
  --depth <N>    Limit recursive traversal depth

Agent hints:
  Use `meta project list --recursive --json` before broad changes to discover the full repo graph.
  Use non-recursive output for a quick human-readable view of the current meta repo.

Examples:
  meta project list
  meta project list --recursive
  meta project list --recursive --json"#
        }
        Some("check") => {
            r#"meta project check - Verify configured projects are present

Usage: meta project check [OPTIONS]

Options:
  --recursive    Check nested meta repos as well
  --depth <N>    Limit recursive traversal depth

Agent hints:
  Run this when commands fail because a child checkout is missing.
  Missing repos can usually be cloned with `meta git update`.

Examples:
  meta project check
  meta project check --recursive"#
        }
        Some("dependents") => {
            r#"meta project dependents - Find projects that depend on another project

Usage: meta project dependents <PROJECT> [OPTIONS]

Arguments:
  <PROJECT>      Project name or provided capability to search for

Options:
  --json         Emit machine-readable dependent project records

Agent hints:
  Use this before changing shared libraries or APIs to identify likely blast radius.
  Pair with `meta project list --recursive --json` when working across nested meta repos.

Examples:
  meta project dependents meta_core
  meta project dependents auth --json"#
        }
        _ => {
            r#"meta project - Inspect and validate meta workspace projects

Usage: meta project <COMMAND>

Commands:
  list          List projects defined in .meta.yaml (alias: ls)
  check         Verify configured projects are cloned
  dependents    List projects that depend on a project or capability

Agent hints:
  Prefer `meta project list --recursive --json` for codebase discovery.
  Use `meta project check --recursive` before assuming a repo is intentionally absent.

Examples:
  meta project list --recursive --json
  meta project check
  meta project dependents meta_core"#
        }
    };

    CommandResult::Message(help.to_string())
}

/// Execute a project command recursively across provided project directories
///
/// This handles the case when --recursive is used. Each project directory may have
/// its own .meta.yaml file with additional projects to check/sync.
fn execute_command_recursive(
    command: &str,
    _options: &ExecuteOptions,
    provided_projects: &[String],
    cwd: &Path,
) -> CommandResult {
    let mut all_missing: Vec<(String, String)> = Vec::new();

    // Check the root meta config first
    // A config that is refused is an error, not "nothing missing"
    let root = match config::find_meta_config_in(cwd) {
        Ok(found) => found,
        Err(e) => return CommandResult::Error(format!("{e:#}")),
    };
    if let Some(root_meta_path) = root {
        let projects = match parse_meta_projects(&root_meta_path) {
            Ok(projects) => projects,
            Err(e) => return CommandResult::Error(format!("{e:#}")),
        };
        for (name, url) in find_missing_projects(&projects, cwd) {
            all_missing.push((name, url));
        }
    }

    // Check each provided project directory for its own meta config
    for project_path in provided_projects {
        let project_dir = cwd.join(project_path);
        // One that can't be examined is an error, not "nothing missing"
        let nested = match config::find_meta_config_in(&project_dir) {
            Ok(found) => found,
            Err(e) => return CommandResult::Error(format!("{e:#}")),
        };
        if let Some(nested_meta_path) = nested {
            let projects = match parse_meta_projects(&nested_meta_path) {
                Ok(projects) => projects,
                Err(e) => return CommandResult::Error(format!("{e:#}")),
            };
            for (name, url) in find_missing_projects(&projects, &project_dir) {
                // Use the full path relative to cwd
                let full_path = format!("{project_path}/{name}");
                all_missing.push((full_path, url));
            }
        }
    }

    match command {
        "project check" => {
            if all_missing.is_empty() {
                CommandResult::Message("All projects are cloned and present.".to_string())
            } else {
                // Print all missing repos
                for (_, url) in &all_missing {
                    meta_git_lib::print_missing_repo(url);
                }
                println!();
                CommandResult::Message(format!(
                    "{} project(s) missing. Run 'meta git update' to clone them.",
                    all_missing.len()
                ))
            }
        }
        _ => CommandResult::ShowHelp(Some(format!(
            "unrecognized command '{}'. Use 'meta git update' to sync projects.",
            escape(command)
        ))),
    }
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

    let root_repo = get_git_remote_url(&start_dir).unwrap_or_default();
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

/// Get the git remote origin URL for a directory
fn get_git_remote_url(dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .current_dir(dir)
        .output()
        .ok()?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
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
    args: &[String],
    options: &ExecuteOptions,
    cwd: &Path,
) -> CommandResult {
    // Check if --json was passed as a trailing arg (not extracted by meta_cli)
    let json_from_args = args.iter().any(|a| a == "--json");
    let effective_options = if json_from_args && !options.json_output {
        ExecuteOptions {
            json_output: true,
            ..*options
        }
    } else {
        ExecuteOptions { ..*options }
    };
    let options = &effective_options;

    let mut positionals = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--json" | "--recursive" | "-r" | "--verbose" | "--parallel" => {}
            "--depth" => {
                iter.next();
            }
            _ if arg.starts_with("--depth=") => {}
            _ if !arg.starts_with('-') => positionals.push(arg),
            _ => {}
        }
    }

    let project_name = match positionals.as_slice() {
        [name] => (*name).clone(),
        [] => {
            return CommandResult::ShowHelp(Some(
                "Usage: meta project dependents <project-name>".to_string(),
            ))
        }
        _ => {
            return CommandResult::Error(
                "Expected exactly one <project-name> argument.".to_string(),
            )
        }
    };

    // Find the root meta config
    let nearest_meta_path = match config::find_meta_config(cwd, None) {
        Ok(Some(path)) => path,
        Ok(None) => {
            return CommandResult::Error(format!("No .meta.yaml found in {}", escape_path(cwd)))
        }
        Err(e) => return CommandResult::Error(format!("{e:#}")),
    };
    let nearest_meta_dir = nearest_meta_path.parent().unwrap_or(Path::new("."));
    let start_dir = if options.recursive {
        match config::find_root_meta_dir(nearest_meta_dir) {
            Ok(root) => root,
            Err(e) => return CommandResult::Error(format!("{e:#}")),
        }
    } else {
        nearest_meta_dir.to_path_buf()
    };

    let found = match config::find_meta_config_in(&start_dir) {
        Ok(found) => found,
        Err(e) => return CommandResult::Error(format!("{e:#}")),
    };
    let Some(meta_path) = found else {
        return CommandResult::Error(format!(
            "No .meta.yaml found in {}",
            escape_path(&start_dir)
        ));
    };

    let (all_projects, _ignore) = match config::parse_meta_config(&meta_path) {
        Ok(result) => result,
        Err(e) => return CommandResult::Error(format!("Failed to parse meta config: {e}")),
    };

    let Some(dependents) = find_dependents(&project_name, &all_projects) else {
        return CommandResult::Error(format!(
            "Unknown project or alias: {}",
            escape(&project_name)
        ));
    };

    if options.json_output {
        let json = match serde_json::to_string(&dependents) {
            Ok(j) => j,
            Err(e) => return CommandResult::Error(format!("Failed to serialize JSON: {e}")),
        };
        CommandResult::Message(json)
    } else {
        if dependents.is_empty() {
            CommandResult::Message(format!(
                "No projects depend on '{}'.",
                escape(&project_name)
            ))
        } else {
            let names: Vec<_> = dependents.iter().map(|d| escape(d)).collect();
            CommandResult::Message(names.join("\n"))
        }
    }
}

/// Normalize a dependency token for case- and separator-insensitive matching.
///
/// Folds hyphens to underscores and lowercases the string so that
/// `loop-lib`, `loop_lib`, and `Loop_Lib` all resolve to the same token.
fn normalize_token(s: &str) -> String {
    s.replace('-', "_").to_lowercase()
}

/// Find all projects that depend on the given project.
///
/// A project B depends on project A if B's `depends_on` contains:
/// - A's name directly, OR
/// - Any of A's `provides` entries
///
/// Token matching is normalized (hyphens ↔ underscores, case-insensitive).
fn find_dependents(project_name: &str, all_projects: &[ProjectInfo]) -> Option<Vec<String>> {
    // Resolve the target project by normalized name or alias
    let normalized_query = normalize_token(project_name);
    let target = all_projects
        .iter()
        .find(|project| normalize_token(&project.name) == normalized_query)
        .or_else(|| {
            all_projects.iter().find(|project| {
                project
                    .provides
                    .iter()
                    .any(|token| normalize_token(token) == normalized_query)
            })
        })?;

    // Build the set of normalized tokens that the target provides
    let mut provided_tokens: HashSet<String> = HashSet::new();
    provided_tokens.insert(normalize_token(&target.name));
    for token in &target.provides {
        provided_tokens.insert(normalize_token(token));
    }

    // Find all projects whose depends_on intersects with provided_tokens
    let mut dependents: Vec<String> = all_projects
        .iter()
        .filter(|p| p.name != target.name)
        .filter(|p| {
            p.depends_on
                .iter()
                .any(|dep| provided_tokens.contains(&normalize_token(dep)))
        })
        .map(|p| p.name.clone())
        .collect();

    dependents.sort();
    Some(dependents)
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Get help text for the plugin
pub fn get_help_text() -> &'static str {
    r#"meta project - Project Inspection Plugin

Commands:
  meta project list         List all projects defined in .meta.yaml (alias: ls)
  meta project check        Check if all projects in .meta.yaml are cloned locally
  meta project dependents   List projects that depend on a given project

Options for list:
  --json               Output as JSON
  --recursive, -r      Include nested meta repo children
  --depth N            Maximum recursion depth (default: unlimited)

Examples:
  meta project dependents meta_git_lib          # Who depends on meta_git_lib?
  meta project dependents meta_git_lib --json   # JSON output

To clone missing projects, use: meta git update
"#
}

fn parse_meta_projects(meta_path: &Path) -> anyhow::Result<HashMap<String, String>> {
    let (projects, _ignore) = config::parse_meta_config(meta_path)?;
    let mut map = HashMap::new();
    for p in projects {
        // Skip projects without a repo URL (cannot clone)
        if let Some(repo) = p.repo {
            map.insert(p.path.clone(), repo);
        }
    }
    Ok(map)
}

fn find_missing_projects(
    projects: &HashMap<String, String>,
    base_dir: &Path,
) -> Vec<(String, String)> {
    projects
        .iter()
        .filter(|(name, _)| !base_dir.join(name).is_dir())
        .map(|(name, url)| (name.clone(), url.clone()))
        .collect()
}

fn print_missing(missing: &[(String, String)]) {
    if !missing.is_empty() {
        for (_, url) in missing {
            meta_git_lib::print_missing_repo(url);
        }
        println!();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// The text of a `Message` result, or `None` for any other result.
    fn message(result: CommandResult) -> Option<String> {
        match result {
            CommandResult::Message(msg) => Some(msg),
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

        assert!(matches!(result, CommandResult::Error(msg) if msg.contains("No .meta.yaml")));
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
    fn test_get_help_text() {
        let help = get_help_text();
        assert!(help.contains("project check"));
        assert!(help.contains("project list"));
        assert!(help.contains("meta git update")); // Points to the right command
        assert!(!help.contains("project sync")); // sync is removed
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
            let output = Command::new("git")
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

    // ── find_dependents ────────────────────────────────────────

    fn make_project(name: &str, provides: &[&str], depends_on: &[&str]) -> ProjectInfo {
        ProjectInfo {
            name: name.to_string(),
            path: name.to_string(),
            repo: Some(format!("git@github.com:org/{name}.git")),
            tags: vec![],
            provides: provides.iter().map(|s| s.to_string()).collect(),
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
            meta: false,
        }
    }

    #[test]
    fn test_find_dependents_by_name() {
        let projects = vec![
            make_project("a", &[], &[]),
            make_project("b", &[], &["a"]),
            make_project("c", &[], &["b"]),
        ];
        assert_eq!(find_dependents("a", &projects), Some(vec!["b".to_string()]));
        assert_eq!(find_dependents("b", &projects), Some(vec!["c".to_string()]));
        assert_eq!(find_dependents("c", &projects), Some(vec![]));
    }

    #[test]
    fn test_find_dependents_by_provides() {
        let projects = vec![
            make_project("loop_lib", &["loop-lib"], &[]),
            make_project("meta_cli", &[], &["loop-lib"]),
            make_project("meta_git_cli", &[], &["loop-lib"]),
        ];
        assert_eq!(
            find_dependents("loop_lib", &projects),
            Some(vec!["meta_cli".to_string(), "meta_git_cli".to_string()])
        );
    }

    #[test]
    fn test_find_dependents_normalized_tokens() {
        // depends_on uses underscore, provides uses hyphen — should still match
        let projects = vec![
            make_project("loop_lib", &["loop-lib"], &[]),
            make_project("consumer_a", &[], &["loop_lib"]),
            make_project("consumer_b", &[], &["Loop-Lib"]),
        ];
        assert_eq!(
            find_dependents("loop_lib", &projects),
            Some(vec!["consumer_a".to_string(), "consumer_b".to_string()])
        );
    }

    #[test]
    fn test_find_dependents_by_alias_query() {
        // Querying by a provides alias should resolve the target and find all dependents
        let projects = vec![
            make_project("loop_lib", &["loop-lib"], &[]),
            make_project("meta_cli", &[], &["loop-lib"]),
            make_project("meta_git_cli", &[], &["loop_lib"]),
        ];
        // Query by alias "loop-lib" should find the target "loop_lib" and collect all dependents
        assert_eq!(
            find_dependents("loop-lib", &projects),
            Some(vec!["meta_cli".to_string(), "meta_git_cli".to_string()])
        );
    }

    #[test]
    fn test_find_dependents_exact_name_before_alias() {
        // T10: If one project merely provides `util` and another is named `util`,
        // the exact name match should win regardless of iteration order.
        let projects = vec![
            make_project("provider", &["util"], &[]),
            make_project("util", &[], &[]),
            make_project("consumer", &[], &["util"]),
        ];
        // Querying "util" should resolve to the project *named* "util", not "provider"
        let deps = find_dependents("util", &projects).unwrap();
        assert_eq!(deps, vec!["consumer"]);
        // "provider" should NOT appear as a dependent
        assert!(!deps.contains(&"provider".to_string()));
    }

    #[test]
    fn test_find_dependents_no_match() {
        let projects = vec![make_project("a", &[], &[]), make_project("b", &[], &[])];
        assert_eq!(find_dependents("a", &projects), Some(vec![]));
    }

    #[test]
    fn test_find_dependents_unknown_project() {
        let projects = vec![make_project("a", &[], &[])];
        assert_eq!(find_dependents("nonexistent", &projects), None);
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
            assert_eq!(parsed, vec!["b"]);
        }
    }

    #[test]
    fn test_project_dependents_depth_flag_skipped() {
        // T12: --depth N should not be treated as a positional arg
        let temp_dir = TempDir::new().unwrap();
        std::fs::write(
            temp_dir.path().join(".meta.yaml"),
            r#"
projects:
  lib_a:
    repo: git@github.com:org/lib_a.git
  app_b:
    repo: git@github.com:org/app_b.git
    depends_on: [lib_a]
"#,
        )
        .unwrap();

        // --depth 1 lib_a: "1" should NOT be picked as project name
        let result = execute_command(
            "project dependents",
            &["--depth".to_string(), "1".to_string(), "lib_a".to_string()],
            &ExecuteOptions::default(),
            &[],
            temp_dir.path(),
        );
        let msg = message(result).unwrap();
        assert_eq!(msg.trim(), "app_b");

        // --depth=1 lib_a variant
        let result = execute_command(
            "project dependents",
            &["--depth=1".to_string(), "lib_a".to_string()],
            &ExecuteOptions::default(),
            &[],
            temp_dir.path(),
        );
        let msg = message(result).unwrap();
        assert_eq!(msg.trim(), "app_b");
    }

    #[test]
    fn test_project_dependents_multiple_positionals_error() {
        // T12: multiple positional args should error
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
            &["a".to_string(), "b".to_string()],
            &ExecuteOptions::default(),
            &[],
            temp_dir.path(),
        );
        assert!(matches!(result, CommandResult::Error(msg) if msg.contains("exactly one")));
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
        assert!(
            matches!(result, CommandResult::Error(msg) if msg.contains("Unknown project or alias"))
        );
    }
}
