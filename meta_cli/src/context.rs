//! Workspace context summary for `meta context`.
//!
//! Outputs a structured summary of the workspace: repos, branches, dirty status,
//! tags, dependencies. Designed for both humans and LLM agents (injected via
//! Claude Code SessionStart hook).

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::dependency_graph::DependencyGraph;
use meta_core::config::{self, ProjectInfo};
use meta_core::display::{escape, escape_path};
use meta_core::git::RepoStatus;

// ── Cache ───────────────────────────────────────────────

const CACHE_TTL_SECONDS: u64 = 30;

#[derive(Debug, Serialize, Deserialize)]
struct CachedContext {
    context: WorkspaceContext,
    timestamp: SystemTime,
    workspace_root: PathBuf,
}

fn cache_path() -> anyhow::Result<PathBuf> {
    meta_core::data_dir::data_file("context_cache")
}

fn load_cache() -> Option<CachedContext> {
    // checked like any store: it is rendered into what agents read
    meta_core::store::read::<Option<CachedContext>>(&cache_path().ok()?)
        .ok()
        .flatten()
}

fn save_cache(cached: &CachedContext, verbose: bool) {
    let Ok(path) = cache_path() else {
        return;
    };
    match meta_core::store::write_atomic(&path, cached) {
        Ok(()) if verbose => eprintln!("Saved context cache to {}", escape_path(&path)),
        Ok(()) => {}
        Err(e) if verbose => eprintln!("Failed to write context cache: {e:#}"),
        Err(_) => {}
    }
}

fn is_cache_valid(cached: &CachedContext, current_root: &PathBuf) -> bool {
    // Check workspace root matches
    if cached.workspace_root != *current_root {
        return false;
    }

    // Check TTL
    let elapsed = match SystemTime::now().duration_since(cached.timestamp) {
        Ok(e) => e,
        Err(_) => return false,
    };

    if elapsed >= Duration::from_secs(CACHE_TTL_SECONDS) {
        return false;
    }

    // Check if git state changed in any repo
    // If .git/HEAD or branch refs were modified after cache timestamp, invalidate
    for repo in &cached.context.repos {
        let repo_path = current_root.join(&repo.path);
        let git_dir = repo_path.join(".git");

        // Check .git/HEAD modification time
        if let Ok(head_meta) = std::fs::metadata(git_dir.join("HEAD")) {
            if let Ok(head_mtime) = head_meta.modified() {
                if head_mtime > cached.timestamp {
                    return false; // HEAD changed, invalidate
                }
            }
        }

        // Check branch ref file if we know the branch
        if let Some(ref branch) = repo.branch {
            let ref_path = git_dir.join("refs").join("heads").join(branch);
            if let Ok(ref_meta) = std::fs::metadata(&ref_path) {
                if let Ok(ref_mtime) = ref_meta.modified() {
                    if ref_mtime > cached.timestamp {
                        return false; // Branch ref changed, invalidate
                    }
                }
            }
        }
    }

    true
}

// ── Public API ──────────────────────────────────────────

