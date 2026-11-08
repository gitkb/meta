//! The tools: each one's arguments as a type, its schema derived from that
//! type, and what it does.

use crate::McpServer;
use anyhow::{Context, Result};
use meta_core::config::{self, ProjectInfo};
use meta_core::dependency_graph::{self, ProjectDependencies};
use meta_core::git::RepoStatus;
use meta_core::query::{Query, RepoState, WorkspaceState};
use meta_core::repo_filter::{repo_path, Filters};
use meta_core::worktree::Recursive;
use meta_plugin::display::escape;
use meta_plugin::{Envelope, ProjectResult};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    pub(crate) config_path: PathBuf,
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
    /// Those with any of these tags, read as `meta --tag` reads them
    Tags(Filters),
    /// The one with this name (`.` for the root)
    Project(String),
}

impl Scope {
    /// From a tool's `project` and `tag`, the tag read and checked as
    /// `meta --tag` does it: one that names no tag (blanks alone), or a tag
    /// none of `known` is, is refused (`read_from` names the config). Both
    /// at once are refused, as neither can say what the other means.
    fn of(
        project: Option<String>,
        tag: Option<String>,
        known: &[String],
        read_from: &str,
    ) -> Result<Self> {
        let tags = tag.as_deref().map(Filters::tags).transpose()?;
        Ok(match (project, tags) {
            (Some(_), Some(_)) => anyhow::bail!("Give 'project' or 'tag', not both"),
            (Some(project), None) => Scope::Project(project),
            (None, Some(tags)) => {
                let filters = Filters {
                    tags,
                    ..Filters::default()
                };
                filters.check_tags(known, read_from, "")?;
                Scope::Tags(filters)
            }
            (None, None) => Scope::All,
        })
    }

    fn tags(&self) -> Option<&Filters> {
        match self {
            Scope::Tags(tags) => Some(tags),
            _ => None,
        }
    }
}

impl Workspace {
    /// The projects a tool's `project` and `tag` select ([`Scope::of`]),
    /// with this config's tags known.
    pub(crate) fn scope(&self, project: Option<String>, tag: Option<String>) -> Result<Scope> {
        let known: Vec<String> = self.projects.iter().flat_map(|p| p.tags.clone()).collect();
        Scope::of(project, tag, &known, &self.read_from())
    }

    /// Where its tags are read from, as `meta --tag`'s errors say it.
    fn read_from(&self) -> String {
        format!(
            " (from {})",
            meta_plugin::display::escape_path(&self.config_path)
        )
    }

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
        project
            .map(|p| self.target(p))
            .ok_or_else(|| not_found(name))
    }

    /// The projects `scope` selects, in config order.
    pub(crate) fn select(&self, scope: &Scope) -> Result<Vec<Target>> {
        Ok(match scope {
            Scope::Project(name) => vec![self.project(name)?],
            Scope::All | Scope::Tags(_) => self
                .projects
                .iter()
                .filter(|p| scope.tags().is_none_or(|tags| tags.tagged(&p.tags)))
                .map(|p| self.target(p))
                .collect(),
        })
    }
}

/// The repos `meta -r` reaches that `filters` select and that exist, in its
/// order, as the read tools see them: what each is called, its folder, its
/// tags.
fn reached(
    recursive: &Recursive,
    filters: &Filters,
) -> Result<Vec<(String, PathBuf, Vec<String>)>> {
    Ok(recursive
        .select(filters)?
        .into_iter()
        .filter(|repo| repo.dir.exists())
        .map(|repo| {
            (
                recursive.name_of(&repo),
                repo.dir.clone(),
                repo.tags().to_vec(),
            )
        })
        .collect())
}

/// The error for a `project` no repo is.
fn not_found(name: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "Project '{}' not found: name a project of .meta.yaml, or . for the workspace root",
        escape(name)
    )
}

/// The `--tag` filter a tool's `tag` asks for; one naming no tag is refused.
fn tag_filter(tag: &str) -> Result<Filters> {
    Ok(Filters {
        tags: Filters::tags(tag)?,
        ..Filters::default()
    })
}

/// The `--include` value for the project `name`, at `path` from where meta
/// names its repos: its path, so meta can't take it for another's name.
fn include(name: &str, path: &str) -> String {
    let outside = Path::new(path).is_absolute() || path.split('/').any(|part| part == "..");
    match path {
        "." => path.to_string(),
        // a project outside the workspace has no path from its root; meta
        // refuses its name if another's path is it
        _ if outside => name.to_string(),
        path => format!("./{path}"),
    }
}

/// The `--include` value for `project` under `meta -r`, which names repos
/// from its root: `.` or a name, a nested repo's whole path or its bare name
/// when that is unique.
fn nested_include(recursive: &Recursive, project: &str) -> Result<String> {
    let path_of = |dir: &Path| repo_path(dir, &recursive.root);
    if project == "." {
        return Ok(include(".", &path_of(&recursive.dir)));
    }
    let (graph, dirs) = tree_graph(recursive)?;
    let key = graph.find(project)?.ok_or_else(|| not_found(project))?;
    let dir = dirs.get(key).ok_or_else(|| not_found(project))?;
    Ok(include(project, &path_of(dir)))
}

/// The dependency graph of what `meta -r` reaches (in a set, its members
/// only), keyed as `meta -r project dependents` keys it, with each key's
/// folder.
fn tree_graph(
    recursive: &Recursive,
) -> Result<(dependency_graph::DependencyGraph, HashMap<String, PathBuf>)> {
    let (mut projects, dirs) =
        dependency_graph::in_tree(&recursive.dir, &recursive.config.projects, None)?;
    if recursive.in_set() {
        let members: Vec<PathBuf> = recursive
            .select(&Filters::default())?
            .iter()
            .map(|m| real(&m.dir))
            .collect();
        projects.retain(|p| {
            dirs.get(&p.name)
                .is_some_and(|dir| members.contains(&real(dir)))
        });
    }
    Ok((dependency_graph::DependencyGraph::build(projects), dirs))
}

/// `dir` as the file system names it, or as given.
fn real(dir: &Path) -> PathBuf {
    dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf())
}

