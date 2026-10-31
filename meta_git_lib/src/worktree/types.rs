//! Types for worktree management.

use serde::Serialize;
use std::collections::HashMap;

// ==================== Domain Types ====================

/// A repo specifier: `alias` or `alias:branch`
#[derive(Debug, Clone)]
pub struct RepoSpec {
    pub alias: String,
    pub branch: Option<String>,
}

impl std::fmt::Display for RepoSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.branch {
            Some(branch) => write!(f, "{}:{}", self.alias, branch),
            None => write!(f, "{}", self.alias),
        }
    }
}

impl std::str::FromStr for RepoSpec {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.split_once(':') {
            Some((alias, branch)) => Ok(RepoSpec {
                alias: alias.to_string(),
                branch: Some(branch.to_string()),
            }),
            None => Ok(RepoSpec {
                alias: s.to_string(),
                branch: None,
            }),
        }
    }
}

// ==================== JSON Output Structures ====================

#[derive(Debug, Serialize)]
pub struct CreateOutput {
    pub name: String,
    pub root: String,
    pub repos: Vec<CreateRepoEntry>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ephemeral: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub custom: HashMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct CreateRepoEntry {
    pub alias: String,
    pub path: String,
    pub branch: String,
    pub created_branch: bool,
}

#[derive(Debug, Serialize)]
pub struct ListOutput {
    pub worktrees: Vec<ListEntry>,
}

#[derive(Debug, Serialize)]
pub struct AddOutput {
    pub name: String,
    pub repos: Vec<CreateRepoEntry>,
}

#[derive(Debug, Serialize)]
pub struct DestroyOutput {
    pub name: String,
    pub path: String,
    pub repos_removed: usize,
    /// A preview: nothing was removed
    pub dry_run: bool,
}

#[derive(Debug, Serialize)]
pub struct ListEntry {
    pub name: String,
    pub root: String,
    pub has_meta_root: bool,
    pub repos: Vec<ListRepoEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_remaining_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize)]
pub struct ListRepoEntry {
    pub alias: String,
    pub branch: String,
    pub dirty: bool,
}

#[derive(Debug, Serialize)]
pub struct StatusOutput {
    pub name: String,
    pub repos: Vec<StatusRepoEntry>,
}

#[derive(Debug, Serialize)]
pub struct StatusRepoEntry {
    pub alias: String,
    pub path: String,
    pub branch: String,
    pub dirty: bool,
    pub modified_count: usize,
    pub untracked_count: usize,
    pub ahead: usize,
    pub behind: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modified_files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DiffOutput {
    pub name: String,
    pub base: String,
    pub repos: Vec<DiffRepoEntry>,
    pub totals: DiffTotals,
}

#[derive(Debug, Serialize)]
pub struct DiffRepoEntry {
    pub alias: String,
    pub base_ref: String,
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct DiffTotals {
    pub repos_changed: usize,
    pub files_changed: usize,
    pub insertions: usize,
    pub deletions: usize,
}

#[derive(Debug, Serialize)]
pub struct PruneOutput {
    pub removed: Vec<PruneEntry>,
    /// Candidates kept because deleting them would lose work.
    pub kept: Vec<PruneEntry>,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PruneEntry {
    pub name: String,
    pub path: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_seconds: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── RepoSpec FromStr ────────────────────────────────────

    #[test]
    fn repo_spec_parse_alias_only() {
        let spec: RepoSpec = "meta_cli".parse().unwrap();
        assert_eq!(spec.alias, "meta_cli");
        assert!(spec.branch.is_none());
    }

    #[test]
    fn repo_spec_parse_alias_with_branch() {
        let spec: RepoSpec = "meta_cli:feature-x".parse().unwrap();
        assert_eq!(spec.alias, "meta_cli");
        assert_eq!(spec.branch.as_deref(), Some("feature-x"));
    }

    #[test]
    fn repo_spec_parse_branch_with_colon() {
        // "foo:bar:baz" → alias="foo", branch="bar:baz" (split_once)
        let spec: RepoSpec = "foo:bar:baz".parse().unwrap();
        assert_eq!(spec.alias, "foo");
        assert_eq!(spec.branch.as_deref(), Some("bar:baz"));
    }

    #[test]
    fn repo_spec_parse_empty_string() {
        let spec: RepoSpec = "".parse().unwrap();
        assert_eq!(spec.alias, "");
        assert!(spec.branch.is_none());
    }

    // ── RepoSpec Display ────────────────────────────────────

    #[test]
    fn repo_spec_display_alias_only() {
        let spec = RepoSpec {
            alias: "meta_cli".to_string(),
            branch: None,
        };
        assert_eq!(spec.to_string(), "meta_cli");
    }

    #[test]
    fn repo_spec_display_alias_with_branch() {
        let spec = RepoSpec {
            alias: "meta_cli".to_string(),
            branch: Some("feature-x".to_string()),
        };
        assert_eq!(spec.to_string(), "meta_cli:feature-x");
    }

    // ── RepoSpec round-trip ─────────────────────────────────

    #[test]
    fn repo_spec_round_trip_alias_only() {
        let input = "meta_core";
        let spec: RepoSpec = input.parse().unwrap();
        assert_eq!(spec.to_string(), input);
    }

    #[test]
    fn repo_spec_round_trip_with_branch() {
        let input = "meta_cli:my-branch";
        let spec: RepoSpec = input.parse().unwrap();
        assert_eq!(spec.to_string(), input);
    }
}