/// Entry point for `meta context`.
pub fn handle_context(json: bool, no_status: bool, no_cache: bool, verbose: bool) -> Result<()> {
    let cwd = std::env::current_dir().context("Failed to get current directory")?;

    let workspace = config::Workspace::find(&cwd, None)?
        .ok_or_else(|| anyhow::anyhow!("Not a meta workspace (no .meta.yaml found)"))?;
    let meta_dir = workspace.dir.clone();

    // Try cache if not bypassed
    if !no_cache && !no_status {
        if let Some(cached) = load_cache() {
            if is_cache_valid(&cached, &meta_dir) {
                if verbose {
                    eprintln!("Using cached context (age < {CACHE_TTL_SECONDS}s)");
                }
                if json {
                    println!("{}", serde_json::to_string_pretty(&cached.context)?);
                } else {
                    print!("{}", format_markdown(&cached.context));
                }
                return Ok(());
            } else if verbose {
                eprintln!("Cache expired or invalid, regenerating...");
            }
        }
    }

    let workspace_name = meta_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let projects = workspace.projects;

    if verbose {
        eprintln!(
            "Found {} projects in {}",
            projects.len(),
            escape_path(&workspace.config_path)
        );
    }

    let repos: Vec<RepoContext> = if no_status {
        projects.iter().map(RepoContext::from_project).collect()
    } else {
        projects
            .par_iter()
            .map(|p| {
                let mut ctx = RepoContext::from_project(p);
                let repo_path = meta_dir.join(&p.path);
                match RepoStatus::read(&repo_path) {
                    Ok(status) => {
                        ctx.branch = status.branch.clone();
                        ctx.dirty = Some(status.is_dirty());
                        ctx.modified_count = Some(status.entries.len());
                        (ctx.ahead, ctx.behind) = status.ahead_behind.unzip();
                    }
                    // a work tree git refuses (`core.bare=true`): its refs,
                    // and no word on its changes
                    Err(_) => {
                        if let Some(refs) = RepoStatus::refs(&repo_path) {
                            ctx.branch = refs.branch;
                            (ctx.ahead, ctx.behind) = refs.ahead_behind.unzip();
                        }
                    }
                }
                ctx
            })
            .collect()
    };

    let dependencies = build_dependency_map(&projects);

    let ctx = WorkspaceContext {
        name: workspace_name,
        description:
            "Multi-repo workspace managed by `meta`. Use `meta` commands for cross-repo operations."
                .to_string(),
        repo_count: repos.len(),
        repos,
        commands: key_commands(),
        dependencies,
    };

    // Save to cache (only if status was collected and cache wasn't bypassed)
    if !no_cache && !no_status {
        let cached = CachedContext {
            context: ctx.clone(),
            timestamp: SystemTime::now(),
            workspace_root: meta_dir,
        };
        save_cache(&cached, verbose);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&ctx)?);
    } else {
        print!("{}", format_markdown(&ctx));
    }

    Ok(())
}

