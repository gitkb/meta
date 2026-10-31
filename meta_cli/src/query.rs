//! Query DSL for filtering and analyzing meta projects
//!
//! Provides a simple query language for agents to ask intelligent questions about workspace state.
//!
//! # Query Syntax
//!
//! Queries use a field:value syntax with optional operators:
//! - `dirty:true` - Projects with uncommitted changes
//! - `branch:main` - Projects on a specific branch
//! - `tag:backend` - Projects with a specific tag
//! - `modified_in:24h` - Projects modified within a time period
//!
//! Queries can be combined with AND:
//! - `dirty:true AND tag:backend`
//! - `branch:main AND modified_in:7d`

use anyhow::{Context, Result};
use meta_core::display::escape;
use meta_core::git::RepoStatus;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Represents a query filter condition
#[derive(Debug, Clone, PartialEq)]
pub enum QueryCondition {
    /// Filter by dirty/clean status
    Dirty(bool),
    /// Filter by branch name
    Branch(String),
    /// Filter by tag
    Tag(String),
    /// Filter by modification time (within duration)
    ModifiedIn(Duration),
    /// Filter by language/build system
    Language(String),
    /// Filter by having unpushed commits
    HasUnpushed(bool),
    /// Filter by being ahead of remote
    AheadOfRemote(bool),
    /// Filter by being behind remote
    BehindRemote(bool),
}

/// A parsed query with multiple conditions
#[derive(Debug, Clone)]
pub struct Query {
    pub conditions: Vec<QueryCondition>,
}

impl Query {
    /// Parse a query string into a Query
    ///
    /// Example: "dirty:true AND tag:backend AND branch:main"
    pub fn parse(query_str: &str) -> Result<Self> {
        let mut conditions = Vec::new();

        // Replace case-insensitive " and " with " AND " for uniformity
        let normalized = query_str.replace(" and ", " AND ");

        // Split by " AND "
        let parts: Vec<&str> = normalized
            .split(" AND ")
            .filter(|s| !s.is_empty())
            .collect();

        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            let condition = Self::parse_condition(part)?;
            conditions.push(condition);
        }

        if conditions.is_empty() {
            anyhow::bail!("Empty query");
        }

        Ok(Query { conditions })
    }

    fn parse_condition(s: &str) -> Result<QueryCondition> {
        let parts: Vec<&str> = s.splitn(2, ':').collect();
        if parts.len() != 2 {
            anyhow::bail!(
                "Invalid condition format: '{}'. Expected 'field:value'",
                escape(s)
            );
        }

        let field = parts[0].trim().to_lowercase();
        let value = parts[1].trim();

        match field.as_str() {
            "dirty" => {
                let is_dirty = value.parse::<bool>()
                    .with_context(|| format!("Invalid boolean value for dirty: '{}'", escape(value)))?;
                Ok(QueryCondition::Dirty(is_dirty))
            }
            "branch" => Ok(QueryCondition::Branch(value.to_string())),
            "tag" => Ok(QueryCondition::Tag(value.to_string())),
            "modified_in" | "modified" => {
                let secs = meta_core::duration::parse_duration(value)?;
                Ok(QueryCondition::ModifiedIn(Duration::from_secs(secs)))
            }
            "language" | "lang" => Ok(QueryCondition::Language(value.to_lowercase())),
            "has_unpushed" | "unpushed" => {
                let has_unpushed = value.parse::<bool>()
                    .with_context(|| format!("Invalid boolean value: '{}'", escape(value)))?;
                Ok(QueryCondition::HasUnpushed(has_unpushed))
            }
            "ahead" | "ahead_of_remote" => {
                let ahead = value.parse::<bool>()
                    .with_context(|| format!("Invalid boolean value: '{}'", escape(value)))?;
                Ok(QueryCondition::AheadOfRemote(ahead))
            }
            "behind" | "behind_remote" => {
                let behind = value.parse::<bool>()
                    .with_context(|| format!("Invalid boolean value: '{}'", escape(value)))?;
                Ok(QueryCondition::BehindRemote(behind))
            }
            _ => anyhow::bail!("Unknown query field: '{}'. Valid fields: dirty, branch, tag, modified_in, language, has_unpushed, ahead, behind", escape(&field)),
        }
    }
}