/// A tool's answer from meta's Envelope: an error when every repo failed.
fn reply(envelope: Envelope) -> Result<Reply> {
    Ok(Reply {
        is_error: all_failed(envelope.results.iter().map(|row| !row.is_failure())),
        text: serde_json::to_string(&envelope)?,
    })
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
        let config::Workspace {
            dir,
            config_path,
            mut projects,
            ..
        } = config::Workspace::in_dir(self.meta_dir()?)?
            .ok_or_else(|| anyhow::anyhow!("No meta config found"))?;
        // a set with no root checkout holds the top config's projects at
        // their places in a set (one outside the workspace at its name)
        if self.rootless_set.is_some() {
            for project in &mut projects {
                project.path = project.set_place().to_string();
            }
        }
        let dir = self.rootless_set.clone().unwrap_or(dir);
        // in a set, the projects it holds
        if let Some(members) = self.members()? {
            projects.retain(|p| members.contains(&real(&dir.join(&p.path))));
        }
        Ok(Workspace {
            dir,
            config_path,
            projects,
        })
    }

    /// The folders of the members of the set the server is in; none outside
    /// one.
    fn members(&self) -> Result<Option<Vec<PathBuf>>> {
        let Some(set) = &self.set else {
            return Ok(None);
        };
        let config = meta_core::worktree::set_config(set, None)?
            .map(|path| config::Workspace::load(&path))
            .transpose()?;
        let members = meta_core::worktree::set_members(set, config.as_ref(), &Filters::default())?;
        Ok(Some(members.iter().map(|m| real(&m.dir)).collect()))
    }

    /// Where meta, run from where the server started, names repos from:
    /// the set's root, else the workspace's.
    fn names_from(&self, workspace: &Workspace) -> PathBuf {
        self.set.clone().unwrap_or_else(|| workspace.dir.clone())
    }

    /// What `meta -r` reaches from where the server started.
    fn recursive(&self) -> Result<Recursive> {
        Recursive::from(&self.start)?.ok_or_else(|| anyhow::anyhow!("No meta repository found"))
    }

    /// meta in `dir`, with `--json` and `args`, in the projects `scope`
    /// selects (a project by its path, so meta can't take it for another's
    /// name). Its Envelope; an error only when meta gave no result and said
    /// why.
    fn call_meta(&self, workspace: &Workspace, scope: &Scope, args: &[&str]) -> Result<Envelope> {
        let options = match scope {
            Scope::All => Vec::new(),
            Scope::Tags(tags) => vec!["--tag".to_string(), tags.tags.join(",")],
            Scope::Project(name) => {
                let dir = workspace.project(name)?.dir;
                let path = repo_path(&dir, &self.names_from(workspace));
                vec!["--include".to_string(), include(name, &path)]
            }
        };
        self.call_with(&options, args)
    }

    /// `meta --json <options> <args>` from where the server started.
    fn call_with(&self, options: &[String], args: &[&str]) -> Result<Envelope> {
        let meta = crate::run::meta()?.cwd(&self.start).args(options);
        // the command, not its arguments, which may be long
        let words: Vec<&str> = args
            .iter()
            .copied()
            .take_while(|arg| !arg.starts_with('-'))
            .collect();
        let what = format!("meta {}", words.join(" "));
        let envelope = self
            .calls
            .meta(meta.args(args))
            .with_context(|| format!("Cannot run {}", escape(&what)))?;
        match envelope {
            // meta failed before any project gave a result: its error
            Envelope {
                error: Some(error),
                results,
                ..
            } if results.is_empty() => anyhow::bail!(
                "{} failed: {}{}",
                escape(&what),
                error.message,
                error
                    .hint
                    .map(|hint| format!(" ({hint})"))
                    .unwrap_or_default()
            ),
            envelope => Ok(envelope),
        }
    }

    /// [`McpServer::call_meta`], answered with its Envelope: an error when
    /// every repo failed.
    fn run_meta(&self, workspace: &Workspace, scope: &Scope, args: &[&str]) -> Result<Reply> {
        reply(self.call_meta(workspace, scope, args)?)
    }

    /// `meta git <args>` in the project `target` alone: its result, or why
    /// there is none (git's words, else meta's).
    fn git_in(
        &self,
        workspace: &Workspace,
        target: &Target,
        args: &[&str],
    ) -> Result<String, String> {
        let mut git_args = vec!["git"];
        git_args.extend(args);
        let envelope = self
            .call_meta(workspace, &Scope::Project(target.name.clone()), &git_args)
            .map_err(|e| format!("{e:#}"))?;
        match envelope.results.into_iter().next() {
            Some(ProjectResult {
                error: None,
                stdout,
                ..
            }) => Ok(stdout.unwrap_or_default()),
            Some(ProjectResult {
                error,
                stdout,
                stderr,
                ..
            }) => {
                let said = [stderr, stdout]
                    .into_iter()
                    .flatten()
                    .map(|text| text.trim().to_string())
                    .find(|text| !text.is_empty());
                Err(said
                    .or(error.map(|error| error.message))
                    .unwrap_or_default())
            }
            None => Err(envelope
                .skipped
                .into_iter()
                .map(|skipped| skipped.reason)
                .chain(envelope.message)
                .next()
                .unwrap_or_else(|| "The call of meta gave no result".to_string())),
        }
    }

    // ========================================================================
    // Core tools
    // ========================================================================

    fn list_projects(&self, args: TagArgs) -> Result<String> {
        let workspace = self.workspace()?;
        let projects = workspace.select(&workspace.scope(None, args.tag)?)?;
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
        let workspace = self.workspace()?;
        self.run_meta(&workspace, &workspace.scope(None, args.tag)?, &meta_args)
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
        let recursive = self.recursive()?;
        let mut options = vec!["-r".to_string()];
        match (args.project, args.tag) {
            (Some(_), Some(_)) => anyhow::bail!("Give 'project' or 'tag', not both"),
            (Some(project), None) => {
                options.extend([
                    "--include".to_string(),
                    nested_include(&recursive, &project)?,
                ]);
            }
            (None, Some(tag)) => {
                let filters = tag_filter(&tag)?;
                recursive.select(&filters)?;
                options.extend(["--tag".to_string(), filters.tags.join(",")]);
            }
            (None, None) => {}
        }
        reply(self.call_with(&options, &["git", "status"])?)
    }

    fn git_pull(&self, args: PullArgs) -> Result<Reply> {
        let mut git_args = vec!["git", "pull"];
        if args.rebase == Some(true) {
            git_args.push("--rebase");
        }
        let workspace = self.workspace()?;
        self.run_meta(&workspace, &workspace.scope(None, args.tag)?, &git_args)
    }

    fn git_push(&self, args: TagArgs) -> Result<Reply> {
        let workspace = self.workspace()?;
        let scope = workspace.scope(None, args.tag)?;
        self.run_meta(&workspace, &scope, &["git", "push"])
    }

    fn git_fetch(&self, args: TagArgs) -> Result<Reply> {
        let workspace = self.workspace()?;
        let scope = workspace.scope(None, args.tag)?;
        self.run_meta(&workspace, &scope, &["git", "fetch"])
    }

    fn git_diff(&self, args: DiffArgs) -> Result<Reply> {
        let workspace = self.workspace()?;
        let diff_args: &[&str] = match args.staged {
            Some(true) => &["diff", "--staged"],
            _ => &["diff"],
        };
        let mut results = Vec::new();
        let mut successes = Vec::new();
        for project in workspace.select(&workspace.scope(args.project, args.tag)?)? {
            if !project.dir.exists() {
                continue;
            }
            let diff = self.git_in(&workspace, &project, diff_args);
            successes.push(diff.is_ok());
            match diff {
                Ok(diff) if diff.is_empty() => {}
                Ok(diff) => results.push(serde_json::json!({
                    "project": project.name,
                    "diff": diff
                })),
                Err(error) => {
                    results.push(serde_json::json!({ "project": project.name, "error": error }))
                }
            }
        }
        Ok(Reply {
            text: serde_json::to_string_pretty(&results)?,
            is_error: all_failed(successes.into_iter()),
        })
    }

    fn git_branch(&self, args: TagArgs) -> Result<String> {
        let filters = match args.tag {
            Some(tag) => tag_filter(&tag)?,
            None => Filters::default(),
        };
        let recursive = self.recursive()?;
        let mut results = Vec::new();
        for (project, dir, _) in reached(&recursive, &filters)? {
            // a work tree git refuses (`core.bare=true`) still has its
            // branch; what git can't read at all is unknown, never level
            // with main
            let status = RepoStatus::read(&dir).ok();
            let read_work_tree = status.is_some();
            let refs = status.or_else(|| RepoStatus::refs(&dir).ok().flatten());
            results.push(match refs {
                Some(refs) if refs.branch.is_some() || read_work_tree => {
                    let (ahead, behind) = refs.ahead_behind.unwrap_or_default();
                    serde_json::json!({
                        "project": project,
                        "branch": refs.branch.unwrap_or_else(|| "HEAD".to_string()),
                        "tracking": refs.upstream.filter(|upstream| !upstream.is_empty()),
                        "ahead": ahead,
                        "behind": behind
                    })
                }
                _ => serde_json::json!({
                    "project": project,
                    "branch": null,
                    "tracking": null,
                    "ahead": null,
                    "behind": null,
                    "error": "git can't read its branch (see `meta git status`)"
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
        let workspace = self.workspace()?;
        let scope = workspace.scope(args.project, args.tag)?;
        let reply = self.run_meta(&workspace, &scope, &meta_args)?;
        Ok(Reply {
            text: format!("Staged files: {}\n{}", files.join(", "), reply.text),
            ..reply
        })
    }

    fn git_commit(&self, args: CommitArgs) -> Result<Reply> {
        let workspace = self.workspace()?;
        let scope = workspace.scope(args.project, args.tag)?;
        self.run_meta(&workspace, &scope, &["git", "commit", "-m", &args.message])
    }

    fn git_checkout(&self, args: CheckoutArgs) -> Result<Reply> {
        let workspace = self.workspace()?;
        // A name git would read as an option never gets to git; the `--` after
        // it keeps git from taking it for a file to restore when no branch
        // has that name
        let check = self
            .calls
            .output(crate::run::git(&workspace.dir).args([
                "check-ref-format",
                "--branch",
                &args.branch,
            ]))
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
        self.run_meta(&workspace, &workspace.scope(None, args.tag)?, &git_args)
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
                    meta_plugin::display::escape_path(&target.dir)
                ))
            } else {
                self.git_in(&workspace, &target, &["commit", "-m", &commit.message])
                    .err()
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
        for project in workspace.select(&workspace.scope(None, args.tag)?)? {
            let dir = &project.dir;
            if !dir.exists() {
                continue;
            }
            let build_systems = meta_core::query::detect_build_systems(dir);
            results.push(serde_json::json!({
                "project": project.name,
                "path": project.path,
                "build_systems": build_systems
            }));
        }
        Ok(serde_json::to_string_pretty(&results)?)
    }

    fn run_tests(&self, args: TestArgs) -> Result<Reply> {
        let workspace = self.workspace()?;
        let targets = workspace.select(&workspace.scope(args.project, args.tag)?)?;
        self.run_each(&targets, |dir| repo_command(dir, RepoAction::Test))
    }

    fn build(&self, args: BuildArgs) -> Result<Reply> {
        let workspace = self.workspace()?;
        // Root builds remain included even when a tag filter selects child repos.
        let mut targets = vec![workspace.project(".")?];
        targets.extend(workspace.select(&workspace.scope(None, args.tag)?)?);
        let release = args.release == Some(true);
        self.run_each(&targets, |dir| {
            repo_command(dir, RepoAction::Build { release })
        })
    }

    fn clean(&self, args: TagArgs) -> Result<Reply> {
        let workspace = self.workspace()?;
        let targets = workspace.select(&workspace.scope(None, args.tag)?)?;
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
            let mut cmd = crate::run::in_dir(program, &target.dir);
            cmd.args(&program_args);
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

    /// The projects of the workspace in `dir` (their paths under `prefix`)
    /// that are no repository of their own, and why, as `meta project
    /// check` says; its error when it gave no answer.
    fn not_repos(
        &self,
        dir: &Path,
        prefix: &str,
    ) -> Result<std::collections::HashMap<String, String>, String> {
        let check = crate::run::meta().and_then(|meta| {
            let meta = meta.cwd(dir).args(["project", "check"]);
            self.calls.meta(meta)
        });
        let envelope = check.map_err(|e| format!("Cannot run meta project check: {e:#}"))?;
        match envelope.error {
            Some(error) if envelope.results.is_empty() => return Err(error.message),
            _ => {}
        }
        Ok(envelope
            .results
            .into_iter()
            .filter_map(|row| {
                let data = row.data?;
                if data.get("state")?.as_str()? != "not_a_repo" {
                    return None;
                }
                let path = normal(&config::nested_path(prefix, &row.project?));
                let reason = data.get("reason")?.as_str()?.to_string();
                Some((path, reason))
            })
            .collect())
    }

    fn search_code(&self, args: SearchArgs) -> Result<Reply> {
        /// The most matches given for one project.
        const MAX_MATCHES: usize = 50;

        let workspace = self.workspace()?;

        // Each project once, the ones nested in a meta project too (as
        // `meta -r project list`), named by its path in the
        // workspace. A project whose nested config can't be read is an
        // error of its own.
        let mut targets = Vec::new();
        search_targets(&workspace.dir, "", &workspace.projects, &mut targets);
        // the tags `meta -r --tag` knows: the nested projects' too
        let known: Vec<String> = targets.iter().flat_map(|t| t.tags.clone()).collect();
        let scope = Scope::of(None, args.tag, &known, &workspace.read_from())?;

        let mut results = Vec::new();
        // whether a project gave an answer, matches or none
        let mut answered = false;
        // what `meta project check` says of each workspace's projects
        let mut checks = std::collections::HashMap::new();

        for SearchTarget {
            path: name,
            level,
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
            if scope.tags().is_some_and(|filters| !filters.tagged(&tags)) || !project_path.exists()
            {
                continue;
            }

            // A project that is no repository of its own is a plain
            // directory; one whose `.git` git won't open is an error.
            let not_repos = checks
                .entry(level.clone())
                .or_insert_with(|| self.not_repos(&workspace.dir.join(&level), &level));
            let own_repo = match not_repos {
                Err(error) => Err(error.clone()),
                Ok(not_repos) => match not_repos.get(&normal(&name)) {
                    None => Ok(true),
                    Some(_) if project_path.join(".git").symlink_metadata().is_err() => Ok(false),
                    Some(why) => Err(why.clone()),
                },
            };
            let own_repo = match own_repo {
                Ok(own_repo) => own_repo,
                Err(error) => {
                    results.push(serde_json::json!({ "project": name, "error": error }));
                    continue;
                }
            };

            // git's search never looks in .git or at what .gitignore leaves
            // out, and takes the pattern after -e, never as an option. The
            // user's git config doesn't change the results: no colour or
            // column, extended regular expressions, file names as they are.
            let mut cmd = crate::run::git(&project_path);
            if !own_repo {
                // a git directory that isn't one: git reads only the
                // directory's own .gitignore files, never the workspace
                // repo's around it, which may ignore the whole project
                let mut git_dir = std::ffi::OsString::from("--git-dir=");
                git_dir.push(project_path.join(".git"));
                cmd.arg(git_dir);
            }
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
            match own_repo {
                true => cmd.arg("--untracked"),
                false => cmd.args(["--no-index", "--exclude-standard"]),
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
        for project in workspace.select(&workspace.scope(args.project, args.tag)?)? {
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
        let meta = crate::run::meta()?
            .cwd(&self.start)
            .args(["plugin", "list"]);
        let envelope = self.calls.meta(meta).context("Failed to list plugins")?;
        if let Some(error) = &envelope.error {
            anyhow::bail!("Failed to list plugins: {}", error.message);
        }
        Ok(serde_json::to_string(&envelope)?)
    }

    // ========================================================================
    // Workspace analysis tools
    // ========================================================================

    fn query_repos(&self, args: QueryArgs) -> Result<String> {
        let query = Query::parse(&args.query)?;
        let recursive = self.recursive()?;
        let mut matching = Vec::new();
        for (name, dir, tags) in reached(&recursive, &Filters::default())? {
            let state = RepoState::collect(&name, &dir, &tags)?;
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
        let recursive = self.recursive()?;
        let mut repo_states = Vec::new();
        for (name, dir, tags) in reached(&recursive, &Filters::default())? {
            if let Ok(state) = RepoState::collect(&name, &dir, &tags) {
                repo_states.push(state);
            }
        }
        Ok(serde_json::to_string_pretty(&WorkspaceState::from_repos(
            &repo_states,
        ))?)
    }

    fn dependency_graph(&self) -> Result<dependency_graph::DependencyGraph> {
        let projects = self.workspace()?.projects;
        Ok(dependency_graph::DependencyGraph::build(
            projects
                .into_iter()
                .map(ProjectDependencies::from)
                .collect(),
        ))
    }

    fn analyze_impact(&self, args: ProjectArgs) -> Result<String> {
        let (graph, _) = tree_graph(&self.recursive()?)?;
        // The root depends on nothing and nothing on it: no dependents
        let key = match args.project.as_str() {
            "." => ".",
            project => graph.find(project)?.ok_or_else(|| not_found(project))?,
        };
        Ok(serde_json::to_string_pretty(&graph.analyze_impact(key))?)
    }

    fn execution_order(&self, args: TagArgs) -> Result<String> {
        let scope = self.workspace()?.scope(None, args.tag)?;
        let graph = self.dependency_graph()?;
        let order: Vec<&str> = graph
            .execution_order()?
            .into_iter()
            .filter(|name| {
                scope.tags().is_none_or(|tags| {
                    graph
                        .get_project(name)
                        .is_some_and(|project| tags.tagged(&project.tags))
                })
            })
            .collect();
        Ok(serde_json::to_string_pretty(&serde_json::json!({
            "execution_order": order,
            "count": order.len(),
            "tag_filter": scope.tags().map(|tags| tags.tags.join(","))
        }))?)
    }
}

/// A project for `meta_search_code`: its path in the workspace, its tags,
/// and the paths, from it, of the projects nested in it.
struct SearchTarget {
    path: String,
    /// The path of the workspace whose config lists it (empty for the root)
    level: String,
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
            level: prefix.to_string(),
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

/// `path` with no `.` in it, its parts joined by `/`.
fn normal(path: &str) -> String {
    let parts: Vec<_> = Path::new(path)
        .components()
        .filter(|part| *part != std::path::Component::CurDir)
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts.join("/")
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
    use crate::test_support::{call, json, server, workspace};
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
        let server = McpServer::nowhere();
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
        let sandbox = meta_plugin::testing::isolated!();
        let (server, _tmp) = workspace(&sandbox);
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
        assert!(!_tmp.join("a/ran").exists());
    }

    #[test]
    fn a_project_is_a_name_of_the_config_or_the_root() {
        let sandbox = meta_plugin::testing::isolated!();
        let (server, tmp) = server(&sandbox, "projects:\n  a: {path: dir/a, tags: [x]}\n");
        sandbox.repo(&tmp.join("dir/a"));
        let path = |project: &str| {
            json(call(
                &server,
                "meta_get_project_path",
                json!({"project": project}),
            ))["path"]
                .as_str()
                .map(PathBuf::from)
        };
        assert_eq!(path("a"), Some(tmp.join("dir/a")));
        assert_eq!(path("."), Some(tmp.clone()));
        let impact = json(call(
            &server,
            "meta_analyze_impact",
            json!({"project": "."}),
        ));
        assert_eq!(impact["total_affected"], 0, "{impact}");
        let outside = tmp.join("..").to_string_lossy().into_owned();
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
        // project and tag at once: neither wins silently; a blank tag is
        // refused, as `meta --tag` refuses it
        let err = call(
            &server,
            "meta_get_file_tree",
            json!({"project": "a", "tag": "x"}),
        );
        assert!(err
            .err()
            .is_some_and(|e| e.to_string().contains("not both")));
        let err = call(
            &server,
            "meta_get_file_tree",
            json!({"project": "a", "tag": " , "}),
        );
        assert!(err
            .err()
            .is_some_and(|e| e.to_string().contains("names no tag")));
    }

    /// `(names of the repos a read tool lists)`, sorted.
    fn listed(rows: &serde_json::Value, key: &str) -> Vec<String> {
        let mut names: Vec<String> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row[key].as_str().unwrap().to_string())
            .collect();
        names.sort();
        names
    }

    const REACHED: [&str; 6] = [
        ".",
        "platform",
        "platform/api",
        "platform/core",
        "platform/svc",
        "web",
    ];

    /// On main these tools saw the top level only: no root, nothing nested.
    #[test]
    fn the_read_tools_see_what_meta_r_sees() {
        let sandbox = meta_plugin::testing::isolated!();
        crate::test_support::use_built_meta();
        let root = crate::test_support::nested(&sandbox);
        let server = McpServer::at(root.clone()).unwrap();
        let dirty = |query: &str| {
            json(call(&server, "meta_query_repos", json!({"query": query})))["projects"].clone()
        };
        // an untracked file in the root, and a modified one nested
        std::fs::write(root.join("stray"), "x").unwrap();
        std::fs::write(root.join("platform/api/README"), "changed").unwrap();
        assert_eq!(listed(&dirty("dirty:true"), "name"), [".", "platform/api"]);
        let state = json(call(&server, "meta_workspace_state", json!({})));
        assert_eq!(state["total_projects"], 6, "{state}");
        assert_eq!(state["dirty_projects"], 2, "{state}");

        assert_eq!(
            listed(&dirty("tag:deep"), "name"),
            ["platform/api", "platform/core", "platform/svc"]
        );
        assert_eq!(listed(&dirty("tag:none"), "name"), Vec::<String>::new());
        assert!(call(&server, "meta_query_repos", json!({"query": "tag:"})).is_err());

        let branches = call(&server, "meta_git_branch", json!({}));
        assert_eq!(listed(&json(branches), "project"), REACHED);
        let status = json(call(&server, "meta_git_status", json!({})));
        assert_eq!(listed(&status["results"], "project"), REACHED, "{status}");

        for tool in ["meta_git_status", "meta_git_branch"] {
            for tag in ["no-such", " , "] {
                assert!(
                    call(&server, tool, json!({"tag": tag})).is_err(),
                    "{tool} {tag}"
                );
            }
        }
        let tagged = json(call(&server, "meta_git_branch", json!({"tag": "deep"})));
        assert_eq!(
            listed(&tagged, "project"),
            ["platform/api", "platform/core", "platform/svc"]
        );
        let tagged = json(call(&server, "meta_git_status", json!({"tag": "deep"})));
        assert_eq!(tagged["results"].as_array().unwrap().len(), 3, "{tagged}");
        for project in ["platform/api", "api"] {
            let one = json(call(
                &server,
                "meta_git_status",
                json!({"project": project}),
            ));
            assert_eq!(
                listed(&one["results"], "project"),
                ["platform/api"],
                "{one}"
            );
        }
    }

    #[test]
    fn impact_reaches_through_nesting_by_path_or_unique_name() {
        let sandbox = meta_plugin::testing::isolated!();
        let root = crate::test_support::nested(&sandbox);
        let server = McpServer::at(root.clone()).unwrap();
        for project in ["platform/core", "core"] {
            let impact = json(call(
                &server,
                "meta_analyze_impact",
                json!({"project": project}),
            ));
            assert_eq!(
                impact["direct_dependents"],
                json!(["platform/api"]),
                "{impact}"
            );
            assert_eq!(
                impact["transitive_dependents"],
                json!(["platform/svc"]),
                "{impact}"
            );
            assert_eq!(impact["total_affected"], 2, "{impact}");
        }
        // two repos of that name: refused, not guessed
        std::fs::write(
            root.join(".meta.yaml"),
            r#"{"projects":{"web":"x","platform":{"repo":"x","meta":true},"other":{"repo":"x","meta":true}}}"#,
        )
        .unwrap();
        sandbox.repo(&root.join("other/core"));
        std::fs::write(
            root.join("other/.meta.yaml"),
            r#"{"projects":{"core":"x"}}"#,
        )
        .unwrap();
        let err = call(&server, "meta_analyze_impact", json!({"project": "core"}));
        assert!(err
            .err()
            .is_some_and(|e| e.to_string().contains("ambiguous")));
    }

    /// In a set: its members only, from its root or a nested member; a
    /// set with no root checkout acts on itself, not on the primary.
    #[test]
    fn inside_a_set_the_tools_see_and_act_on_the_set() {
        let sandbox = meta_plugin::testing::isolated!();
        crate::test_support::use_built_meta();
        let root = crate::test_support::nested(&sandbox);
        for origin_of in ["", "web", "platform", "platform/core", "platform/api"] {
            let up = sandbox
                .work()
                .join("up")
                .join(origin_of.replace('/', "-") + ".git");
            sandbox.git(
                &sandbox.work(),
                &[
                    "clone",
                    "-q",
                    "--bare",
                    root.join(origin_of).to_str().unwrap(),
                    up.to_str().unwrap(),
                ],
            );
            sandbox.git(
                &root.join(origin_of),
                &["remote", "add", "origin", up.to_str().unwrap()],
            );
        }
        let make = |line: &[&str]| {
            let out = crate::run::meta()
                .unwrap()
                .cwd(&root)
                .args(line)
                .json()
                .unwrap()
                .wait()
                .unwrap();
            assert!(out.error.is_none(), "{:?}", out.error);
        };
        make(&[
            "-r",
            "git",
            "worktree",
            "create",
            "s",
            "--repo",
            "platform/api",
        ]);
        let set = root.join(".worktrees/s");
        std::fs::write(root.join("web/stray"), "x").unwrap();
        std::fs::write(set.join("platform/api/README"), "changed").unwrap();
        for start in [set.clone(), set.join("platform/api")] {
            let server = McpServer::at(start).unwrap();
            let state = json(call(&server, "meta_workspace_state", json!({})));
            assert_eq!(state["total_projects"], 4, "{state}");
            assert_eq!(state["dirty_projects"], 1, "{state}");
            let dirty = json(call(
                &server,
                "meta_query_repos",
                json!({"query": "dirty:true"}),
            ));
            assert_eq!(
                listed(&dirty["projects"], "name"),
                ["platform/api"],
                "{dirty}"
            );
            let branches = json(call(&server, "meta_git_branch", json!({})));
            let members = [".", "platform", "platform/api", "platform/core"];
            assert_eq!(listed(&branches, "project"), members);
            let status = json(call(&server, "meta_git_status", json!({})));
            assert_eq!(listed(&status["results"], "project"), members, "{status}");
            // svc depends on api, but the set doesn't hold it
            let impact = json(call(
                &server,
                "meta_analyze_impact",
                json!({"project": "platform/api"}),
            ));
            assert_eq!(impact["direct_dependents"], json!([]), "{impact}");
            let impact = json(call(
                &server,
                "meta_analyze_impact",
                json!({"project": "core"}),
            ));
            assert_eq!(
                impact["direct_dependents"],
                json!(["platform/api"]),
                "{impact}"
            );
        }
        // from a nested member, a project of its config is the one in the set
        std::fs::write(set.join("platform/api/new"), "x").unwrap();
        let server = McpServer::at(set.join("platform")).unwrap();
        let added = call(
            &server,
            "meta_git_add",
            json!({"project": "api", "files": ["new"]}),
        )
        .unwrap();
        assert!(!added.is_error, "{}", added.text);
        let staged = sandbox.git(
            &set.join("platform/api"),
            &["diff", "--cached", "--name-only"],
        );
        assert_eq!(staged.trim(), "new");

        make(&[
            "git",
            "worktree",
            "create",
            "bare",
            "--repo",
            "web",
            "--no-deps",
        ]);
        let bare = root.join(".worktrees/bare");
        assert!(!bare.join(".git").exists());
        let server = McpServer::at(bare.clone()).unwrap();
        let branches = json(call(&server, "meta_git_branch", json!({})));
        assert_eq!(listed(&branches, "project"), ["web"]);
        // its members only, an atomic batch too: no project it doesn't hold
        for (atomic, command) in [(true, vec!["true"]), (false, vec!["touch", "ran"])] {
            let batch = json(call(
                &server,
                "meta_batch_execute",
                json!({"command": command, "atomic": atomic}),
            ));
            assert_eq!(batch["has_failure"], false, "{batch}");
            assert_eq!(listed(&batch["results"], "project"), ["web"], "{batch}");
        }
        let projects = call(&server, "meta_list_projects", json!({})).unwrap().text;
        assert!(projects.starts_with("Found 1 project"), "{projects}");
        std::fs::write(bare.join("web/Makefile"), "build:\n\ttouch built\n").unwrap();
        std::fs::write(root.join("web/Makefile"), "build:\n\ttouch built\n").unwrap();
        call(&server, "meta_build", json!({})).unwrap();
        assert!(bare.join("web/ran").exists());
        assert!(bare.join("web/built").exists());
        assert!(!root.join("web/ran").exists());
        assert!(!root.join("web/built").exists());
    }

    /// In a set with no root checkout, a project outside the workspace is
    /// at its name, and is a member there.
    #[test]
    fn a_rootless_set_holds_an_outside_project_at_its_name() {
        let sandbox = meta_plugin::testing::isolated!();
        crate::test_support::use_built_meta();
        let root = crate::test_support::nested(&sandbox);
        sandbox.repo(&sandbox.work().join("sib"));
        std::fs::write(
            root.join(".meta.yaml"),
            r#"{"projects":{"web":"x","platform":{"repo":"x","meta":true},"sib":{"repo":"x","path":"../sib"}}}"#,
        )
        .unwrap();
        let made = crate::run::meta()
            .unwrap()
            .cwd(&root)
            .args([
                "git",
                "worktree",
                "create",
                "o",
                "--repo",
                "sib",
                "--no-deps",
            ])
            .json()
            .unwrap()
            .wait()
            .unwrap();
        assert!(made.error.is_none(), "{:?}", made.error);
        let set = root.join(".worktrees/o");
        assert!(set.join("sib/.git").exists() && !set.join(".git").exists());
        let server = McpServer::at(set).unwrap();
        let projects = call(&server, "meta_list_projects", json!({})).unwrap().text;
        assert!(projects.starts_with("Found 1 project"), "{projects}");
        assert!(projects.contains("- sib (sib)"), "{projects}");
    }

    #[test]
    fn multi_commit_refuses_a_project_outside_the_workspace_before_committing() {
        let sandbox = meta_plugin::testing::isolated!();
        let (server, tmp) = server(&sandbox, "projects:\n  a: {path: dir/a}\n");
        let a = tmp.join("dir/a");
        sandbox.repo(&a);
        let outside = sandbox.work().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        sandbox.repo(&outside);
        for dir in [&a, &outside] {
            std::fs::write(dir.join("README"), "staged").unwrap();
            sandbox.git(dir, &["add", "README"]);
        }
        let ws_relative = format!("../{}", outside.file_name().unwrap().to_string_lossy());
        let absolute = outside.to_string_lossy().into_owned();
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
        assert_eq!(sandbox.git(&outside, &["log", "--format=%s"]), "init\n");
        assert_eq!(sandbox.git(&a, &["log", "--format=%s"]), "init\n");
    }

    /// A server keeps to its own workspace: when that one's config is gone,
    /// it never takes the workspace around it for its own.
    #[test]
    fn a_workspace_whose_config_is_gone_is_not_its_parents() {
        let sandbox = meta_plugin::testing::isolated!();
        let (_, tmp) = server(&sandbox, "projects:\n  child: {meta: true}\n");
        let child = tmp.join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join(".meta.yaml"), "projects: {}\n").unwrap();
        let server = McpServer::at(child.clone()).unwrap();
        assert!(call(&server, "meta_list_projects", json!({})).is_ok());
        std::fs::remove_file(child.join(".meta.yaml")).unwrap();
        let err = call(&server, "meta_list_projects", json!({}))
            .err()
            .unwrap();
        assert!(err.to_string().contains("No meta config found"), "{err}");
    }

    #[test]
    fn checkout_says_why_git_refused_the_branch() {
        let sandbox = meta_plugin::testing::isolated!();
        let (server, tmp) = server(&sandbox, "projects: {}\n");
        sandbox.repo(&tmp);
        let refused = |branch: &str| {
            let reply = call(&server, "meta_git_checkout", json!({"branch": branch}));
            format!("{:#}", reply.err().unwrap())
        };
        assert!(refused("--orphan=evil").contains("is not a valid branch name"));
        // git repeats the name; a control character in it comes back escaped
        assert!(!refused("a\nb").contains('\n'));
        // a valid name, but a config git can't read: git's words, not a guess
        std::fs::write(tmp.join(".git/config"), "[broken").unwrap();
        let err = refused("feature/demo");
        assert!(
            err.contains("fatal:") && !err.contains("not a valid branch name"),
            "{err}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn build_uses_make_for_root_and_child_and_fails_only_when_all_do() {
        let sandbox = meta_plugin::testing::isolated!();
        let (server, tmp) = server(&sandbox, r#"{"projects":{"child":{"tags":["selected"]}}}"#);
        let child = tmp.join("child");
        std::fs::create_dir(&child).unwrap();
        for path in [&tmp, child.as_path()] {
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
        assert!(tmp.join("built").exists());
        assert!(child.join("built").exists());

        // one failed: its result says so, the others' still count
        let failing = "all:\n\t@echo build-failed >&2\n\t@exit 1\n";
        std::fs::write(child.join("Makefile"), failing).unwrap();
        let reply = call(&server, "meta_build", json!({})).unwrap();
        assert!(!reply.is_error, "{}", reply.text);
        assert!(reply.text.contains("build-failed"), "{}", reply.text);
        // all failed: the call failed
        std::fs::write(tmp.join("Makefile"), failing).unwrap();
        let reply = call(&server, "meta_build", json!({})).unwrap();
        assert!(reply.is_error, "{}", reply.text);
    }

    #[test]
    fn tags_select_and_refuse_as_meta_tag_does() {
        let sandbox = meta_plugin::testing::isolated!();
        crate::test_support::use_built_meta();
        let config = r#"{"projects":{"a":{"repo":"x","tags":["red"],"meta":true},
            "b":{"repo":"x","tags":["blue","red"]},"c":{"repo":"x","tags":["green"]},"d":"x"}}"#;
        let (server, root) = server(&sandbox, config);
        // a nested project's tag: `meta --tag` (no -r) doesn't know it
        std::fs::create_dir(root.join("a")).unwrap();
        let nested = r#"{"projects":{"n":{"repo":"x","tags":["ui"]}}}"#;
        std::fs::write(root.join("a/.meta.yaml"), nested).unwrap();
        let workspace = server.workspace().unwrap();
        // what `meta --tag` (with -r: `recursive`) selects among the same
        // projects (the root, which MCP's tag scopes never take, aside), or
        // its error
        let host = |tag: Option<&str>, recursive: bool| -> Result<Vec<String>, String> {
            let filters = match tag {
                Some(tag) => Filters::tags(tag).map_err(|e| format!("{e:#}"))?,
                None => vec![],
            };
            let filters = Filters {
                tags: filters,
                ..Filters::default()
            };
            let tree = match recursive {
                true => config::walk_projects(&workspace.dir, &workspace.projects, None).unwrap(),
                false => vec![],
            };
            let scope = meta_core::repo_filter::Scope::workspace(
                &workspace.dir,
                Some(&workspace.config_path),
                &workspace.projects,
                &tree,
            );
            Ok(scope
                .select_repos(&filters)
                .map_err(|e| format!("{e:#}"))?
                .iter()
                .filter_map(|repo| repo.name().map(str::to_string))
                .collect())
        };
        let all = ["a", "b", "c", "d"];
        for (tag, want) in [
            (None, &all[..]),
            (Some("red"), &["a", "b"][..]),
            (Some(" red "), &["a", "b"]),
            (Some("green,blue"), &["b", "c"]),
            (Some(" blue , green ,"), &["b", "c"]),
            (Some("red,green"), &["a", "b", "c"]),
        ] {
            let mcp: Vec<String> = workspace
                .select(&workspace.scope(None, tag.map(str::to_string)).unwrap())
                .unwrap()
                .into_iter()
                .map(|target| target.name)
                .collect();
            assert_eq!(mcp, want, "{tag:?}");
            assert_eq!(host(tag, false).unwrap(), want, "{tag:?}");
        }
        // blank and unknown tags: every tool that takes `tag` refuses them
        // with meta's error, before anything runs
        let tagged: Vec<_> = contract()
            .into_iter()
            .filter(|(_, arguments, _)| arguments.contains(&"tag"))
            .collect();
        assert_eq!(tagged.len(), 19);
        for tag in [
            "",
            ",",
            " ",
            "nothing",
            "red,nothing",
            "RED",
            "red green",
            "ui",
        ] {
            for (tool, _, smallest) in &tagged {
                // these read nested projects, as `meta -r` does
                let recursive =
                    ["meta_search_code", "meta_git_status", "meta_git_branch"].contains(tool);
                let mut arguments = smallest.clone();
                arguments["tag"] = json!(tag);
                let result = call(&server, tool, arguments)
                    .map(|reply| reply.text)
                    .map_err(|e| format!("{e:#}"));
                match host(Some(tag), recursive) {
                    Ok(_) => assert!(result.is_ok(), "{tool} {tag:?}: {result:?}"),
                    // MCP takes no -r, so its error leaves out that hint
                    Err(refused) => {
                        let refused = refused.trim_end_matches(
                            " --recursive reads the tags of nested meta repos too.",
                        );
                        assert!(
                            result.as_ref().err().is_some_and(|e| e.contains(refused)),
                            "{tool} {tag:?}: {result:?}, not {refused:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn detect_build_systems_reports_every_system_meta_knows() {
        let sandbox = meta_plugin::testing::isolated!();
        let (server, root) = server(&sandbox, r#"{"projects":{"all":"x"}}"#);
        let dir = root.join("all");
        std::fs::create_dir(&dir).unwrap();
        for marker in [
            "meson.build",
            "CMakeLists.txt",
            "setup.py",
            "pyproject.toml",
            "build.gradle.kts",
            "build.gradle",
            "pom.xml",
            "makefile",
            "Makefile",
            "go.mod",
            "package.json",
            "Cargo.toml",
        ] {
            std::fs::write(dir.join(marker), "").unwrap();
        }
        let results = json(call(&server, "meta_detect_build_systems", json!({})));
        assert_eq!(
            results[0]["build_systems"],
            json!(["cargo", "npm", "go", "make", "maven", "gradle", "python", "cmake", "meson"])
        );
    }

    #[test]
    #[cfg(unix)]
    fn output_past_the_capture_bound_is_drained_and_said_cut() {
        let sandbox = meta_plugin::testing::isolated!();
        let (server, root) = server(&sandbox, r#"{"projects":{}}"#);
        // 32 MiB on each stream, twice the bound; then it still ends, as
        // nothing blocks it
        let make =
            "all:\n\t@yes x | head -c 33554432\n\t@yes y | head -c 33554432 >&2\n\t@touch ended\n";
        std::fs::write(root.join("Makefile"), make).unwrap();
        let results = json(call(&server, "meta_build", json!({})));
        let limit = meta_plugin::process::CAPTURE_LIMIT;
        for stream in ["stdout", "stderr"] {
            let text = results[0][stream].as_str().unwrap();
            let cut = format!(
                "\n[meta-mcp: {stream} cut at {limit} bytes; {} more were dropped]\n",
                (32 << 20) - limit
            );
            assert!(text.ends_with(&cut), "{stream}: {:?}", text.lines().last());
            assert_eq!(text.len(), limit + cut.len(), "{stream}");
        }
        assert!(root.join("ended").exists());
    }

    #[test]
    fn file_tree_lists_links_without_following_them() {
        let sandbox = meta_plugin::testing::isolated!();
        let (server, tmp) = server(
            &sandbox,
            "projects:\n  a: {tags: [x]}\n  b: {tags: [y]}\n  c: {tags: [z]}\n",
        );
        for project in ["a", "b", "c"] {
            std::fs::create_dir_all(tmp.join(project).join("src")).unwrap();
        }
        // a link back up the tree, which a walk that follows it never leaves
        // (Windows makes one only with Developer Mode)
        let link = tmp.join("a/src/up");
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

        // a blank list is refused, as `meta --tag` refuses it
        let err = call(&server, "meta_get_file_tree", json!({"tag": " , "}));
        assert!(err
            .err()
            .is_some_and(|e| e.to_string().contains("names no tag")));

        // null, as clients send for an option they don't use, is the default
        json(call(&server, "meta_get_file_tree", json!({"depth": null})));
    }

    #[test]
    fn execution_order_selects_any_of_the_tags() {
        let sandbox = meta_plugin::testing::isolated!();
        let (server, _tmp) = server(
            &sandbox,
            "projects:\n  a: {tags: [x]}\n  b: {tags: [y]}\n  c: {tags: [z]}\n",
        );
        let order = json(call(
            &server,
            "meta_execution_order",
            json!({"tag": "x, y"}),
        ));
        assert_eq!(order["count"], 2, "{order}");
    }

    #[test]
    fn build_and_test_share_detection_and_keep_release() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.work();
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
            std::fs::write(tmp.join(file), "").unwrap();
            assert_eq!(
                repo_command(&tmp, RepoAction::Build { release: true }),
                Some((program, build))
            );
            assert_eq!(repo_command(&tmp, RepoAction::Test), Some((program, test)));
        }
    }

    #[test]
    fn get_config_ignores_meta_directory_and_shows_a_broken_config() {
        let sandbox = meta_plugin::testing::isolated!();
        let (server, tmp) = server(&sandbox, "projects: {}\n");
        std::fs::create_dir_all(tmp.join(".meta/skills")).unwrap();
        let result = call(&server, "meta_get_config", json!({})).unwrap().text;
        assert!(result.contains(".meta.yaml"));
        assert!(result.contains("projects: {}"));
        std::fs::write(tmp.join(".meta.yaml"), "projects: {}\nunknown_top: 1\n").unwrap();
        let reply = call(&server, "meta_get_config", json!({})).unwrap();
        assert!(
            !reply.is_error && reply.text.contains("unknown_top: 1"),
            "{}",
            reply.text
        );
    }
}