// ── Types ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceContext {
    pub name: String,
    pub description: String,
    pub repo_count: usize,
    pub repos: Vec<RepoContext>,
    pub commands: Vec<CommandRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<HashMap<String, Vec<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRef {
    pub command: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoContext {
    pub name: String,
    pub path: String,
    /// Git remote URL. Should be present for all normal projects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ahead: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind: Option<usize>,
    pub tags: Vec<String>,
}

impl RepoContext {
    /// Create a RepoContext from a ProjectInfo with no git status.
    pub fn from_project(p: &ProjectInfo) -> Self {
        Self {
            name: p.name.clone(),
            path: p.path.clone(),
            repo: p.repo.clone(),
            branch: None,
            dirty: None,
            modified_count: None,
            ahead: None,
            behind: None,
            tags: p.tags.clone(),
        }
    }
}

fn key_commands() -> Vec<CommandRef> {
    vec![
        CommandRef {
            command: "meta git status".to_string(),
            description: "all repos at once".to_string(),
        },
        CommandRef {
            command: "meta exec -- <cmd>".to_string(),
            description: "run in all repos".to_string(),
        },
        CommandRef {
            command: "meta git commit -m \"msg\"".to_string(),
            description: "commit in all dirty repos".to_string(),
        },
        CommandRef {
            command: "meta --include X,Y exec -- <cmd>".to_string(),
            description: "target specific repos".to_string(),
        },
    ]
}

// ── Dependency Graph ────────────────────────────────────

fn build_dependency_map(projects: &[ProjectInfo]) -> Option<HashMap<String, Vec<String>>> {
    let has_deps = projects.iter().any(|p| !p.depends_on.is_empty());
    if !has_deps {
        return None;
    }

    let dep_projects: Vec<_> = projects.iter().map(|p| p.clone().into()).collect();
    let graph = DependencyGraph::build(dep_projects).ok()?;

    let mut map = HashMap::new();
    for project in projects {
        let deps = graph.get_dependencies(&project.name);
        if !deps.is_empty() {
            map.insert(
                project.name.clone(),
                deps.iter().map(|s| s.to_string()).collect(),
            );
        }
    }

    if map.is_empty() {
        None
    } else {
        Some(map)
    }
}

// ── Markdown Formatting ─────────────────────────────────

/// Text from `.meta.yaml`, git or the filesystem, made safe for this markdown,
/// which agents read: control characters escaped, then `\` and `|` escaped
/// so the value stays in its table cell.
fn md(s: &str) -> String {
    escape(s).replace('\\', "\\\\").replace('|', "\\|")
}

fn md_list(items: &[String]) -> String {
    items.iter().map(|s| md(s)).collect::<Vec<_>>().join(", ")
}

pub fn format_markdown(ctx: &WorkspaceContext) -> String {
    let mut out = String::new();

    // Header
    out.push_str(&format!(
        "# Meta Workspace: {} ({} repos)\n\n",
        md(&ctx.name),
        ctx.repo_count
    ));
    out.push_str(&md(&ctx.description));
    out.push_str("\n\n");

    // Repo table
    let has_status = ctx.repos.iter().any(|r| r.branch.is_some());
    let has_tags = ctx.repos.iter().any(|r| !r.tags.is_empty());

    if has_status && has_tags {
        out.push_str("## Repos\n");
        out.push_str("| Repo | Branch | Status | Tags |\n");
        out.push_str("|------|--------|--------|------|\n");
        for r in &ctx.repos {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                md(&r.name),
                md(r.branch.as_deref().unwrap_or("-")),
                format_status(r),
                md_list(&r.tags),
            ));
        }
    } else if has_status {
        out.push_str("## Repos\n");
        out.push_str("| Repo | Branch | Status |\n");
        out.push_str("|------|--------|--------|\n");
        for r in &ctx.repos {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                md(&r.name),
                md(r.branch.as_deref().unwrap_or("-")),
                format_status(r),
            ));
        }
    } else {
        out.push_str("## Repos\n");
        if has_tags {
            out.push_str("| Repo | Tags |\n");
            out.push_str("|------|------|\n");
            for r in &ctx.repos {
                out.push_str(&format!("| {} | {} |\n", md(&r.name), md_list(&r.tags)));
            }
        } else {
            for r in &ctx.repos {
                out.push_str(&format!("- {}\n", md(&r.name)));
            }
        }
    }

    // Key commands
    out.push_str("\n## Key Commands\n");
    for cmd in &ctx.commands {
        out.push_str(&format!(
            "- `{}` — {}\n",
            md(&cmd.command),
            md(&cmd.description)
        ));
    }

    // Dependencies
    if let Some(ref deps) = ctx.dependencies {
        out.push_str("\n## Dependencies\n");
        let mut sorted_keys: Vec<&String> = deps.keys().collect();
        sorted_keys.sort();
        for key in sorted_keys {
            let targets = &deps[key];
            out.push_str(&format!("{} → {}\n", md(key), md_list(targets)));
        }
    }

    out
}

fn format_status(r: &RepoContext) -> String {
    let base = match (r.dirty, r.modified_count) {
        (Some(false), _) => "clean".to_string(),
        (Some(true), Some(n)) => format!("{n} modified"),
        (Some(true), None) => "dirty".to_string(),
        _ => "-".to_string(),
    };

    // Add ahead/behind indicator
    match (r.ahead, r.behind) {
        (Some(a), Some(b)) if a > 0 && b > 0 => format!("{base} (↑{a} ↓{b})"),
        (Some(a), _) if a > 0 => format!("{base} (↑{a})"),
        (_, Some(b)) if b > 0 => format!("{base} (↓{b})"),
        _ => base,
    }
}