/// Repository state information collected for querying
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoState {
    pub name: String,
    pub path: String,
    pub branch: String,
    pub tags: Vec<String>,
    pub is_dirty: bool,
    pub has_staged: bool,
    pub has_unstaged: bool,
    pub has_untracked: bool,
    pub ahead: usize,
    pub behind: usize,
    pub last_commit_time: Option<i64>,
    pub last_commit_hash: Option<String>,
    pub last_commit_message: Option<String>,
    pub build_systems: Vec<String>,
}

impl RepoState {
    /// Collect state for a repository at the given path
    pub fn collect(name: &str, path: &Path, tags: &[String]) -> Result<Self> {
        let path_str = path.to_string_lossy().to_string();

        // A repo git can't read shows as on branch "unknown", clean
        let status = RepoStatus::read(path).ok();
        let branch = match &status {
            Some(status) => status.branch.clone().unwrap_or_else(|| "HEAD".to_string()),
            None => "unknown".to_string(),
        };
        let status = status.unwrap_or_default();
        let (ahead, behind) = status.ahead_behind.unwrap_or_default();
        let last_commit = status
            .oid
            .as_ref()
            .and_then(|_| meta_core::git::query(path, ["log", "-1", "--format=%ct%n%s"]));
        let (last_commit_time, last_commit_message) = match last_commit.as_deref() {
            Some(log) => {
                let (time, message) = log.split_once('\n').unwrap_or((log, ""));
                (time.parse().ok(), Some(message.to_string()))
            }
            None => (None, None),
        };

        // Detect build systems
        let build_systems = detect_build_systems(path);

        Ok(RepoState {
            name: name.to_string(),
            path: path_str,
            branch,
            tags: tags.to_vec(),
            is_dirty: status.is_dirty(),
            has_staged: status.has_staged(),
            has_unstaged: status.has_unstaged(),
            has_untracked: status.has_untracked(),
            ahead,
            behind,
            last_commit_time,
            last_commit_hash: status.oid,
            last_commit_message,
            build_systems,
        })
    }

    /// Check if this repo state matches a query
    pub fn matches(&self, query: &Query) -> bool {
        for condition in &query.conditions {
            if !self.matches_condition(condition) {
                return false;
            }
        }
        true
    }

