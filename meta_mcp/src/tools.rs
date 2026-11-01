//! The tools: each one's arguments as a type, its schema derived from that
//! type, and what it does.

use crate::McpServer;
use anyhow::{Context, Result};
use loop_lib::JsonOutput;
use meta_cli::dependency_graph::{self, ProjectDependencies};
use meta_cli::query::{Query, RepoState, WorkspaceState};
use meta_core::config::{self, ProjectInfo};
use meta_core::display::escape;
use meta_core::git::{git, RepoStatus};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

/// What a tool answers: its text, and whether it failed.
pub(crate) struct Reply {
    pub(crate) text: String,
    pub(crate) is_error: bool,
}

impl From<String> for Reply {
    fn from(text: String) -> Self {
        Reply {
            text,
            is_error: false,
        }
    }
}

/// One tool: its name, what it says it does, the schema of its arguments
/// and what runs it.
pub(crate) struct Tool {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) input_schema: serde_json::Value,
    call: Box<Call>,
}

/// What runs a tool, given its arguments.
type Call = dyn Fn(&McpServer, serde_json::Value) -> Result<Reply> + Send + Sync;

impl Tool {
    pub(crate) fn call(&self, server: &McpServer, arguments: serde_json::Value) -> Result<Reply> {
        (self.call)(server, arguments)
    }
}

/// The tool `name`, which runs `run` once its arguments are read as `A`: a
/// wrong type or an unknown argument is an error naming the tool, the
/// argument and the value, before anything runs.
fn tool<A, R>(
    name: &'static str,
    description: &'static str,
    run: fn(&McpServer, A) -> Result<R>,
) -> Tool
where
    A: DeserializeOwned + JsonSchema + 'static,
    R: Into<Reply> + 'static,
{
    let schema = input_schema::<A>();
    Tool {
        name,
        description,
        input_schema: schema.clone(),
        call: Box::new(move |server, arguments| {
            // serde would also read a struct from a list of its fields' values
            if let Some((path, given)) = not_an_object(&schema, &arguments, ".".to_string()) {
                return Err(invalid(name, &path, "not an object", Some(given)));
            }
            let parsed = serde_path_to_error::deserialize(&arguments).map_err(|e| {
                use serde_path_to_error::Segment;
                let given = e
                    .path()
                    .iter()
                    .try_fold(&arguments, |value, segment| match segment {
                        Segment::Seq { index } => value.get(index),
                        Segment::Map { key } => value.get(key),
                        Segment::Enum { .. } | Segment::Unknown => None,
                    });
                invalid(name, &e.path().to_string(), e.inner(), given)
            })?;
            run(server, parsed).map(Into::into)
        }),
    }
}

/// The error for arguments `name` can't take: where (`.` for the whole),
/// what is wrong, and the value given there (for an unknown argument, the
/// object holding it).
fn invalid(
    name: &str,
    path: &str,
    what: impl std::fmt::Display,
    given: Option<&serde_json::Value>,
) -> anyhow::Error {
    let mut given = given.map(ToString::to_string).unwrap_or_default();
    if given.len() > 200 {
        let end = given.floor_char_boundary(200);
        given.truncate(end);
        given.push_str("...");
    }
    match path {
        "." => anyhow::anyhow!("Invalid arguments for {name}: {what} (given {given})"),
        path => anyhow::anyhow!("Invalid arguments for {name}: {path}: {what} (given {given})"),
    }
}

/// The first place in `value` that `schema` says holds an object but
/// doesn't, with its path from `path`.
fn not_an_object<'a>(
    schema: &serde_json::Value,
    value: &'a serde_json::Value,
    path: String,
) -> Option<(String, &'a serde_json::Value)> {
    if schema.get("type") == Some(&serde_json::json!("object")) && !value.is_object() {
        return Some((path, value));
    }
    match value {
        serde_json::Value::Object(fields) => fields.iter().find_map(|(key, field)| {
            let path = match path.as_str() {
                "." => key.clone(),
                path => format!("{path}.{key}"),
            };
            not_an_object(schema.get("properties")?.get(key)?, field, path)
        }),
        serde_json::Value::Array(items) => items.iter().enumerate().find_map(|(index, item)| {
            not_an_object(schema.get("items")?, item, format!("{path}[{index}]"))
        }),
        _ => None,
    }
}

/// `A`'s JSON schema, as MCP clients read it: no `$schema` or `$ref`, no
/// title, and the tool's description in place of the type's.
fn input_schema<A: JsonSchema>() -> serde_json::Value {
    let mut schema = schemars::generate::SchemaSettings::draft2020_12()
        .with(|settings| {
            settings.meta_schema = None;
            settings.inline_subschemas = true;
        })
        .into_generator()
        .into_root_schema_for::<A>()
        .to_value();
    if let Some(schema) = schema.as_object_mut() {
        schema.remove("title");
        schema.remove("description");
        // a tool with no arguments still lists none, as clients expect
        schema
            .entry("properties")
            .or_insert_with(|| serde_json::json!({}));
    }
    schema
}

/// Every tool, in the order `tools/list` gives them.
pub(crate) static TOOLS: LazyLock<Vec<Tool>> = LazyLock::new(|| {
    vec![
        // Core tools
        tool(
            "meta_list_projects",
            "List all projects in the meta repository",
            McpServer::list_projects,
        ),
        tool(
            "meta_exec",
            "Execute a command across all meta projects (meta exec)",
            McpServer::exec,
        ),
        tool(
            "meta_get_config",
            "Get the meta repository configuration including all projects, tags, and settings",
            McpServer::get_config,
        ),
        tool(
            "meta_get_project_path",
            "Get the absolute path for a specific project",
            McpServer::get_project_path,
        ),
        // Git tools
        tool(
            "meta_git_status",
            "Get git status for all projects with structured output showing dirty/clean state, branch, and changes",
            McpServer::git_status,
        ),
        tool(
            "meta_git_pull",
            "Pull changes from remote for all projects or filtered by tag",
            McpServer::git_pull,
        ),
        tool(
            "meta_git_push",
            "Push commits to remote for all projects or filtered by tag",
            McpServer::git_push,
        ),
        tool(
            "meta_git_fetch",
            "Fetch from remotes for all projects in parallel",
            McpServer::git_fetch,
        ),
        tool(
            "meta_git_diff",
            "Get diffs across repositories showing what has changed",
            McpServer::git_diff,
        ),
        tool(
            "meta_git_branch",
            "Get branch information for all projects including current branch, tracking branch, and ahead/behind status",
            McpServer::git_branch,
        ),
        tool(
            "meta_git_add",
            "Stage files across repositories",
            McpServer::git_add,
        ),
        tool(
            "meta_git_commit",
            "Commit staged changes across repositories with a shared message",
            McpServer::git_commit,
        ),
        tool(
            "meta_git_checkout",
            "Checkout a branch across repositories",
            McpServer::git_checkout,
        ),
        tool(
            "meta_git_multi_commit",
            "Create commits with different messages for each repository. Allows tailored commit messages per project.",
            McpServer::git_multi_commit,
        ),
        // Build/test tools
        tool(
            "meta_detect_build_systems",
            "Detect build systems (Cargo, npm, make, etc.) for each project",
            McpServer::detect_build_systems,
        ),
        tool(
            "meta_run_tests",
            "Run tests across all projects using detected build systems",
            McpServer::run_tests,
        ),
        tool(
            "meta_build",
            "Build all projects using detected build systems",
            McpServer::build,
        ),
        tool(
            "meta_clean",
            "Clean build artifacts across all projects",
            McpServer::clean,
        ),
        // Discovery tools
        tool(
            "meta_search_code",
            "Search for patterns across all repositories (nested ones too) with git grep: tracked files, and untracked files .gitignore doesn't ignore. At most 50 matches per repository; an entry with more says \"truncated\": true",
            McpServer::search_code,
        ),
        tool(
            "meta_get_file_tree",
            "Get file tree structure for projects",
            McpServer::get_file_tree,
        ),
        tool(
            "meta_list_plugins",
            "List all installed meta plugins and their commands",
            McpServer::list_plugins,
        ),
        // Workspace analysis tools
        tool(
            "meta_query_repos",
            "Query repositories by state/criteria using a simple DSL: field:value conditions joined by AND (there is no OR or NOT). Examples: 'dirty:true', 'tag:backend', 'dirty:true AND branch:main', 'modified_in:24h'",
            McpServer::query_repos,
        ),
        tool(
            "meta_workspace_state",
            "Get a summary of the entire workspace state including dirty/clean counts, branches, and tags",
            McpServer::workspace_state,
        ),
        tool(
            "meta_analyze_impact",
            "Analyze what would be affected if a project changes. Returns direct and transitive dependents.",
            McpServer::analyze_impact,
        ),
        tool(
            "meta_execution_order",
            "Get topological execution order respecting dependencies. Dependencies come before dependents.",
            McpServer::execution_order,
        ),
        tool(
            "meta_batch_execute",
            "Execute a command across projects, optionally rolling back the projects it ran in if it fails",
            McpServer::batch_execute,
        ),
    ]
});

