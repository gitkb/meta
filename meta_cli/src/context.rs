//! Workspace context summary for `meta context`.
//!
//! Outputs a structured summary of the workspace: repos, branches, dirty status,
//! tags, dependencies. Designed for both humans and LLM agents (injected via
//! Claude Code SessionStart hook).

use anyhow::Result;
use meta_core::{errln, out, outln};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

use crate::dependency_graph::DependencyGraph;
use crate::repo_filter::{Filters, Scope};
use meta_core::config::{self, ProjectInfo};
use meta_core::display::{escape, escape_path};
use meta_core::git::{GitUrl, Operation, RepoStatus};

// ── Public API ──────────────────────────────────────────

/// Entry point for `meta context`: the workspace of the nearest config at or
/// above `start`, with the projects `filters` select.
pub fn handle_context(
    json: bool,
    no_status: bool,
    verbose: bool,
    start: &Path,
    filters: &Filters,
) -> Result<()> {
    let workspace = config::Workspace::find(start, None)?
        .ok_or_else(|| anyhow::anyhow!("Not a meta workspace (no .meta.yaml found)"))?;
    let meta_dir = workspace.dir.clone();

    let workspace_name = meta_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let selected = Scope::workspace(
        &meta_dir,
        Some(&workspace.config_path),
        &workspace.projects,
        &[],
    )
    .select(filters)?;
    let projects: Vec<ProjectInfo> = workspace
        .projects
        .into_iter()
        .filter(|p| selected.contains(&meta_dir.join(&p.path)))
        .collect();

    if verbose {
        errln!(
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
                let read = |dir| anyhow::Ok((RepoStatus::read(dir)?, Operation::read(dir)?));
                match read(&repo_path) {
                    Ok((status, operation)) => {
                        ctx.dirty = Some(status.is_dirty());
                        ctx.modified_count = Some(status.entries.len());
                        ctx.untracked_count = Some(status.untracked_count());
                        ctx.detached = Some(status.branch.is_none());
                        ctx.upstream_gone =
                            status.upstream.is_some().then(|| status.upstream_gone());
                        if let Some(op) = operation {
                            ctx.operation = Some(op.name());
                            if let Operation::Rebase { branch } = op {
                                ctx.rebasing = branch;
                            }
                        }
                        (ctx.ahead, ctx.behind) = status.ahead_behind.unzip();
                        ctx.head = status.oid;
                        ctx.upstream = status.upstream;
                        ctx.branch = status.branch;
                    }
                    // a work tree git refuses (`core.bare=true`): its refs,
                    // and no word on its changes
                    Err(_) => {
                        if let Ok(Some(refs)) = RepoStatus::refs(&repo_path) {
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

    if json {
        outln!("{}", serde_json::to_string_pretty(&ctx)?)?;
    } else {
        out!("{}", format_markdown(&ctx))?;
    }

    Ok(())
}

// ── Types ───────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct WorkspaceContext {
    pub name: String,
    pub description: String,
    pub repo_count: usize,
    pub repos: Vec<RepoContext>,
    pub commands: Vec<CommandRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<HashMap<String, Vec<String>>>,
}

#[derive(Debug, Serialize)]
pub struct CommandRef {
    pub command: String,
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct RepoContext {
    pub name: String,
    pub path: String,
    /// Git remote URL. Should be present for all normal projects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<GitUrl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Whether HEAD names a commit rather than a branch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detached: Option<bool>,
    /// The commit HEAD names; absent before the first commit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// A git operation stopped half-way: `rebase`, `am`, `merge`,
    /// `cherry-pick`, `revert` or `bisect`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<&'static str>,
    /// The branch a stopped rebase is rebasing (HEAD is detached meanwhile).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebasing: Option<String>,
    /// Anything changed or untracked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
    /// The paths changed or untracked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_count: Option<usize>,
    /// Of those, the untracked ones.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub untracked_count: Option<usize>,
    /// The branch's upstream (`origin/main`); absent without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    /// Whether that upstream branch no longer exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_gone: Option<bool>,
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
            detached: None,
            head: None,
            operation: None,
            rebasing: None,
            dirty: None,
            modified_count: None,
            untracked_count: None,
            upstream: None,
            upstream_gone: None,
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
    let graph = DependencyGraph::build(dep_projects);

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
    let has_status = ctx
        .repos
        .iter()
        .any(|r| r.branch.is_some() || r.dirty.is_some());
    let has_tags = ctx.repos.iter().any(|r| !r.tags.is_empty());

    if has_status && has_tags {
        out.push_str("## Repos\n");
        out.push_str("| Repo | Branch | Status | Tags |\n");
        out.push_str("|------|--------|--------|------|\n");
        for r in &ctx.repos {
            out.push_str(&format!(
                "| {} | {} | {} | {} |\n",
                md(&r.name),
                md(&format_branch(r)),
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
                md(&format_branch(r)),
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

/// The branch, the branch a stopped rebase is rebasing, or the commit a
/// detached HEAD names.
fn format_branch(r: &RepoContext) -> String {
    match (&r.branch, &r.rebasing, r.detached, &r.head) {
        (Some(branch), ..) => branch.clone(),
        (None, Some(branch), ..) => format!("(rebasing {branch})"),
        (None, None, Some(true), Some(head)) => {
            format!("(detached {})", head.get(..7).unwrap_or(head))
        }
        _ => "-".to_string(),
    }
}

/// What a person must know before acting in the repo: an operation stopped
/// half-way, its changes, and how it stands against its upstream.
fn format_status(r: &RepoContext) -> String {
    let mut parts = Vec::new();
    if let Some(op) = &r.operation {
        parts.push(format!("{op} in progress"));
    }
    match (r.dirty, r.modified_count, r.untracked_count) {
        (None, _, _) => parts.push("-".to_string()),
        (Some(false), _, _) => parts.push("clean".to_string()),
        (Some(true), Some(all), untracked) => {
            let untracked = untracked.unwrap_or(0);
            let modified = all.saturating_sub(untracked);
            if modified > 0 {
                parts.push(format!("{modified} modified"));
            }
            if untracked > 0 {
                parts.push(format!("{untracked} untracked"));
            }
        }
        (Some(true), None, _) => parts.push("dirty".to_string()),
    }
    if r.detached == Some(false) {
        if r.upstream.is_none() {
            parts.push("no upstream".to_string());
        } else if r.upstream_gone == Some(true) {
            parts.push("upstream gone".to_string());
        }
    }
    let base = parts.join(", ");

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
            repo: Some(format!("git@github.com:org/{name}.git").into()),
            branch: branch.map(|s| s.to_string()),
            // a status read: a branch with its upstream there
            detached: dirty.map(|_| branch.is_none()),
            head: dirty.map(|_| "0123456789abcdef".to_string()),
            operation: None,
            rebasing: None,
            dirty,
            modified_count: modified,
            untracked_count: modified.map(|_| 0),
            upstream: branch
                .filter(|_| dirty.is_some())
                .map(|b| format!("origin/{b}")),
            upstream_gone: branch.filter(|_| dirty.is_some()).map(|_| false),
            ahead: None,
            behind: None,
            tags: tags.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    fn table_rows(markdown: &str) -> Vec<Vec<String>> {
        markdown
            .lines()
            .filter(|line| line.starts_with('|'))
            .map(|line| {
                let mut cells = Vec::new();
                let mut cell = String::new();
                let mut escaped = false;
                for ch in line.chars().skip(1) {
                    if ch == '|' && !escaped {
                        cells.push(cell.trim().to_string());
                        cell.clear();
                    } else {
                        cell.push(ch);
                    }
                    escaped = ch == '\\' && !escaped;
                }
                assert!(
                    cell.trim().is_empty(),
                    "table row has no closing pipe: {line}"
                );
                cells
            })
            .collect()
    }

    #[test]
    fn markdown_escapes_untrusted_values() {
        // Values from git or the filesystem can carry terminal escapes or
        // break the table that agents read.
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
        let rows = table_rows(&md);
        assert_eq!(rows.len(), 3, "{md}");
        assert!(rows.iter().all(|row| row.len() == 4), "{rows:?}");
        let cells = &rows[2];
        assert_eq!(cells[0], r"z\\u{1b}[2J");
        assert_eq!(cells[1], r"evil\|branch\\\|x\\u{1b}]52;c;cm0K\\u{7}");
        assert_eq!(cells[2], "clean");
        assert_eq!(cells[3], r"n\\nfake");
        assert!(
            md.contains("# Meta Workspace: ws\\\\u{1b}]0;TITLE\\\\u{7} (1 repos)"),
            "{md}"
        );
        assert!(md.contains("a\\\\u{1b}[1A → b\\\\nc"), "{md}");
    }

    #[test]
    fn status_reports_changes_and_upstream_properties() {
        // `fact` stands alone in `text`: no digit touches it ("3 modified"
        // is not "13 modified", "↑2" not "↑21")
        let says = |text: &str, fact: &str| {
            text.match_indices(fact).any(|(at, _)| {
                let before = text.get(..at).and_then(|t| t.chars().next_back());
                let after = text.get(at + fact.len()..).and_then(|t| t.chars().next());
                !before.is_some_and(|c| c.is_ascii_digit())
                    && !after.is_some_and(|c| c.is_ascii_digit())
            })
        };
        // (dirty, total changes, untracked, ahead, behind, upstream gone)
        for (dirty, total, untracked, ahead, behind, gone) in [
            (Some(false), Some(0), Some(0), None, None, false),
            (Some(true), Some(3), Some(0), None, None, false),
            (Some(true), None, None, None, None, false), // dirty without counts
            (None, None, None, None, None, false),       // unknown
            (Some(false), Some(0), Some(0), Some(3), None, false),
            (Some(false), Some(0), Some(0), None, Some(2), false),
            (Some(false), Some(0), Some(0), Some(5), Some(3), false),
            (Some(true), Some(4), Some(0), Some(2), None, false),
            (Some(true), Some(1), Some(0), None, Some(7), false),
            (Some(true), Some(2), Some(0), Some(1), Some(1), false),
            (Some(false), Some(0), Some(0), Some(0), Some(0), false),
            (Some(true), Some(3), Some(1), None, None, false),
            (Some(true), Some(1), Some(1), None, None, false),
            (Some(false), Some(0), Some(0), Some(2), None, true),
            (None, None, None, Some(2), None, false), // unknown with ahead count
        ] {
            let mut repo = make_repo("x", Some("main"), dirty, total, vec![]);
            repo.untracked_count = untracked;
            repo.ahead = ahead;
            repo.behind = behind;
            repo.upstream_gone = Some(gone);
            let status = format_status(&repo);
            assert_eq!(
                status.contains("clean"),
                dirty == Some(false),
                "{repo:?}: {status}"
            );
            match (dirty, total) {
                (None, _) => assert!(
                    status.contains('-') && !status.contains("dirty"),
                    "{repo:?}: {status}"
                ),
                (Some(true), None) => assert!(status.contains("dirty"), "{repo:?}: {status}"),
                (Some(true), Some(all)) => {
                    for (count, label) in [
                        (all - untracked.unwrap_or(0), "modified"),
                        (untracked.unwrap_or(0), "untracked"),
                    ] {
                        if count > 0 {
                            assert!(
                                says(&status, &format!("{count} {label}")),
                                "{repo:?}: {status}"
                            );
                        } else {
                            assert!(!status.contains(label), "{repo:?}: {status}");
                        }
                    }
                }
                _ => {}
            }
            for (count, marker) in [(ahead, '↑'), (behind, '↓')] {
                if let Some(count) = count.filter(|n| *n > 0) {
                    assert!(
                        says(&status, &format!("{marker}{count}")),
                        "{repo:?}: {status}"
                    );
                } else {
                    assert!(!status.contains(marker), "{repo:?}: {status}");
                }
            }
            assert!(
                !status.contains("0 modified") && !status.contains("0 untracked"),
                "{repo:?}: {status}"
            );
            assert_eq!(status.contains("upstream gone"), gone, "{repo:?}: {status}");
        }
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
        let rows = table_rows(&md);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0], ["Repo", "Branch", "Status"]);
        assert_eq!(rows[2][0], "api");
        assert_eq!(rows[2][1], "main");
        assert!(rows[2][2].contains("clean"));
        assert_eq!(rows[3][0], "web");
        assert_eq!(rows[3][1], "feat-x");
        assert!(rows[3][2].contains("2 modified"));
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
        let rows = table_rows(&md);
        let tags = rows[0].iter().position(|cell| cell == "Tags").unwrap();
        assert_eq!(rows[2][tags], "backend");
    }

    #[test]
    fn markdown_shows_status_when_every_repo_is_detached() {
        let ctx = make_ctx(
            vec![make_repo("api", None, Some(false), Some(0), vec![])],
            None,
        );
        let md = format_markdown(&ctx);
        let rows = table_rows(&md);
        assert_eq!(rows[2][0], "api");
        assert!(
            rows[2][1].contains("detached") && rows[2][1].contains("0123456"),
            "{md}"
        );
        assert!(rows[2][2].contains("clean"), "{md}");
    }

    #[test]
    fn markdown_names_stopped_rebases_and_missing_or_gone_upstreams() {
        let mut rebase = make_repo("backend", None, Some(true), Some(2), vec![]);
        rebase.operation = Some("rebase");
        rebase.rebasing = Some("main".to_string());
        rebase.untracked_count = Some(1);
        let mut gone = make_repo("frontend", Some("main"), Some(false), Some(0), vec![]);
        gone.upstream_gone = Some(true);
        let mut local = make_repo("local", Some("topic"), Some(false), Some(0), vec![]);
        local.upstream = None;
        local.upstream_gone = None;
        let md = format_markdown(&make_ctx(vec![rebase, gone, local], None));
        let rows = table_rows(&md);
        assert_eq!(rows.len(), 5, "{md}");
        assert_eq!(rows[2][0], "backend");
        assert!(
            rows[2][1].contains("rebasing") && rows[2][1].contains("main"),
            "{md}"
        );
        for fact in ["rebase in progress", "1 modified", "1 untracked"] {
            assert!(rows[2][2].contains(fact), "{md}");
        }
        assert_eq!(rows[3][0], "frontend");
        assert_eq!(rows[3][1], "main");
        assert!(
            rows[3][2].contains("clean") && rows[3][2].contains("upstream gone"),
            "{md}"
        );
        assert_eq!(rows[4][0], "local");
        assert_eq!(rows[4][1], "topic");
        assert!(
            rows[4][2].contains("clean") && rows[4][2].contains("no upstream"),
            "{md}"
        );
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
}