    fn matches_condition(&self, condition: &QueryCondition) -> bool {
        match condition {
            QueryCondition::Dirty(expected) => self.is_dirty == *expected,
            QueryCondition::Branch(expected) => self.branch == *expected,
            QueryCondition::Tag(expected) => self.tags.iter().any(|t| t == expected),
            QueryCondition::ModifiedIn(duration) => {
                if let Some(commit_time) = self.last_commit_time {
                    let commit_time =
                        SystemTime::UNIX_EPOCH + Duration::from_secs(commit_time as u64);
                    let now = SystemTime::now();
                    if let Ok(elapsed) = now.duration_since(commit_time) {
                        elapsed <= *duration
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            QueryCondition::Language(lang) => self
                .build_systems
                .iter()
                .any(|bs| bs.to_lowercase() == *lang),
            QueryCondition::HasUnpushed(expected) => (self.ahead > 0) == *expected,
            QueryCondition::AheadOfRemote(expected) => (self.ahead > 0) == *expected,
            QueryCondition::BehindRemote(expected) => (self.behind > 0) == *expected,
        }
    }
}

/// Detect build systems in a project directory
fn detect_build_systems(path: &Path) -> Vec<String> {
    let mut systems = Vec::new();

    let checks = [
        ("Cargo.toml", "cargo"),
        ("package.json", "npm"),
        ("go.mod", "go"),
        ("Makefile", "make"),
        ("makefile", "make"),
        ("pom.xml", "maven"),
        ("build.gradle", "gradle"),
        ("build.gradle.kts", "gradle"),
        ("pyproject.toml", "python"),
        ("setup.py", "python"),
        ("CMakeLists.txt", "cmake"),
        ("meson.build", "meson"),
    ];

    for (file, system) in checks {
        if path.join(file).exists() && !systems.contains(&system.to_string()) {
            systems.push(system.to_string());
        }
    }

    systems
}

/// Workspace state summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceState {
    pub total_projects: usize,
    pub dirty_projects: usize,
    pub clean_projects: usize,
    pub ahead_of_remote: usize,
    pub behind_remote: usize,
    pub projects_by_branch: HashMap<String, usize>,
    pub projects_by_tag: HashMap<String, usize>,
    pub projects_by_build_system: HashMap<String, usize>,
}

impl WorkspaceState {
    /// Compute workspace state from a list of repo states
    pub fn from_repos(repos: &[RepoState]) -> Self {
        let mut projects_by_branch: HashMap<String, usize> = HashMap::new();
        let mut projects_by_tag: HashMap<String, usize> = HashMap::new();
        let mut projects_by_build_system: HashMap<String, usize> = HashMap::new();

        let mut dirty_projects = 0;
        let mut ahead_of_remote = 0;
        let mut behind_remote = 0;

        for repo in repos {
            if repo.is_dirty {
                dirty_projects += 1;
            }
            if repo.ahead > 0 {
                ahead_of_remote += 1;
            }
            if repo.behind > 0 {
                behind_remote += 1;
            }

            *projects_by_branch.entry(repo.branch.clone()).or_insert(0) += 1;

            for tag in &repo.tags {
                *projects_by_tag.entry(tag.clone()).or_insert(0) += 1;
            }

            for bs in &repo.build_systems {
                *projects_by_build_system.entry(bs.clone()).or_insert(0) += 1;
            }
        }

        WorkspaceState {
            total_projects: repos.len(),
            dirty_projects,
            clean_projects: repos.len() - dirty_projects,
            ahead_of_remote,
            behind_remote,
            projects_by_branch,
            projects_by_tag,
            projects_by_build_system,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_query() {
        let query = Query::parse("dirty:true").unwrap();
        assert_eq!(query.conditions.len(), 1);
        assert_eq!(query.conditions[0], QueryCondition::Dirty(true));
    }

    #[test]
    fn test_parse_compound_query() {
        let query = Query::parse("dirty:true AND tag:backend").unwrap();
        assert_eq!(query.conditions.len(), 2);
        assert_eq!(query.conditions[0], QueryCondition::Dirty(true));
        assert_eq!(
            query.conditions[1],
            QueryCondition::Tag("backend".to_string())
        );
    }

    #[test]
    fn test_parse_branch_query() {
        let query = Query::parse("branch:main").unwrap();
        assert_eq!(query.conditions.len(), 1);
        assert_eq!(
            query.conditions[0],
            QueryCondition::Branch("main".to_string())
        );
    }

    #[test]
    fn test_parse_modified_in_query() {
        let query = Query::parse("modified_in:24h").unwrap();
        assert_eq!(
            query.conditions,
            [QueryCondition::ModifiedIn(Duration::from_secs(86400))]
        );
    }

    #[test]
    fn untracked_files_are_neither_staged_nor_unstaged() {
        let dir = tempfile::tempdir().unwrap();
        meta_core::git::run(dir.path(), ["init", "-q", "-b", "main"]).unwrap();
        std::fs::write(dir.path().join("new"), "").unwrap();
        let state = RepoState::collect("r", dir.path(), &[]).unwrap();
        assert!(state.is_dirty && state.has_untracked);
        assert!(!state.has_staged && !state.has_unstaged);
        // before the first commit, the branch it will be on
        assert_eq!(state.branch, "main");
        assert_eq!(state.last_commit_hash, None);
    }

    #[test]
    fn test_repo_state_matches() {
        let repo = RepoState {
            name: "test".to_string(),
            path: "/test".to_string(),
            branch: "main".to_string(),
            tags: vec!["backend".to_string(), "rust".to_string()],
            is_dirty: true,
            has_staged: false,
            has_unstaged: true,
            has_untracked: false,
            ahead: 2,
            behind: 0,
            last_commit_time: Some(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
            ),
            last_commit_hash: Some("abc123".to_string()),
            last_commit_message: Some("test commit".to_string()),
            build_systems: vec!["cargo".to_string()],
        };

        // Test dirty match
        let query = Query::parse("dirty:true").unwrap();
        assert!(repo.matches(&query));

        let query = Query::parse("dirty:false").unwrap();
        assert!(!repo.matches(&query));

        // Test branch match
        let query = Query::parse("branch:main").unwrap();
        assert!(repo.matches(&query));

        let query = Query::parse("branch:develop").unwrap();
        assert!(!repo.matches(&query));

        // Test tag match
        let query = Query::parse("tag:backend").unwrap();
        assert!(repo.matches(&query));

        let query = Query::parse("tag:frontend").unwrap();
        assert!(!repo.matches(&query));

        // Test compound query
        let query = Query::parse("dirty:true AND tag:backend AND branch:main").unwrap();
        assert!(repo.matches(&query));

        let query = Query::parse("dirty:true AND tag:frontend").unwrap();
        assert!(!repo.matches(&query));
    }
}