// ============================================================================
// Arguments
// ============================================================================

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoArgs {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TagArgs {
    /// Filter projects by tag (a comma-separated list selects any of them)
    tag: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectArgs {
    /// Name of the project in .meta.yaml, or . for the workspace root
    project: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExecArgs {
    /// The program and its arguments, one per item, e.g. ["git", "log", "--format=%h %s"]. They run as given, with no shell; for a shell, ask for one: ["sh", "-c", "..."]
    command: Vec<String>,
    /// Filter projects by tag (a comma-separated list selects any of them)
    tag: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct StatusArgs {
    /// Specific project to check, by its name in .meta.yaml or . for the workspace root (optional, defaults to all; not with tag)
    project: Option<String>,
    /// Filter projects by tag (a comma-separated list selects any of them)
    tag: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PullArgs {
    /// Filter projects by tag (a comma-separated list selects any of them)
    tag: Option<String>,
    /// Use rebase instead of merge (default: false)
    rebase: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiffArgs {
    /// Specific project to diff, by its name in .meta.yaml or . for the workspace root (optional, defaults to all; not with tag)
    project: Option<String>,
    /// Show only staged changes (default: false shows unstaged)
    staged: Option<bool>,
    /// Filter projects by tag (a comma-separated list selects any of them)
    tag: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AddArgs {
    /// Specific project to add files in, by its name in .meta.yaml or . for the workspace root (optional, defaults to all; not with tag)
    project: Option<String>,
    /// Paths to add, one per item, passed to git as given (default: ["."] for all changed files)
    files: Option<Vec<String>>,
    /// Filter projects by tag (a comma-separated list selects any of them)
    tag: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommitArgs {
    /// Commit message
    message: String,
    /// Specific project to commit in, by its name in .meta.yaml or . for the workspace root (optional, defaults to all with staged changes; not with tag)
    project: Option<String>,
    /// Filter projects by tag (a comma-separated list selects any of them)
    tag: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckoutArgs {
    /// Branch name to checkout
    branch: String,
    /// Create the branch if it doesn't exist (default: false)
    create: Option<bool>,
    /// Filter projects by tag (a comma-separated list selects any of them)
    tag: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct MultiCommitArgs {
    /// Array of commit objects, each specifying a project and message
    commits: Vec<ProjectCommit>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectCommit {
    /// Project name in .meta.yaml (use '.' for root repo)
    project: String,
    /// Commit message for this project
    message: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TestArgs {
    /// Filter projects by tag (a comma-separated list selects any of them)
    tag: Option<String>,
    /// Specific project to test, by its name in .meta.yaml or . for the workspace root (optional, defaults to all; not with tag)
    project: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BuildArgs {
    /// Filter projects by tag (a comma-separated list selects any of them)
    tag: Option<String>,
    /// Build in release mode (default: false)
    release: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchArgs {
    /// Search pattern: an extended regular expression (as grep -E)
    pattern: String,
    /// File glob pattern to filter (e.g., '*.rs', '*.ts')
    file_pattern: Option<String>,
    /// Filter projects by tag (a comma-separated list selects any of them)
    tag: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileTreeArgs {
    /// Specific project to get tree for, by its name in .meta.yaml or . for the workspace root (optional, defaults to all; not with tag)
    project: Option<String>,
    /// Maximum depth to traverse (default: 3)
    depth: Option<usize>,
    /// Filter projects by tag (a comma-separated list selects any of them)
    tag: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueryArgs {
    /// Query string using DSL (e.g., 'dirty:true AND tag:backend')
    query: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct BatchArgs {
    /// The program and its arguments to run in each project, one per item. They run as given, with no shell; for a shell, ask for one: ["sh", "-c", "..."]
    pub(crate) command: Vec<String>,
    /// Filter projects by tag (optional; a comma-separated list selects any of them)
    pub(crate) tag: Option<String>,
    /// If true, refuse to start unless every target project is the root of a clean git repository; on the first failure, stop and put each project the command reached back on its branch and commit from before. Its uncommitted changes and untracked files there are saved in a git stash, and the commits HEAD and the branch were on are reported, all by id in the result. Ignored files, submodules, other refs and effects outside the repositories are not undone. Projects it never reached are not touched (default: false)
    pub(crate) atomic: Option<bool>,
}

// ============================================================================
// The workspace and the projects a tool acts on
// ============================================================================

/// The workspace a tool acts in: its root, and the projects its config lists.
pub(crate) struct Workspace {
    pub(crate) dir: PathBuf,
    pub(crate) projects: Vec<ProjectInfo>,
}

/// A project a tool acts on.
pub(crate) struct Target {
    /// Its name in the config, or `.` for the workspace root
    pub(crate) name: String,
    /// Its path from the workspace root (`.` for the root)
    path: String,
    pub(crate) dir: PathBuf,
    pub(crate) tags: Vec<String>,
}

/// Which projects a tool acts on.
pub(crate) enum Scope {
    All,
    /// Those with any of these comma-separated tags
    Tags(String),
    /// The one with this name (`.` for the root)
    Project(String),
}

impl Scope {
    /// From a tool's `project` and `tag`: a blank tag list is no filter
    /// (as no `tag` at all); both at once are refused, as neither can say
    /// what the other means.
    pub(crate) fn of(project: Option<String>, tag: Option<String>) -> Result<Self> {
        let tag = tag.filter(|tags| tags.split(',').any(|tag| !tag.trim().is_empty()));
        Ok(match (project, tag) {
            (Some(_), Some(_)) => anyhow::bail!("Give 'project' or 'tag', not both"),
            (Some(project), None) => Scope::Project(project),
            (None, Some(tags)) => Scope::Tags(tags),
            (None, None) => Scope::All,
        })
    }

    fn tags(&self) -> Option<&str> {
        match self {
            Scope::Tags(tags) => Some(tags),
            _ => None,
        }
    }
}

impl Workspace {
    fn target(&self, project: &ProjectInfo) -> Target {
        Target {
            name: project.name.clone(),
            path: project.path.clone(),
            dir: self.dir.join(&project.path),
            tags: project.tags.clone(),
        }
    }

    /// The project `name` names in the config, or the workspace root for
    /// `.`. Never a path: nothing outside the workspace.
    pub(crate) fn project(&self, name: &str) -> Result<Target> {
        if name == "." {
            return Ok(Target {
                name: ".".to_string(),
                path: ".".to_string(),
                dir: self.dir.clone(),
                tags: vec![],
            });
        }
        let project = self.projects.iter().find(|p| p.name == name);
        project.map(|p| self.target(p)).ok_or_else(|| {
            anyhow::anyhow!(
                "Project '{}' not found: name a project of .meta.yaml, or . for the workspace root",
                escape(name)
            )
        })
    }

    /// The projects `scope` selects, in config order.
    pub(crate) fn select(&self, scope: &Scope) -> Result<Vec<Target>> {
        Ok(match scope {
            Scope::Project(name) => vec![self.project(name)?],
            Scope::All | Scope::Tags(_) => self
                .projects
                .iter()
                .filter(|p| scope.tags().is_none_or(|tags| has_any_tag(tags, &p.tags)))
                .map(|p| self.target(p))
                .collect(),
        })
    }
}

/// Whether `project_tags` holds any of the comma-separated `tags`, as
/// `meta --tag` selects.
fn has_any_tag(tags: &str, project_tags: &[String]) -> bool {
    tags.split(',')
        .map(str::trim)
        .any(|tag| !tag.is_empty() && project_tags.iter().any(|t| t == tag))
}

/// Whether results that each succeeded or not mean the tool failed: there
/// were some, and none succeeded.
pub(crate) fn all_failed(mut successes: impl Iterator<Item = bool>) -> bool {
    match successes.next() {
        None => false,
        Some(first) => !first && successes.all(|success| !success),
    }
}

impl McpServer {
    /// The workspace's root: the server's directory, or the one above it
    /// that holds a meta config.
    fn meta_dir(&self) -> Result<&Path> {
        self.meta_dir
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("No meta repository found"))
    }

    /// The workspace around the server's directory, its config read afresh.
    pub(crate) fn workspace(&self) -> Result<Workspace> {
        let config::Workspace { dir, projects, .. } = config::Workspace::in_dir(self.meta_dir()?)?
            .ok_or_else(|| anyhow::anyhow!("No meta config found"))?;
        Ok(Workspace { dir, projects })
    }

    /// git in `target`'s folder, when that is a repository's top level. A
    /// folder that isn't is an error: git would act on the repository
    /// around it, which may be the workspace's or one outside it.
    fn own_repo_git(&self, target: &Target) -> Result<Command> {
        let out = self
            .calls
            .output(git(&target.dir).args(["rev-parse", "--is-inside-work-tree", "--show-cdup"]))
            .context("Cannot run git rev-parse")?;
        let at = || {
            format!(
                "Project '{}' at {}",
                escape(&target.name),
                meta_core::display::escape_path(&target.dir)
            )
        };
        if !out.status.success() {
            // git's reason: no repository, or one it won't open
            anyhow::bail!("{}: {}", at(), String::from_utf8_lossy(&out.stderr).trim());
        }
        // in a work tree, with no way up to its top level: "true" and an
        // empty line (in a bare repository "false"; below the top "../")
        if out.stdout.trim_ascii() != b"true" {
            anyhow::bail!("{} is not a git repository of its own", at());
        }
        Ok(git(&target.dir))
    }

    /// `meta` in the workspace root.
    fn meta(&self, workspace: &Workspace) -> Command {
        let mut meta = Command::new("meta");
        meta.current_dir(&workspace.dir);
        meta
    }

    /// `meta --json` with `args`, in the projects `scope` selects (a project
    /// by its path, so meta can't take it for another's name). The reply is
    /// meta's result for each repo; it is an error only when meta gave none,
    /// or every repo failed.
    fn run_meta(&self, workspace: &Workspace, scope: &Scope, args: &[&str]) -> Result<Reply> {
        let mut meta = self.meta(workspace);
        meta.arg("--json");
        match scope {
            Scope::All => {}
            Scope::Tags(tags) => {
                meta.arg("--tag").arg(tags);
            }
            Scope::Project(name) => {
                let path = workspace.project(name)?.path;
                let outside = std::path::Path::new(&path).is_absolute()
                    || path.split('/').any(|part| part == "..");
                meta.arg("--include").arg(match path.as_str() {
                    "." => path,
                    // a project outside the workspace has no path from its
                    // root; meta refuses its name if another's path is it
                    _ if outside => name.clone(),
                    path => format!("./{path}"),
                });
            }
        }
        meta.args(args);
        // the command, not its arguments, which may be long
        let words: Vec<&str> = args
            .iter()
            .copied()
            .take_while(|arg| !arg.starts_with('-'))
            .collect();
        let what = format!("meta {}", words.join(" "));
        let output = self
            .calls
            .output(&mut meta)
            .with_context(|| format!("Cannot run {}", escape(&what)))?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        match serde_json::from_str::<JsonOutput>(&stdout) {
            Ok(json) => Ok(Reply {
                is_error: all_failed(json.results.iter().map(|result| result.success)),
                text: stdout,
            }),
            Err(_) if output.status.success() => Ok(stdout.into()),
            Err(_) => anyhow::bail!(
                "{} failed ({}): {}{}",
                escape(&what),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim(),
                stdout
            ),
        }
    }

    // ========================================================================
    // Core tools
    // ========================================================================

    fn list_projects(&self, args: TagArgs) -> Result<String> {
        let workspace = self.workspace()?;
        let projects = workspace.select(&Scope::of(None, args.tag)?)?;
        let mut output = format!(
            "Found {} project(s) in {}\n\n",
            projects.len(),
            workspace.dir.display()
        );
        for project in projects {
            output.push_str(&format!("- {} ({})\n", project.name, project.path));
            if !project.tags.is_empty() {
                output.push_str(&format!("  Tags: {}\n", project.tags.join(", ")));
            }
        }
        Ok(output)
    }

    fn exec(&self, args: ExecArgs) -> Result<Reply> {
        if args.command.is_empty() {
            anyhow::bail!("'command' is empty: name the program to run");
        }
        let mut meta_args = vec!["exec", "--"];
        meta_args.extend(args.command.iter().map(String::as_str));
        self.run_meta(&self.workspace()?, &Scope::of(None, args.tag)?, &meta_args)
    }

    fn get_config(&self, _args: NoArgs) -> Result<String> {
        // Found, not read as a config: a broken one is shown as it is
        let path = config::find_meta_config_in(self.meta_dir()?)?
            .ok_or_else(|| anyhow::anyhow!("No meta config found"))?;
        let content = std::fs::read_to_string(&path)?;
        Ok(format!("Config file: {}\n\n{}", path.display(), content))
    }

    fn get_project_path(&self, args: ProjectArgs) -> Result<String> {
        let project = self.workspace()?.project(&args.project)?;
        Ok(serde_json::json!({
            "project": project.name,
            "path": project.dir.display().to_string(),
            "exists": project.dir.exists()
        })
        .to_string())
    }

    // ========================================================================
    // Git tools
    // ========================================================================

    fn git_status(&self, args: StatusArgs) -> Result<Reply> {
        let scope = Scope::of(args.project, args.tag)?;
        self.run_meta(&self.workspace()?, &scope, &["git", "status"])
    }

    fn git_pull(&self, args: PullArgs) -> Result<Reply> {
        let mut git_args = vec!["git", "pull"];
        if args.rebase == Some(true) {
            git_args.push("--rebase");
        }
        self.run_meta(&self.workspace()?, &Scope::of(None, args.tag)?, &git_args)
    }

    fn git_push(&self, args: TagArgs) -> Result<Reply> {
        let scope = Scope::of(None, args.tag)?;
        self.run_meta(&self.workspace()?, &scope, &["git", "push"])
    }

    fn git_fetch(&self, args: TagArgs) -> Result<Reply> {
        let scope = Scope::of(None, args.tag)?;
        self.run_meta(&self.workspace()?, &scope, &["git", "fetch"])
    }

    fn git_diff(&self, args: DiffArgs) -> Result<Reply> {
        let workspace = self.workspace()?;
        let mut results = Vec::new();
        let mut successes = Vec::new();
        for project in workspace.select(&Scope::of(args.project, args.tag)?)? {
            if !project.dir.exists() {
                continue;
            }
            let diff = self.own_repo_git(&project).and_then(|mut cmd| {
                cmd.arg("diff");
                if args.staged == Some(true) {
                    cmd.arg("--staged");
                }
                Ok(self.calls.output(&mut cmd)?)
            });
            match diff {
                Ok(out) if out.status.success() => {
                    successes.push(true);
                    let diff = String::from_utf8_lossy(&out.stdout);
                    if !diff.is_empty() {
                        results.push(serde_json::json!({
                            "project": project.name,
                            "diff": diff
                        }));
                    }
                }
                failed => {
                    successes.push(false);
                    let error = match failed {
                        Ok(out) => String::from_utf8_lossy(&out.stderr).trim().to_string(),
                        Err(e) => format!("{e:#}"),
                    };
                    results.push(serde_json::json!({ "project": project.name, "error": error }));
                }
            }
        }
        Ok(Reply {
            text: serde_json::to_string_pretty(&results)?,
            is_error: all_failed(successes.into_iter()),
        })
    }

    fn git_branch(&self, args: TagArgs) -> Result<String> {
        let workspace = self.workspace()?;
        let mut results = Vec::new();
        for project in workspace.select(&Scope::of(None, args.tag)?)? {
            if !project.dir.exists() {
                continue;
            }
            // a work tree git refuses (`core.bare=true`) still has its refs;
            // what git can't read at all is unknown, never level with main
            let status = RepoStatus::read(&project.dir)
                .or_else(|e| RepoStatus::refs(&project.dir).and_then(|refs| refs.ok_or(e)));
            results.push(match status {
                Ok(status) => {
                    let (ahead, behind) = status.ahead_behind.unwrap_or_default();
                    serde_json::json!({
                        "project": project.name,
                        "branch": status.branch.unwrap_or_else(|| "HEAD".to_string()),
                        "tracking": status.upstream.filter(|upstream| !upstream.is_empty()),
                        "ahead": ahead,
                        "behind": behind
                    })
                }
                Err(e) => serde_json::json!({
                    "project": project.name,
                    "branch": null,
                    "tracking": null,
                    "ahead": null,
                    "behind": null,
                    "error": format!("{e:#}")
                }),
            });
        }
        Ok(serde_json::to_string_pretty(&results)?)
    }

    fn git_add(&self, args: AddArgs) -> Result<Reply> {
        let files = match args.files {
            None => vec![".".to_string()],
            Some(files) if files.is_empty() => {
                anyhow::bail!("'files' is empty: name the paths to add")
            }
            Some(files) => files,
        };
        let mut meta_args = vec!["git", "add", "--"];
        meta_args.extend(files.iter().map(String::as_str));
        let scope = Scope::of(args.project, args.tag)?;
        let reply = self.run_meta(&self.workspace()?, &scope, &meta_args)?;
        Ok(Reply {
            text: format!("Staged files: {}\n{}", files.join(", "), reply.text),
            ..reply
        })
    }

    fn git_commit(&self, args: CommitArgs) -> Result<Reply> {
        let scope = Scope::of(args.project, args.tag)?;
        self.run_meta(
            &self.workspace()?,
            &scope,
            &["git", "commit", "-m", &args.message],
        )
    }

    fn git_checkout(&self, args: CheckoutArgs) -> Result<Reply> {
        let workspace = self.workspace()?;
        // A name git would read as an option never gets to git; the `--` after
        // it keeps git from taking it for a file to restore when no branch
        // has that name
        let check = self
            .calls
            .output(git(&workspace.dir).args(["check-ref-format", "--branch", &args.branch]))
            .context("Cannot run git check-ref-format")?;
        if !check.status.success() {
            // git's reason: a name it refuses, or a config it can't read
            match String::from_utf8_lossy(&check.stderr).trim() {
                "" => anyhow::bail!("'{}' is not a valid branch name", escape(&args.branch)),
                why => anyhow::bail!("{}", escape(why)),
            }
        }
        let mut git_args = vec!["git", "checkout"];
        if args.create == Some(true) {
            git_args.push("-b");
        }
        git_args.extend([args.branch.as_str(), "--"]);
        self.run_meta(&workspace, &Scope::of(None, args.tag)?, &git_args)
    }

    fn git_multi_commit(&self, args: MultiCommitArgs) -> Result<Reply> {
        #[derive(Serialize)]
        struct CommitResult {
            project: String,
            success: bool,
            message: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            error: Option<String>,
        }

        let workspace = self.workspace()?;
        // Every project named, before anything is committed
        let targets = args
            .commits
            .iter()
            .map(|commit| workspace.project(&commit.project))
            .collect::<Result<Vec<_>>>()?;
        let mut results = Vec::new();
        for (commit, target) in args.commits.into_iter().zip(targets) {
            let error = if !target.dir.exists() {
                Some(format!(
                    "Project path does not exist: {}",
                    meta_core::display::escape_path(&target.dir)
                ))
            } else {
                let commit = self.own_repo_git(&target).and_then(|mut cmd| {
                    Ok(self
                        .calls
                        .output(cmd.args(["commit", "-m", &commit.message]))?)
                });
                match commit {
                    Ok(out) if out.status.success() => None,
                    // git says why on stderr, or ("nothing to commit") on stdout
                    Ok(out) => Some(match String::from_utf8_lossy(&out.stderr).trim() {
                        "" => String::from_utf8_lossy(&out.stdout).trim().to_string(),
                        stderr => stderr.to_string(),
                    }),
                    Err(e) => Some(format!("{e:#}")),
                }
            };
            results.push(CommitResult {
                project: commit.project,
                success: error.is_none(),
                message: commit.message,
                error,
            });
        }
        let succeeded = results.iter().filter(|r| r.success).count();
        Ok(Reply {
            is_error: all_failed(results.iter().map(|r| r.success)),
            text: serde_json::to_string_pretty(&serde_json::json!({
                "summary": {
                    "total": results.len(),
                    "succeeded": succeeded,
                    "failed": results.len() - succeeded
                },
                "results": results,
            }))?,
        })
    }

    // ========================================================================
    // Build/test tools
    // ========================================================================

    fn detect_build_systems(&self, args: TagArgs) -> Result<String> {
        let workspace = self.workspace()?;
        let mut results = Vec::new();
        for project in workspace.select(&Scope::of(None, args.tag)?)? {
            let dir = &project.dir;
            if !dir.exists() {
                continue;
            }
            let has = |file: &str| dir.join(file).exists();
            let build_systems: Vec<&str> = [
                ("cargo", has("Cargo.toml")),
                ("npm", has("package.json")),
                ("make", has("Makefile") || has("makefile")),
                ("go", has("go.mod")),
                ("maven", has("pom.xml")),
                ("gradle", has("build.gradle") || has("build.gradle.kts")),
                ("python", has("pyproject.toml") || has("setup.py")),
            ]
            .into_iter()
            .filter_map(|(name, found)| found.then_some(name))
            .collect();
            results.push(serde_json::json!({
                "project": project.name,
                "path": project.path,
                "build_systems": build_systems
            }));
        }
        Ok(serde_json::to_string_pretty(&results)?)
    }

    fn run_tests(&self, args: TestArgs) -> Result<Reply> {
        let targets = self
            .workspace()?
            .select(&Scope::of(args.project, args.tag)?)?;
        self.run_each(&targets, |dir| repo_command(dir, RepoAction::Test))
    }

    fn build(&self, args: BuildArgs) -> Result<Reply> {
        let workspace = self.workspace()?;
        // Root builds remain included even when a tag filter selects child repos.
        let mut targets = vec![workspace.project(".")?];
        targets.extend(workspace.select(&Scope::of(None, args.tag)?)?);
        let release = args.release == Some(true);
        self.run_each(&targets, |dir| {
            repo_command(dir, RepoAction::Build { release })
        })
    }

    fn clean(&self, args: TagArgs) -> Result<Reply> {
        let targets = self.workspace()?.select(&Scope::of(None, args.tag)?)?;
        self.run_each(&targets, |dir| {
            let has = |file: &str| dir.join(file).is_file();
            // npm has no clean; removing node_modules is left to the user
            if has("Cargo.toml") {
                Some(("cargo", vec!["clean"]))
            } else if has("package.json") {
                None
            } else if has("go.mod") {
                Some(("go", vec!["clean"]))
            } else if has("Makefile") {
                Some(("make", vec!["clean"]))
            } else {
                None
            }
        })
    }

    /// Runs, in each of `targets` that exists, the command `command` picks
    /// for its directory (none: skipped), in the project's own repository
    /// whatever a git hook around this server set.
    fn run_each(
        &self,
        targets: &[Target],
        command: impl Fn(&Path) -> Option<(&'static str, Vec<&'static str>)>,
    ) -> Result<Reply> {
        #[derive(Serialize)]
        struct RepoRun {
            project: String,
            command: String,
            success: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            stdout: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            stderr: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            error: Option<String>,
        }

        let mut results = Vec::new();
        for target in targets {
            if !target.dir.exists() {
                continue;
            }
            let Some((program, program_args)) = command(&target.dir) else {
                continue;
            };
            let mut cmd = Command::new(program);
            meta_core::git::clear_repo_env(&mut cmd)
                .args(&program_args)
                .current_dir(&target.dir);
            let line = format!("{program} {}", program_args.join(" "))
                .trim_end()
                .to_string();
            results.push(match self.calls.output(&mut cmd) {
                Ok(out) => RepoRun {
                    project: target.name.clone(),
                    command: line,
                    success: out.status.success(),
                    stdout: Some(String::from_utf8_lossy(&out.stdout).into_owned()),
                    stderr: Some(String::from_utf8_lossy(&out.stderr).into_owned()),
                    error: None,
                },
                Err(e) => RepoRun {
                    project: target.name.clone(),
                    command: line,
                    success: false,
                    stdout: None,
                    stderr: None,
                    error: Some(format!("Cannot run {program}: {e}")),
                },
            });
        }
        Ok(Reply {
            is_error: all_failed(results.iter().map(|r| r.success)),
            text: serde_json::to_string_pretty(&results)?,
        })
    }

    // ========================================================================
    // Discovery tools
    // ========================================================================

    fn search_code(&self, args: SearchArgs) -> Result<Reply> {
        /// The most matches given for one project.
        const MAX_MATCHES: usize = 50;

        let workspace = self.workspace()?;
        let scope = Scope::of(None, args.tag)?;

        // Each project once, the ones nested in a meta project too (as
        // `meta project list --recursive`), named by its path in the
        // workspace. A project whose nested config can't be read is an
        // error of its own.
        let mut targets = Vec::new();
        search_targets(&workspace.dir, "", &workspace.projects, &mut targets);

        let mut results = Vec::new();
        // whether a project gave an answer, matches or none
        let mut answered = false;

        for SearchTarget {
            path: name,
            tags,
            nested,
        } in targets
        {
            let nested = match nested {
                Ok(nested) => nested,
                // whatever the tags: the projects it hides may have them
                Err(error) => {
                    results.push(serde_json::json!({ "project": name, "error": error }));
                    continue;
                }
            };
            let project_path = workspace.dir.join(&name);
            if scope.tags().is_some_and(|tag| !has_any_tag(tag, &tags)) || !project_path.exists() {
                continue;
            }

            // git's search never looks in .git or at what .gitignore leaves
            // out, and takes the pattern after -e, never as an option. A
            // project that is no repository of its own is a plain directory.
            // The user's git config doesn't change the results: no colour or
            // column, extended regular expressions, file names as they are.
            let mut cmd = git(&project_path);
            cmd.args([
                "-c",
                "core.quotePath=false",
                "grep",
                "--no-color",
                "--no-column",
                "--no-recurse-submodules",
                "-E",
                "-n",
                "-I",
            ]);
            match meta_core::gitdir::common_dir(&project_path) {
                Ok(Some(_)) => cmd.arg("--untracked"),
                // only the directory's own .gitignore files: `git` keeps it
                // from the workspace repo around it, which may ignore the
                // whole project, unless the path can't be a ceiling
                Ok(None) if meta_core::git::ceiling(&project_path).is_none() => {
                    results.push(serde_json::json!({
                        "project": name,
                        "error": format!(
                            "Cannot search {}: its path contains the path list separator",
                            meta_core::display::escape_path(&project_path)
                        )
                    }));
                    continue;
                }
                Ok(None) => cmd.args(["--no-index", "--exclude-standard"]),
                Err(e) => {
                    results.push(serde_json::json!({
                        "project": name,
                        "error": format!("{e:#}")
                    }));
                    continue;
                }
            };
            cmd.arg("-e").arg(&args.pattern).arg("--");
            // a file name pattern, in any directory
            if let Some(fp) = &args.file_pattern {
                cmd.arg(format!(":(glob)**/{fp}"));
            }
            // the projects nested in it are searched as their own: by their
            // path, and where it leads when it is a link inside this project
            let real = |path: &std::path::Path| path.canonicalize().ok();
            for path in &nested {
                cmd.arg(format!(":(exclude,literal){path}"));
                let resolved = real(&project_path.join(path)).zip(real(&project_path));
                if let Some(target) = resolved
                    .as_ref()
                    .and_then(|(child, here)| child.strip_prefix(here).ok())
                    .filter(|target| *target != std::path::Path::new(path))
                {
                    cmd.arg(format!(":(exclude,literal){}", target.display()));
                }
            }

            // Past the cap, git's output is closed, which stops it
            let (out, truncated) = match self.calls.output_lines(&mut cmd, MAX_MATCHES) {
                Ok(out) => out,
                Err(e) => {
                    results.push(serde_json::json!({
                        "project": name,
                        "error": format!("Cannot run git grep: {e}")
                    }));
                    continue;
                }
            };
            // Exit 1 is no match. git also says on stderr what it couldn't
            // read while it goes on, so matches may come with an error.
            let stdout = String::from_utf8_lossy(&out.stdout);
            let matches: Vec<&str> = stdout.lines().collect();
            let stderr = String::from_utf8_lossy(&out.stderr);
            let error = match (stderr.trim(), out.status.code()) {
                ("", _) if truncated => None,
                ("", Some(0 | 1)) => None,
                ("", _) => Some(format!("git grep failed: {}", out.status)),
                (stderr, _) => Some(stderr.to_string()),
            };
            answered |= error.is_none() || !matches.is_empty();
            if matches.is_empty() && error.is_none() {
                continue;
            }
            let mut entry = serde_json::Map::new();
            entry.insert("project".to_string(), name.into());
            if !matches.is_empty() {
                entry.insert("matches".to_string(), matches.into());
            }
            if truncated {
                entry.insert("truncated".to_string(), true.into());
            }
            if let Some(error) = error {
                entry.insert("error".to_string(), error.into());
            }
            results.push(entry.into());
        }

        // no answer: whatever is listed is an error
        Ok(Reply {
            is_error: !answered && !results.is_empty(),
            text: serde_json::to_string_pretty(&results)?,
        })
    }

    fn get_file_tree(&self, args: FileTreeArgs) -> Result<String> {
        let depth = args.depth.unwrap_or(3);
        let workspace = self.workspace()?;
        let mut results = Vec::new();
        for project in workspace.select(&Scope::of(args.project, args.tag)?)? {
            if !project.dir.exists() {
                continue;
            }
            results.push(serde_json::json!({
                "project": project.name,
                "tree": build_file_tree(&project.dir, depth, 0)?
            }));
        }
        Ok(serde_json::to_string_pretty(&results)?)
    }

    fn list_plugins(&self, _args: NoArgs) -> Result<String> {
        let mut meta = self.meta(&self.workspace()?);
        meta.args(["plugin", "list", "--json"]);
        let output = self
            .calls
            .output(&mut meta)
            .context("Failed to list plugins")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to list plugins:\n{stdout}\n{stderr}");
        }
        Ok(stdout.into_owned())
    }

    // ========================================================================
    // Workspace analysis tools
    // ========================================================================

    fn query_repos(&self, args: QueryArgs) -> Result<String> {
        let query = Query::parse(&args.query)?;
        let workspace = self.workspace()?;
        let mut matching = Vec::new();
        for project in workspace.select(&Scope::All)? {
            if !project.dir.exists() {
                continue;
            }
            let state = RepoState::collect(&project.name, &project.dir, &project.tags)?;
            if state.matches(&query) {
                matching.push(state);
            }
        }
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "query": args.query,
            "matches": matching.len(),
            "projects": matching
        }))?)
    }

    fn workspace_state(&self, _args: NoArgs) -> Result<String> {
        let workspace = self.workspace()?;
        let mut repo_states = Vec::new();
        for project in workspace.select(&Scope::All)? {
            if !project.dir.exists() {
                continue;
            }
            if let Ok(state) = RepoState::collect(&project.name, &project.dir, &project.tags) {
                repo_states.push(state);
            }
        }
        Ok(serde_json::to_string_pretty(&WorkspaceState::from_repos(
            &repo_states,
        ))?)
    }

    fn dependency_graph(&self) -> Result<dependency_graph::DependencyGraph> {
        let projects = self.workspace()?.projects;
        dependency_graph::DependencyGraph::build(
            projects
                .into_iter()
                .map(ProjectDependencies::from)
                .collect(),
        )
    }

    fn analyze_impact(&self, args: ProjectArgs) -> Result<String> {
        // The root depends on nothing and nothing on it: no dependents
        let project = self.workspace()?.project(&args.project)?;
        Ok(serde_json::to_string_pretty(
            &self.dependency_graph()?.analyze_impact(&project.name),
        )?)
    }

    fn execution_order(&self, args: TagArgs) -> Result<String> {
        let scope = Scope::of(None, args.tag)?;
        let graph = self.dependency_graph()?;
        let order: Vec<&str> = graph
            .execution_order()?
            .into_iter()
            .filter(|name| {
                scope.tags().is_none_or(|tags| {
                    graph
                        .get_project(name)
                        .is_some_and(|project| has_any_tag(tags, &project.tags))
                })
            })
            .collect();
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "execution_order": order,
            "count": order.len(),
            "tag_filter": scope.tags()
        }))?)
    }
}

/// A project for `meta_search_code`: its path in the workspace, its tags,
/// and the paths, from it, of the projects nested in it.
struct SearchTarget {
    path: String,
    tags: Vec<String>,
    nested: Result<Vec<String>, String>,
}

/// Every project of `projects` (listed by the config in `dir`) and the ones
/// nested in them, parents first, with paths under `prefix`. Each nested
/// config is read on its own, so one that can't be read is only its
/// project's error.
fn search_targets(dir: &Path, prefix: &str, projects: &[ProjectInfo], out: &mut Vec<SearchTarget>) {
    for project in projects {
        let path = config::nested_path(prefix, &project.path);
        // one level: this project's own nested config
        let nested = config::walk_projects(dir, std::slice::from_ref(project), Some(1))
            .map(|nodes| {
                nodes
                    .into_iter()
                    .flat_map(|node| node.children)
                    .map(|child| child.info)
                    .collect::<Vec<_>>()
            })
            .map_err(|e| format!("{e:#}"));
        out.push(SearchTarget {
            path: path.clone(),
            tags: project.tags.clone(),
            nested: nested
                .as_ref()
                .map(|nested| nested.iter().map(|child| child.path.clone()).collect())
                .map_err(Clone::clone),
        });
        if let Ok(nested) = nested {
            search_targets(&dir.join(&project.path), &path, &nested, out);
        }
    }
}

fn build_file_tree(
    path: &Path,
    max_depth: usize,
    current_depth: usize,
) -> Result<serde_json::Value> {
    if current_depth >= max_depth {
        return Ok(serde_json::json!(null));
    }

    if path.is_file() {
        return Ok(serde_json::json!({
            "type": "file",
            "name": path.file_name().unwrap_or_default().to_string_lossy()
        }));
    }

    let mut children = Vec::new();

    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            let name = entry_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            // Skip hidden files and common ignored directories
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }

            // A link is listed, not followed: it may lead back up the tree
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                children.push(serde_json::json!({
                    "type": "symlink",
                    "name": name
                }));
            } else if file_type.is_dir() {
                children.push(serde_json::json!({
                    "type": "directory",
                    "name": name,
                    "children": build_file_tree(&entry_path, max_depth, current_depth + 1)?
                }));
            } else {
                children.push(serde_json::json!({
                    "type": "file",
                    "name": name
                }));
            }
        }
    }

    Ok(serde_json::Value::Array(children))
}

/// npm is a `.cmd` script on Windows, which `Command` doesn't find by its bare name.
const NPM: &str = if cfg!(windows) { "npm.cmd" } else { "npm" };

/// What to run in a repo: build it (`release` for cargo) or test it.
#[derive(Clone, Copy)]
enum RepoAction {
    Build { release: bool },
    Test,
}

/// Choose build/test commands from the same per-repo build-system precedence.
fn repo_command(path: &Path, action: RepoAction) -> Option<(&'static str, Vec<&'static str>)> {
    let build = matches!(action, RepoAction::Build { .. });
    if path.join("Cargo.toml").is_file() {
        Some(match action {
            RepoAction::Build { release: true } => ("cargo", vec!["build", "--release"]),
            RepoAction::Build { release: false } => ("cargo", vec!["build"]),
            RepoAction::Test => ("cargo", vec!["test"]),
        })
    } else if path.join("package.json").is_file() {
        Some((
            NPM,
            if build {
                vec!["run", "build"]
            } else {
                vec!["test"]
            },
        ))
    } else if path.join("go.mod").is_file() {
        Some(("go", vec![if build { "build" } else { "test" }, "./..."]))
    } else if path.join("Makefile").is_file() {
        Some(("make", if build { vec![] } else { vec!["test"] }))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{call, git, init_repo, json, server, workspace};
    use serde_json::json;

    /// Each tool's arguments (the contract MCP clients rely on), and the
    /// smallest arguments it takes.
    fn contract() -> Vec<(&'static str, Vec<&'static str>, serde_json::Value)> {
        let tag = || vec!["tag"];
        vec![
            ("meta_list_projects", tag(), json!({})),
            (
                "meta_exec",
                vec!["command", "tag"],
                json!({"command": ["true"]}),
            ),
            ("meta_get_config", vec![], json!({})),
            (
                "meta_get_project_path",
                vec!["project"],
                json!({"project": "a"}),
            ),
            ("meta_git_status", vec!["project", "tag"], json!({})),
            ("meta_git_pull", vec!["rebase", "tag"], json!({})),
            ("meta_git_push", tag(), json!({})),
            ("meta_git_fetch", tag(), json!({})),
            ("meta_git_diff", vec!["project", "staged", "tag"], json!({})),
            ("meta_git_branch", tag(), json!({})),
            ("meta_git_add", vec!["files", "project", "tag"], json!({})),
            (
                "meta_git_commit",
                vec!["message", "project", "tag"],
                json!({"message": "m"}),
            ),
            (
                "meta_git_checkout",
                vec!["branch", "create", "tag"],
                json!({"branch": "b"}),
            ),
            (
                "meta_git_multi_commit",
                vec!["commits"],
                json!({"commits": [{"project": "a", "message": "m"}]}),
            ),
            ("meta_detect_build_systems", tag(), json!({})),
            ("meta_run_tests", vec!["project", "tag"], json!({})),
            ("meta_build", vec!["release", "tag"], json!({})),
            ("meta_clean", tag(), json!({})),
            (
                "meta_search_code",
                vec!["file_pattern", "pattern", "tag"],
                json!({"pattern": "p"}),
            ),
            (
                "meta_get_file_tree",
                vec!["depth", "project", "tag"],
                json!({}),
            ),
            ("meta_list_plugins", vec![], json!({})),
            (
                "meta_query_repos",
                vec!["query"],
                json!({"query": "dirty:true"}),
            ),
            ("meta_workspace_state", vec![], json!({})),
            (
                "meta_analyze_impact",
                vec!["project"],
                json!({"project": "a"}),
            ),
            ("meta_execution_order", tag(), json!({})),
            (
                "meta_batch_execute",
                vec!["atomic", "command", "tag"],
                json!({"command": ["true"]}),
            ),
        ]
    }

    #[test]
    fn every_tool_takes_exactly_the_arguments_it_advertises() {
        let contract = contract();
        let names: Vec<_> = TOOLS.iter().map(|tool| tool.name).collect();
        let listed: Vec<_> = contract.iter().map(|(name, ..)| *name).collect();
        assert_eq!(names, listed);
        // No workspace: a call whose arguments are read stops there, and
        // runs nothing
        let server = McpServer::in_dir(None);
        for (tool, (_, arguments, smallest)) in TOOLS.iter().zip(contract) {
            let schema = &tool.input_schema;
            let mut properties: Vec<_> = schema["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            properties.sort_unstable();
            assert_eq!(properties, arguments, "{}", tool.name);
            let required: Vec<_> = smallest.as_object().unwrap().keys().collect();
            let mut advertised: Vec<_> = schema["required"]
                .as_array()
                .map(|required| required.iter().map(|r| r.as_str().unwrap()).collect())
                .unwrap_or_default();
            advertised.sort_unstable();
            assert_eq!(advertised, required, "{}", tool.name);
            assert_eq!(schema["additionalProperties"], false, "{}", tool.name);

            // each optional argument may be null, as clients send it
            let mut all = smallest.clone();
            for argument in &arguments {
                all.as_object_mut()
                    .unwrap()
                    .entry(*argument)
                    .or_insert(serde_json::Value::Null);
            }
            let err = call(&server, tool.name, all)
                .err()
                .map(|e| format!("{e:#}"));
            assert!(
                !err.as_deref()
                    .unwrap_or_default()
                    .contains("Invalid arguments"),
                "{}: {err:?}",
                tool.name
            );

            // arguments by position, as serde would read a struct
            let err = format!(
                "{:#}",
                call(&server, tool.name, json!([[], null])).err().unwrap()
            );
            assert!(err.contains("not an object"), "{err}");
            if tool.name == "meta_git_multi_commit" {
                let positional = json!({"commits": [["a", "m"]]});
                let err = format!("{:#}", call(&server, tool.name, positional).err().unwrap());
                assert!(err.contains("commits[0]: not an object"), "{err}");
            }

            // an argument it doesn't advertise: named, with its value
            let mut unknown = smallest.clone();
            unknown["tags"] = json!("nomatch");
            let err = format!("{:#}", call(&server, tool.name, unknown).err().unwrap());
            assert!(
                err.contains(&format!("Invalid arguments for {}", tool.name))
                    && err.contains("tags")
                    && err.contains("nomatch"),
                "{err}"
            );
        }
    }

    #[test]
    fn a_wrong_type_is_an_error_naming_the_tool_the_argument_and_the_value() {
        let (server, _tmp) = workspace();
        for (tool, arguments, named) in [
            ("meta_list_projects", json!({"tag": 123}), ["tag", "123"]),
            (
                "meta_get_project_path",
                json!({"project": ["a"]}),
                ["project", "[\"a\"]"],
            ),
            (
                "meta_git_checkout",
                json!({"branch": "b", "create": "yes"}),
                ["create", "\"yes\""],
            ),
            (
                "meta_batch_execute",
                json!({"command": ["touch", "ran"], "atomic": "true"}),
                ["atomic", "\"true\""],
            ),
            ("meta_get_file_tree", json!({"depth": -1}), ["depth", "-1"]),
            (
                "meta_git_multi_commit",
                json!({"commits": [{"project": "a", "message": 7}]}),
                ["commits[0].message", "7"],
            ),
        ] {
            let err = format!("{:#}", call(&server, tool, arguments).err().unwrap());
            assert!(err.contains(tool), "{err}");
            for name in named {
                assert!(err.contains(name), "{err}");
            }
        }
        // the batch never ran without its rollback
        assert!(!_tmp.path().join("a/ran").exists());
    }

    #[test]
    fn a_project_is_a_name_of_the_config_or_the_root() {
        let (server, tmp) = server("projects:\n  a: {path: dir/a, tags: [x]}\n");
        init_repo(&tmp.path().join("dir/a"));
        let path = |project: &str| {
            json(call(
                &server,
                "meta_get_project_path",
                json!({"project": project}),
            ))["path"]
                .as_str()
                .map(PathBuf::from)
        };
        assert_eq!(path("a"), Some(tmp.path().join("dir/a")));
        assert_eq!(path("."), Some(tmp.path().to_path_buf()));
        // by its name, at its own path
        std::fs::write(tmp.path().join("dir/a/README"), "changed").unwrap();
        let diff = json(call(&server, "meta_git_diff", json!({"project": "a"})));
        assert_eq!(diff.as_array().unwrap().len(), 1, "{diff}");
        assert!(
            diff[0]["diff"].as_str().unwrap().contains("+changed"),
            "{diff}"
        );
        let impact = json(call(
            &server,
            "meta_analyze_impact",
            json!({"project": "."}),
        ));
        assert_eq!(impact["total_affected"], 0, "{impact}");
        let outside = tmp.path().join("..").to_string_lossy().into_owned();
        // not a path, not an unknown name, in any tool that takes one
        for project in ["dir/a", "../x", outside.as_str(), "no-such"] {
            for tool in [
                "meta_get_project_path",
                "meta_analyze_impact",
                "meta_run_tests",
                "meta_get_file_tree",
                "meta_git_diff",
                "meta_git_status",
                "meta_git_add",
            ] {
                let err = call(&server, tool, json!({"project": project})).err();
                assert!(
                    err.is_some_and(|e| e.to_string().contains("not found")),
                    "{tool} {project}"
                );
            }
        }
        // project and tag at once: neither wins silently; a blank tag is no tag
        let err = call(
            &server,
            "meta_get_file_tree",
            json!({"project": "a", "tag": "x"}),
        );
        assert!(err
            .err()
            .is_some_and(|e| e.to_string().contains("not both")));
        let tree = json(call(
            &server,
            "meta_get_file_tree",
            json!({"project": "a", "tag": " , "}),
        ));
        assert_eq!(tree[0]["project"], "a", "{tree}");
    }

    #[test]
    fn multi_commit_commits_only_in_projects_of_the_workspace() {
        let (server, tmp) = server("projects:\n  a: {path: dir/a}\n");
        let a = tmp.path().join("dir/a");
        init_repo(&a);
        let outside = tempfile::tempdir().unwrap();
        init_repo(outside.path());
        for dir in [&a, &outside.path().to_path_buf()] {
            std::fs::write(dir.join("README"), "staged").unwrap();
            git(dir, &["add", "README"]);
        }
        let ws_relative = format!(
            "../{}",
            outside.path().file_name().unwrap().to_string_lossy()
        );
        let absolute = outside.path().to_string_lossy().into_owned();
        for project in [ws_relative.as_str(), absolute.as_str()] {
            // refused before anything is committed, even the valid entry
            let commits = json!({"commits": [
                {"project": "a", "message": "in a"},
                {"project": project, "message": "outside"}
            ]});
            let err = call(&server, "meta_git_multi_commit", commits)
                .err()
                .unwrap();
            assert!(err.to_string().contains("not found"), "{err}");
        }
        assert_eq!(git(outside.path(), &["log", "--format=%s"]), "init\n");
        assert_eq!(git(&a, &["log", "--format=%s"]), "init\n");

        // by name, at the project's own path; the root has nothing staged,
        // and git's reason (on stdout) is its error
        let commits = json!({"commits": [
            {"project": "a", "message": "in a"},
            {"project": ".", "message": "in the root"}
        ]});
        init_repo(tmp.path());
        let reply = call(&server, "meta_git_multi_commit", commits).unwrap();
        assert!(!reply.is_error, "{}", reply.text);
        let result: serde_json::Value = serde_json::from_str(&reply.text).unwrap();
        assert_eq!(git(&a, &["log", "-1", "--format=%s"]), "in a\n");
        assert_eq!(result["results"][0]["success"], true, "{result}");
        assert!(
            result["results"][1]["error"]
                .as_str()
                .is_some_and(|e| e.contains("nothing")),
            "{result}"
        );
        // every commit failed: the call failed
        let commits = json!({"commits": [{"project": "a", "message": "again"}]});
        assert!(
            call(&server, "meta_git_multi_commit", commits)
                .unwrap()
                .is_error
        );
    }

    #[test]
    fn git_branch_reports_unknown_when_a_projects_git_file_points_nowhere() {
        let (server, tmp) = server("projects:\n  broken: {}\n");
        init_repo(tmp.path());
        std::fs::create_dir(tmp.path().join("broken")).unwrap();
        std::fs::write(
            tmp.path().join("broken/.git"),
            format!("gitdir: {}\n", tmp.path().join("gone").display()),
        )
        .unwrap();

        let result = json(call(&server, "meta_git_branch", json!({})));
        let project = result
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["project"] == "broken")
            .unwrap();
        assert_eq!(
            project.get("branch"),
            Some(&serde_json::Value::Null),
            "{result}"
        );
        assert_eq!(
            project.get("ahead"),
            Some(&serde_json::Value::Null),
            "{result}"
        );
        assert!(
            project["error"].as_str().is_some_and(|e| e.contains("git")),
            "{result}"
        );
    }

    #[test]
    fn git_acts_only_in_a_project_that_is_its_own_repository() {
        // a plain folder, in a workspace root that is a repository with
        // work staged
        let (server, tmp) = server("projects:\n  plain: {}\n");
        init_repo(tmp.path());
        std::fs::create_dir(tmp.path().join("plain")).unwrap();
        std::fs::write(tmp.path().join("staged"), "root work").unwrap();
        git(tmp.path(), &["add", "staged"]);
        let commits = json!({"commits": [{"project": "plain", "message": "m"}]});
        let reply = call(&server, "meta_git_multi_commit", commits).unwrap();
        assert!(
            reply.is_error && reply.text.contains("not a git repository"),
            "{}",
            reply.text
        );
        let diff = call(
            &server,
            "meta_git_diff",
            json!({"project": "plain", "staged": true}),
        )
        .unwrap();
        assert!(
            diff.is_error && !diff.text.contains("root work"),
            "{}",
            diff.text
        );
        assert_eq!(git(tmp.path(), &["log", "--format=%s"]), "init\n");

        // a repository git won't open: its reason, not a guess
        std::fs::write(tmp.path().join("plain/.git"), "gitdir: /nonexistent\n").unwrap();
        let commits = json!({"commits": [{"project": "plain", "message": "m"}]});
        let reply = call(&server, "meta_git_multi_commit", commits).unwrap();
        // git's own words, which change between versions
        assert!(reply.is_error, "{}", reply.text);
        let result: serde_json::Value = serde_json::from_str(&reply.text).unwrap();
        let error = result["results"][0]["error"].as_str().unwrap_or_default();
        let reason = error
            .strip_prefix("Project 'plain' at ")
            .unwrap_or_default();
        assert!(
            reason.contains(": fatal:") && !reason.contains("of its own"),
            "{error}"
        );

        // a plain folder in a workspace inside a bare repository
        let bare = tempfile::tempdir().unwrap();
        git(bare.path(), &["init", "-q", "--bare"]);
        let ws = bare.path().join("ws");
        std::fs::create_dir_all(ws.join("plain")).unwrap();
        std::fs::write(ws.join(".meta.yaml"), "projects:\n  plain: {}\n").unwrap();
        let server = McpServer::in_dir(Some(ws));
        let diff = call(&server, "meta_git_diff", json!({"project": "plain"})).unwrap();
        // or, under safe.bareRepository=explicit, git's own refusal
        let refused = diff.text.contains("not a git repository of its own")
            || diff.text.contains("Project 'plain' at ") && diff.text.contains(": fatal:");
        assert!(diff.is_error && refused, "{}", diff.text);

        // a workspace root that isn't a repository, inside one that is
        let outer = tempfile::tempdir().unwrap();
        init_repo(outer.path());
        std::fs::write(outer.path().join("wip"), "outer work").unwrap();
        git(outer.path(), &["add", "wip"]);
        let ws = outer.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        std::fs::write(ws.join(".meta.yaml"), "projects: {}\n").unwrap();
        let server = McpServer::in_dir(Some(ws));
        let commits = json!({"commits": [{"project": ".", "message": "m"}]});
        assert!(
            call(&server, "meta_git_multi_commit", commits)
                .unwrap()
                .is_error
        );
        assert_eq!(git(outer.path(), &["log", "--format=%s"]), "init\n");
    }

    /// The check reads no path from git, so any folder name works.
    #[test]
    fn a_repository_with_any_name_is_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let mut names = vec![std::ffi::OsString::from("name")];
        // Windows drops a trailing space from a folder's name
        #[cfg(unix)]
        names.push(std::ffi::OsString::from("ends in a space "));
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::ffi::OsStrExt;
            names.push(std::ffi::OsStr::from_bytes(b"byte \xff").to_os_string());
        }
        for name in names {
            let ws = tmp.path().join(name);
            init_repo(&ws);
            std::fs::write(ws.join(".meta.yaml"), "projects: {}\n").unwrap();
            std::fs::write(ws.join("README"), "changed").unwrap();
            let server = McpServer::in_dir(Some(ws.clone()));
            let diff = json(call(&server, "meta_git_diff", json!({"project": "."})));
            assert_eq!(diff.as_array().unwrap().len(), 1, "{ws:?}: {diff}");
        }
    }

    /// A server keeps to its own workspace: when that one's config is gone,
    /// it never takes the workspace around it for its own.
    #[test]
    fn a_workspace_whose_config_is_gone_is_not_its_parents() {
        let (_, tmp) = server("projects:\n  child: {meta: true}\n");
        let child = tmp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join(".meta.yaml"), "projects: {}\n").unwrap();
        let server = McpServer::in_dir(Some(child.clone()));
        assert!(call(&server, "meta_list_projects", json!({})).is_ok());
        std::fs::remove_file(child.join(".meta.yaml")).unwrap();
        let err = call(&server, "meta_list_projects", json!({}))
            .err()
            .unwrap();
        assert!(err.to_string().contains("No meta config found"), "{err}");
    }

    #[test]
    fn checkout_says_why_git_refused_the_branch() {
        let (server, tmp) = server("projects: {}\n");
        init_repo(tmp.path());
        let refused = |branch: &str| {
            let reply = call(&server, "meta_git_checkout", json!({"branch": branch}));
            format!("{:#}", reply.err().unwrap())
        };
        assert!(refused("--orphan=evil").contains("is not a valid branch name"));
        // git repeats the name; a control character in it comes back escaped
        assert!(!refused("a\nb").contains('\n'));
        // a valid name, but a config git can't read: git's words, not a guess
        std::fs::write(tmp.path().join(".git/config"), "[broken").unwrap();
        let err = refused("feature/demo");
        assert!(
            err.contains("fatal:") && !err.contains("not a valid branch name"),
            "{err}"
        );
    }

    #[test]
    fn search_gives_at_most_fifty_matches_per_project_and_says_so() {
        let (server, tmp) = server("projects:\n  a: {}\n  b: {}\n");
        for (project, lines) in [("a", 60), ("b", 50)] {
            let dir = tmp.path().join(project);
            init_repo(&dir);
            std::fs::write(dir.join("f"), "hit\n".repeat(lines)).unwrap();
        }
        let found = json(call(&server, "meta_search_code", json!({"pattern": "hit"})));
        assert_eq!(found[0]["project"], "a", "{found}");
        assert_eq!(found[0]["matches"].as_array().unwrap().len(), 50);
        assert_eq!(found[0]["truncated"], true);
        assert!(found[0].get("error").is_none(), "{found}");
        assert_eq!(found[1]["matches"].as_array().unwrap().len(), 50);
        assert!(found[1].get("truncated").is_none(), "{found}");

        // no match is an answer; a search git refuses everywhere is a failure
        let none = call(&server, "meta_search_code", json!({"pattern": "absent"})).unwrap();
        assert!(!none.is_error, "{}", none.text);
        let refused = call(&server, "meta_search_code", json!({"pattern": "["})).unwrap();
        assert!(refused.is_error, "{}", refused.text);
    }

    #[test]
    #[cfg(unix)]
    fn build_uses_make_for_root_and_child_and_fails_only_when_all_do() {
        let (server, tmp) = server(r#"{"projects":{"child":{"tags":["selected"]}}}"#);
        let child = tmp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        for path in [tmp.path(), child.as_path()] {
            std::fs::write(path.join("Makefile"), "all:\n\t@echo made > built\n").unwrap();
        }
        let results = json(call(
            &server,
            "meta_build",
            json!({"tag": "selected", "release": true}),
        ));
        assert_eq!(results.as_array().unwrap().len(), 2);
        assert!(results
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["command"] == "make" && r["success"] == true));
        assert!(tmp.path().join("built").exists());
        assert!(child.join("built").exists());

        // one failed: its result says so, the others' still count
        let failing = "all:\n\t@echo build-failed >&2\n\t@exit 1\n";
        std::fs::write(child.join("Makefile"), failing).unwrap();
        let reply = call(&server, "meta_build", json!({})).unwrap();
        assert!(!reply.is_error, "{}", reply.text);
        assert!(reply.text.contains("build-failed"), "{}", reply.text);
        // all failed: the call failed
        std::fs::write(tmp.path().join("Makefile"), failing).unwrap();
        let reply = call(&server, "meta_build", json!({})).unwrap();
        assert!(reply.is_error, "{}", reply.text);
    }

    #[test]
    fn build_tags_select_like_meta_tag() {
        let tags = ["red".to_string()];
        assert!(has_any_tag("red", &tags));
        assert!(has_any_tag("blue, red", &tags));
        assert!(!has_any_tag("blue", &tags));
    }

    #[test]
    fn file_tree_lists_links_without_following_them() {
        let (server, tmp) =
            server("projects:\n  a: {tags: [x]}\n  b: {tags: [y]}\n  c: {tags: [z]}\n");
        for project in ["a", "b", "c"] {
            std::fs::create_dir_all(tmp.path().join(project).join("src")).unwrap();
        }
        // a link back up the tree, which a walk that follows it never leaves
        // (Windows makes one only with Developer Mode)
        let link = tmp.path().join("a/src/up");
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink("..", &link).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir("..", &link).is_ok();

        // any of the tags, as meta --tag
        let tree = json(call(
            &server,
            "meta_get_file_tree",
            json!({"tag": "x,y", "depth": 100}),
        ));
        assert_eq!(tree.as_array().unwrap().len(), 2, "{tree}");
        if linked {
            assert_eq!(
                tree[0]["tree"][0]["children"],
                json!([{"type": "symlink", "name": "up"}]),
                "{tree}"
            );
        }

        // a blank list is no filter, in every tool
        let tree = json(call(&server, "meta_get_file_tree", json!({"tag": " , "})));
        assert_eq!(tree.as_array().unwrap().len(), 3, "{tree}");

        // null, as clients send for an option they don't use, is the default
        json(call(&server, "meta_get_file_tree", json!({"depth": null})));
    }

    #[test]
    fn execution_order_selects_any_of_the_tags() {
        let (server, _tmp) =
            server("projects:\n  a: {tags: [x]}\n  b: {tags: [y]}\n  c: {tags: [z]}\n");
        let order = json(call(
            &server,
            "meta_execution_order",
            json!({"tag": "x, y"}),
        ));
        assert_eq!(order["count"], 2, "{order}");
    }

    #[test]
    fn build_and_test_share_detection_and_keep_release() {
        let tmp = tempfile::tempdir().unwrap();
        for (file, program, build, test) in [
            ("Makefile", "make", vec![], vec!["test"]),
            (
                "go.mod",
                "go",
                vec!["build", "./..."],
                vec!["test", "./..."],
            ),
            ("package.json", NPM, vec!["run", "build"], vec!["test"]),
            (
                "Cargo.toml",
                "cargo",
                vec!["build", "--release"],
                vec!["test"],
            ),
        ] {
            std::fs::write(tmp.path().join(file), "").unwrap();
            assert_eq!(
                repo_command(tmp.path(), RepoAction::Build { release: true }),
                Some((program, build))
            );
            assert_eq!(
                repo_command(tmp.path(), RepoAction::Test),
                Some((program, test))
            );
        }
    }

    #[test]
    fn get_config_ignores_meta_directory_and_shows_a_broken_config() {
        let (server, tmp) = server("projects: {}\n");
        std::fs::create_dir_all(tmp.path().join(".meta/skills")).unwrap();
        let result = call(&server, "meta_get_config", json!({})).unwrap().text;
        assert!(result.contains(".meta.yaml"));
        assert!(result.contains("projects: {}"));
        std::fs::write(
            tmp.path().join(".meta.yaml"),
            "projects: {}\nunknown_top: 1\n",
        )
        .unwrap();
        let reply = call(&server, "meta_get_config", json!({})).unwrap();
        assert!(
            !reply.is_error && reply.text.contains("unknown_top: 1"),
            "{}",
            reply.text
        );
    }
}