// ── Tests ───────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx(
        repos: Vec<RepoContext>,
        deps: Option<HashMap<String, Vec<String>>>,
    ) -> WorkspaceContext {
        WorkspaceContext {
            name: "test-workspace".to_string(),
            description: "Multi-repo workspace managed by `meta`. Use `meta` commands for cross-repo operations.".to_string(),
            repo_count: repos.len(),
            repos,
            commands: key_commands(),
            dependencies: deps,
        }
    }

    fn make_repo(
        name: &str,
        branch: Option<&str>,
        dirty: Option<bool>,
        modified: Option<usize>,
        tags: Vec<&str>,
    ) -> RepoContext {
        RepoContext {
            name: name.to_string(),
            path: name.to_string(),
            repo: Some(format!("git@github.com:org/{name}.git")),
            branch: branch.map(|s| s.to_string()),
            dirty,
            modified_count: modified,
            ahead: None,
            behind: None,
            tags: tags.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn markdown_escapes_untrusted_values() {
        // Values from git, the filesystem or an old context cache can carry
        // terminal escapes or break the table that agents read.
        let mut ctx = make_ctx(
            vec![make_repo(
                "z\u{1b}[2J",
                Some("evil|branch\\|x\u{1b}]52;c;cm0K\u{7}"),
                Some(false),
                Some(0),
                vec!["n\nfake"],
            )],
            Some(HashMap::from([(
                "a\u{1b}[1A".to_string(),
                vec!["b\nc".to_string()],
            )])),
        );
        ctx.name = "ws\u{1b}]0;TITLE\u{7}".to_string();
        let md = format_markdown(&ctx);
        assert!(!md.contains('\u{1b}') && !md.contains('\u{7}'), "{md}");
        assert!(
            md.contains(
                "| z\\\\u{1b}[2J | evil\\|branch\\\\\\|x\\\\u{1b}]52;c;cm0K\\\\u{7} | clean | n\\\\nfake |"
            ),
            "{md}"
        );
        assert!(
            md.contains("# Meta Workspace: ws\\\\u{1b}]0;TITLE\\\\u{7} (1 repos)"),
            "{md}"
        );
        assert!(md.contains("a\\\\u{1b}[1A → b\\\\nc"), "{md}");
    }

    // ── format_status ──────────────────────────────────

    #[test]
    fn status_clean() {
        let r = make_repo("x", Some("main"), Some(false), Some(0), vec![]);
        assert_eq!(format_status(&r), "clean");
    }

    #[test]
    fn status_dirty_with_count() {
        let r = make_repo("x", Some("main"), Some(true), Some(3), vec![]);
        assert_eq!(format_status(&r), "3 modified");
    }

    #[test]
    fn status_dirty_no_count() {
        let r = make_repo("x", Some("main"), Some(true), None, vec![]);
        assert_eq!(format_status(&r), "dirty");
    }

    #[test]
    fn status_unknown() {
        let r = make_repo("x", None, None, None, vec![]);
        assert_eq!(format_status(&r), "-");
    }

    #[test]
    fn status_ahead_only() {
        let mut r = make_repo("x", Some("main"), Some(false), Some(0), vec![]);
        r.ahead = Some(3);
        assert_eq!(format_status(&r), "clean (↑3)");
    }

    #[test]
    fn status_behind_only() {
        let mut r = make_repo("x", Some("main"), Some(false), Some(0), vec![]);
        r.behind = Some(2);
        assert_eq!(format_status(&r), "clean (↓2)");
    }

    #[test]
    fn status_ahead_and_behind() {
        let mut r = make_repo("x", Some("main"), Some(false), Some(0), vec![]);
        r.ahead = Some(5);
        r.behind = Some(3);
        assert_eq!(format_status(&r), "clean (↑5 ↓3)");
    }

    #[test]
    fn status_dirty_with_ahead() {
        let mut r = make_repo("x", Some("main"), Some(true), Some(4), vec![]);
        r.ahead = Some(2);
        assert_eq!(format_status(&r), "4 modified (↑2)");
    }

    #[test]
    fn status_dirty_with_behind() {
        let mut r = make_repo("x", Some("main"), Some(true), Some(1), vec![]);
        r.behind = Some(7);
        assert_eq!(format_status(&r), "1 modified (↓7)");
    }

    #[test]
    fn status_dirty_with_ahead_and_behind() {
        let mut r = make_repo("x", Some("main"), Some(true), Some(2), vec![]);
        r.ahead = Some(1);
        r.behind = Some(1);
        assert_eq!(format_status(&r), "2 modified (↑1 ↓1)");
    }

    #[test]
    fn status_zero_ahead_behind_not_shown() {
        let mut r = make_repo("x", Some("main"), Some(false), Some(0), vec![]);
        r.ahead = Some(0);
        r.behind = Some(0);
        assert_eq!(format_status(&r), "clean");
    }

    // ── format_markdown ─────────────────────────────────

    #[test]
    fn markdown_includes_header() {
        let ctx = make_ctx(
            vec![make_repo("lib", Some("main"), Some(false), Some(0), vec![])],
            None,
        );
        let md = format_markdown(&ctx);
        assert!(md.contains("# Meta Workspace: test-workspace (1 repos)"));
        assert!(md.contains("Multi-repo workspace"));
    }

    #[test]
    fn markdown_includes_repo_table_with_status() {
        let ctx = make_ctx(
            vec![
                make_repo("api", Some("main"), Some(false), Some(0), vec![]),
                make_repo("web", Some("feat-x"), Some(true), Some(2), vec![]),
            ],
            None,
        );
        let md = format_markdown(&ctx);
        assert!(md.contains("| Repo | Branch | Status |"));
        assert!(md.contains("| api | main | clean |"));
        assert!(md.contains("| web | feat-x | 2 modified |"));
    }

    #[test]
    fn markdown_includes_tags_column_when_present() {
        let ctx = make_ctx(
            vec![make_repo(
                "api",
                Some("main"),
                Some(false),
                Some(0),
                vec!["backend"],
            )],
            None,
        );
        let md = format_markdown(&ctx);
        assert!(md.contains("| Tags |"));
        assert!(md.contains("| backend |"));
    }

    #[test]
    fn markdown_no_status_shows_simple_list() {
        let ctx = make_ctx(
            vec![
                make_repo("api", None, None, None, vec![]),
                make_repo("web", None, None, None, vec![]),
            ],
            None,
        );
        let md = format_markdown(&ctx);
        assert!(md.contains("- api"));
        assert!(md.contains("- web"));
    }

    #[test]
    fn markdown_includes_key_commands() {
        let ctx = make_ctx(vec![], None);
        let md = format_markdown(&ctx);
        assert!(md.contains("## Key Commands"));
        assert!(md.contains("meta git status"));
        assert!(md.contains("meta exec"));
    }

    #[test]
    fn markdown_includes_dependencies_when_present() {
        let mut deps = HashMap::new();
        deps.insert("api".to_string(), vec!["core".to_string()]);
        let ctx = make_ctx(vec![], Some(deps));
        let md = format_markdown(&ctx);
        assert!(md.contains("## Dependencies"));
        assert!(md.contains("api → core"));
    }

    #[test]
    fn markdown_omits_dependencies_when_none() {
        let ctx = make_ctx(vec![], None);
        let md = format_markdown(&ctx);
        assert!(!md.contains("## Dependencies"));
    }

    // ── JSON serialization ──────────────────────────────

    #[test]
    fn json_serializes_full_context() {
        let ctx = make_ctx(
            vec![make_repo(
                "api",
                Some("main"),
                Some(false),
                Some(0),
                vec!["backend"],
            )],
            None,
        );
        let json = serde_json::to_string(&ctx).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["name"], "test-workspace");
        assert_eq!(v["repo_count"], 1);
        assert_eq!(v["repos"][0]["name"], "api");
        assert_eq!(v["repos"][0]["path"], "api");
        assert_eq!(v["repos"][0]["repo"], "git@github.com:org/api.git");
        assert_eq!(v["repos"][0]["branch"], "main");
        assert_eq!(v["repos"][0]["dirty"], false);
        assert_eq!(v["repos"][0]["tags"][0], "backend");
    }

    #[test]
    fn json_omits_none_fields() {
        let ctx = make_ctx(vec![make_repo("api", None, None, None, vec![])], None);
        let json = serde_json::to_string(&ctx).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v["repos"][0].get("branch").is_none());
        assert!(v["repos"][0].get("dirty").is_none());
        assert!(v["repos"][0].get("modified_count").is_none());
        assert!(v.get("dependencies").is_none());
    }

    #[test]
    fn json_includes_description_and_commands() {
        let ctx = make_ctx(vec![], None);
        let json = serde_json::to_string(&ctx).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v["description"].as_str().unwrap().contains("meta"));
        let commands = v["commands"].as_array().unwrap();
        assert!(!commands.is_empty());
        assert!(commands[0].get("command").is_some());
        assert!(commands[0].get("description").is_some());
    }

    #[test]
    fn json_includes_dependencies_when_present() {
        let mut deps = HashMap::new();
        deps.insert("api".to_string(), vec!["core".to_string()]);
        let ctx = make_ctx(vec![], Some(deps));
        let json = serde_json::to_string(&ctx).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["dependencies"]["api"][0], "core");
    }

    // ── is_cache_valid ──────────────────────────────────

    #[test]
    fn cache_valid_within_ttl_no_git_changes() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace_root = temp_dir.path().to_path_buf();

        // Create a cached context with timestamp 10 seconds ago
        let timestamp = SystemTime::now() - Duration::from_secs(10);
        let cached = CachedContext {
            context: make_ctx(vec![], None),
            timestamp,
            workspace_root: workspace_root.clone(),
        };

        // Should be valid (within 30s TTL, no repos to check)
        assert!(is_cache_valid(&cached, &workspace_root));
    }

    #[test]
    fn cache_invalid_when_ttl_expired() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace_root = temp_dir.path().to_path_buf();

        // Create a cached context with timestamp 31 seconds ago (beyond TTL)
        let timestamp = SystemTime::now() - Duration::from_secs(31);
        let cached = CachedContext {
            context: make_ctx(vec![], None),
            timestamp,
            workspace_root: workspace_root.clone(),
        };

        // Should be invalid (TTL expired)
        assert!(!is_cache_valid(&cached, &workspace_root));
    }

    #[test]
    fn cache_invalid_when_workspace_root_differs() {
        let temp_dir1 = tempfile::tempdir().unwrap();
        let temp_dir2 = tempfile::tempdir().unwrap();

        let timestamp = SystemTime::now() - Duration::from_secs(10);
        let cached = CachedContext {
            context: make_ctx(vec![], None),
            timestamp,
            workspace_root: temp_dir1.path().to_path_buf(),
        };

        // Different workspace root should invalidate
        assert!(!is_cache_valid(&cached, &temp_dir2.path().to_path_buf()));
    }

    #[test]
    fn cache_invalid_when_git_head_changed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace_root = temp_dir.path().to_path_buf();

        // Create a repo directory with .git/HEAD
        let repo_path = workspace_root.join("test_repo");
        std::fs::create_dir_all(&repo_path).unwrap();
        let git_dir = repo_path.join(".git");
        std::fs::create_dir_all(&git_dir).unwrap();

        // Create .git/HEAD with old timestamp
        let head_file = git_dir.join("HEAD");
        std::fs::write(&head_file, "ref: refs/heads/main\n").unwrap();

        // Cache timestamp before HEAD file
        let cache_time = head_file.metadata().unwrap().modified().unwrap() - Duration::from_secs(5);

        let mut repo = make_repo("test_repo", Some("main"), Some(false), Some(0), vec![]);
        repo.path = "test_repo".to_string();

        let cached = CachedContext {
            context: make_ctx(vec![repo], None),
            timestamp: cache_time,
            workspace_root: workspace_root.clone(),
        };

        // Should be invalid (HEAD modified after cache timestamp)
        assert!(!is_cache_valid(&cached, &workspace_root));
    }

    #[test]
    fn cache_invalid_when_branch_ref_changed() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace_root = temp_dir.path().to_path_buf();

        // Create a repo directory with .git/refs/heads/main
        let repo_path = workspace_root.join("test_repo");
        std::fs::create_dir_all(&repo_path).unwrap();
        let git_dir = repo_path.join(".git");
        let refs_dir = git_dir.join("refs").join("heads");
        std::fs::create_dir_all(&refs_dir).unwrap();

        // Create branch ref with current timestamp
        let main_ref = refs_dir.join("main");
        std::fs::write(&main_ref, "abc123def456\n").unwrap();

        // Cache timestamp before ref file
        let cache_time = main_ref.metadata().unwrap().modified().unwrap() - Duration::from_secs(5);

        let mut repo = make_repo("test_repo", Some("main"), Some(false), Some(0), vec![]);
        repo.path = "test_repo".to_string();

        let cached = CachedContext {
            context: make_ctx(vec![repo], None),
            timestamp: cache_time,
            workspace_root: workspace_root.clone(),
        };

        // Should be invalid (branch ref modified after cache timestamp)
        assert!(!is_cache_valid(&cached, &workspace_root));
    }

    #[test]
    fn cache_valid_when_git_files_unchanged() {
        use std::thread;

        let temp_dir = tempfile::tempdir().unwrap();
        let workspace_root = temp_dir.path().to_path_buf();

        // Create a repo directory with .git/HEAD and branch ref
        let repo_path = workspace_root.join("test_repo");
        std::fs::create_dir_all(&repo_path).unwrap();
        let git_dir = repo_path.join(".git");
        let refs_dir = git_dir.join("refs").join("heads");
        std::fs::create_dir_all(&refs_dir).unwrap();

        // Create files
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(refs_dir.join("main"), "abc123def456\n").unwrap();

        // Small delay to ensure filesystem timestamp resolution
        thread::sleep(Duration::from_millis(10));

        // Cache timestamp AFTER files were created
        let cache_time = SystemTime::now();

        let mut repo = make_repo("test_repo", Some("main"), Some(false), Some(0), vec![]);
        repo.path = "test_repo".to_string();

        let cached = CachedContext {
            context: make_ctx(vec![repo], None),
            timestamp: cache_time,
            workspace_root: workspace_root.clone(),
        };

        // Should be valid (files haven't changed since cache)
        assert!(is_cache_valid(&cached, &workspace_root));
    }

    #[test]
    fn cache_valid_when_git_dir_missing() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace_root = temp_dir.path().to_path_buf();

        // Create repo path but no .git directory
        let repo_path = workspace_root.join("test_repo");
        std::fs::create_dir_all(&repo_path).unwrap();

        let timestamp = SystemTime::now() - Duration::from_secs(10);
        let mut repo = make_repo("test_repo", Some("main"), Some(false), Some(0), vec![]);
        repo.path = "test_repo".to_string();

        let cached = CachedContext {
            context: make_ctx(vec![repo], None),
            timestamp,
            workspace_root: workspace_root.clone(),
        };

        // Should be valid (missing .git is not an invalidation reason)
        assert!(is_cache_valid(&cached, &workspace_root));
    }
}
