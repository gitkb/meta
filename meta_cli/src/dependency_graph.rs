//! Dependency tracking and impact analysis for meta projects
//!
//! Supports extended .meta.yaml schema with provides/depends_on fields:
//!
//! ```yaml
//! projects:
//!   api-service:
//!     repo: git@github.com:org/api.git
//!     tags: [backend]
//!     provides: [api-v2]
//!     depends_on:
//!       - auth-service
//!       - shared-utils
//! ```

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

use meta_core::config::ProjectInfo;
use meta_core::git::GitUrl;

/// Represents a project with dependency information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDependencies {
    pub name: String,
    pub path: String,
    /// Git remote URL. Should be present for all normal projects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<GitUrl>,
    pub tags: Vec<String>,
    /// What this project provides (e.g., APIs, libraries)
    pub provides: Vec<String>,
    /// What this project depends on (other project names or provided items)
    pub depends_on: Vec<String>,
}

impl From<ProjectInfo> for ProjectDependencies {
    fn from(p: ProjectInfo) -> Self {
        Self {
            name: p.name,
            path: p.path,
            repo: p.repo,
            tags: p.tags,
            provides: p.provides,
            depends_on: p.depends_on,
        }
    }
}

/// Dependency graph for analyzing relationships between projects
#[derive(Debug, Clone)]
pub struct DependencyGraph {
    /// Map from project name to project info
    projects: HashMap<String, ProjectDependencies>,
    /// Map from provided item to project name that provides it
    providers: HashMap<String, String>,
    /// Adjacency list: project -> projects it depends on
    dependencies: HashMap<String, Vec<String>>,
    /// Reverse adjacency list: project -> projects that depend on it
    dependents: HashMap<String, Vec<String>>,
    /// Each `depends_on` that names neither a project nor what one provides:
    /// (project, the name)
    unresolved: Vec<(String, String)>,
    /// Each item two projects provide: (item, the project kept, the other)
    duplicates: Vec<(String, String, String)>,
}

/// What a `depends_on` of `project` that names nothing says, escaped.
pub fn unresolved_message(project: &str, dep: &str) -> String {
    use meta_core::display::escape;
    format!(
        "'{}' depends on '{}', which no project is or provides",
        escape(project),
        escape(dep)
    )
}

impl DependencyGraph {
    /// Build a dependency graph from a list of projects. What it can't
    /// resolve is kept for the caller to report: [`Self::unresolved`],
    /// [`Self::warn_unresolved`].
    pub fn build(projects: Vec<ProjectDependencies>) -> Self {
        let mut graph = DependencyGraph {
            projects: HashMap::new(),
            providers: HashMap::new(),
            dependencies: HashMap::new(),
            dependents: HashMap::new(),
            unresolved: Vec::new(),
            duplicates: Vec::new(),
        };

        // First pass: register all projects and their provides
        for project in &projects {
            graph.projects.insert(project.name.clone(), project.clone());

            // Register provided items
            for provided in &project.provides {
                if let Some(existing) = graph
                    .providers
                    .insert(provided.clone(), project.name.clone())
                {
                    graph
                        .duplicates
                        .push((provided.clone(), project.name.clone(), existing));
                }
            }
        }

        // Second pass: resolve dependencies
        for project in &projects {
            for dep in &project.depends_on {
                // A project's name, or what a project provides
                let resolved = if graph.projects.contains_key(dep) {
                    dep.clone()
                } else if let Some(provider) = graph.providers.get(dep) {
                    provider.clone()
                } else {
                    graph.unresolved.push((project.name.clone(), dep.clone()));
                    continue;
                };
                graph
                    .dependents
                    .entry(resolved.clone())
                    .or_default()
                    .push(project.name.clone());
                graph
                    .dependencies
                    .entry(project.name.clone())
                    .or_default()
                    .push(resolved);
            }
        }

        graph
    }

    /// Each `depends_on` the graph couldn't resolve: (project, the name).
    pub fn unresolved(&self) -> &[(String, String)] {
        &self.unresolved
    }

    /// Warn about what [`Self::build`] couldn't resolve.
    pub fn warn_unresolved(&self) {
        for (project, dep) in &self.unresolved {
            meta_core::out::warn(unresolved_message(project, dep));
        }
        // nothing to fail: without strict, warn_or_bail only warns
        let _ = self.warn_duplicates(false);
    }

    /// Warn about each item two projects provide, or with `strict` fail.
    pub fn warn_duplicates(&self, strict: bool) -> Result<()> {
        use meta_core::display::escape;
        for (item, kept, other) in &self.duplicates {
            let kept = escape(kept);
            meta_core::out::warn_or_bail(
                strict,
                format!(
                    "Both '{}' and '{kept}' provide '{}'; '{kept}' is used",
                    escape(other),
                    escape(item)
                ),
            )?;
        }
        Ok(())
    }

    /// Get direct dependencies of a project
    pub fn get_dependencies(&self, project: &str) -> Vec<&str> {
        self.dependencies
            .get(project)
            .map(|deps| deps.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// Get all transitive dependencies of a project
    pub fn get_all_dependencies(&self, project: &str) -> Vec<&str> {
        let mut visited = HashSet::new();
        let mut result = Vec::new();
        let mut queue = VecDeque::new();

        if let Some(deps) = self.dependencies.get(project) {
            for dep in deps {
                queue.push_back(dep.as_str());
            }
        }

        while let Some(current) = queue.pop_front() {
            if visited.contains(current) {
                continue;
            }
            visited.insert(current);
            result.push(current);

            if let Some(deps) = self.dependencies.get(current) {
                for dep in deps {
                    if !visited.contains(dep.as_str()) {
                        queue.push_back(dep.as_str());
                    }
                }
            }
        }

        result
    }

    /// The projects that depend directly on `project` (a project's name, or
    /// else a capability one provides: the first such project by name),
    /// through its name or anything it provides, sorted; `None` when no
    /// project has that name or provides it.
    pub fn dependents_of(&self, project: &str) -> Option<Vec<&str>> {
        let target = self.projects.get(project).or_else(|| {
            self.projects
                .values()
                .filter(|p| p.provides.iter().any(|t| t == project))
                .min_by_key(|p| &p.name)
        })?;
        let names: Vec<&str> = std::iter::once(target.name.as_str())
            .chain(target.provides.iter().map(String::as_str))
            .collect();
        let mut dependents: Vec<&str> = self
            .projects
            .values()
            .filter(|p| p.name != target.name)
            .filter(|p| p.depends_on.iter().any(|d| names.contains(&d.as_str())))
            .map(|p| p.name.as_str())
            .collect();
        dependents.sort_unstable();
        Some(dependents)
    }

    /// Get all transitive dependents (impact analysis)
    /// Returns all projects that would be affected if the given project changes
    pub fn analyze_impact(&self, project: &str) -> ImpactAnalysis {
        let mut visited = HashSet::new();
        let mut direct_dependents = Vec::new();
        let mut transitive_dependents = Vec::new();
        let mut queue = VecDeque::new();

        // Get direct dependents
        if let Some(deps) = self.dependents.get(project) {
            for dep in deps {
                direct_dependents.push(dep.clone());
                queue.push_back((dep.as_str(), 1));
            }
        }

        // BFS to find all transitive dependents
        while let Some((current, depth)) = queue.pop_front() {
            if visited.contains(current) {
                continue;
            }
            visited.insert(current);

            if depth > 1 {
                transitive_dependents.push(current.to_string());
            }

            if let Some(deps) = self.dependents.get(current) {
                for dep in deps {
                    if !visited.contains(dep.as_str()) {
                        queue.push_back((dep.as_str(), depth + 1));
                    }
                }
            }
        }

        ImpactAnalysis {
            project: project.to_string(),
            direct_dependents,
            transitive_dependents,
            total_affected: visited.len(),
        }
    }

    /// Get topological sort order for building/testing
    /// Returns projects in order such that dependencies come before dependents
    pub fn execution_order(&self) -> Result<Vec<&str>> {
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut result = Vec::new();
        let mut queue = VecDeque::new();

        // Initialize in-degrees to 0
        for name in self.projects.keys() {
            in_degree.insert(name.as_str(), 0);
        }

        // Calculate in-degrees: for each project, count how many dependencies it has
        // A project can only be processed after all its dependencies are processed
        for (project, deps) in &self.dependencies {
            // The in-degree is the number of dependencies this project has
            in_degree.insert(project.as_str(), deps.len());
        }

        // Start with nodes that have no dependencies (in_degree = 0)
        for (name, &degree) in &in_degree {
            if degree == 0 {
                queue.push_back(*name);
            }
        }

        // Process queue using Kahn's algorithm
        while let Some(current) = queue.pop_front() {
            result.push(current);

            // For each project that depends on 'current', decrease its in-degree
            if let Some(dependents) = self.dependents.get(current) {
                for dependent in dependents {
                    if let Some(degree) = in_degree.get_mut(dependent.as_str()) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(dependent.as_str());
                        }
                    }
                }
            }
        }

        // Check for cycles
        if result.len() != self.projects.len() {
            anyhow::bail!(
                "Dependency cycle detected! Processed {} of {} projects",
                result.len(),
                self.projects.len()
            );
        }

        Ok(result)
    }

    /// Get project info
    pub fn get_project(&self, name: &str) -> Option<&ProjectDependencies> {
        self.projects.get(name)
    }

    /// Get all projects
    pub fn all_projects(&self) -> Vec<&ProjectDependencies> {
        self.projects.values().collect()
    }
}

/// Result of impact analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactAnalysis {
    pub project: String,
    pub direct_dependents: Vec<String>,
    pub transitive_dependents: Vec<String>,
    pub total_affected: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_projects() -> Vec<ProjectDependencies> {
        vec![
            ProjectDependencies {
                name: "shared-utils".to_string(),
                path: "shared-utils".to_string(),
                repo: Some("git@github.com:org/shared-utils.git".into()),
                tags: vec!["lib".to_string()],
                provides: vec!["utils".to_string()],
                depends_on: vec![],
            },
            ProjectDependencies {
                name: "auth-service".to_string(),
                path: "auth-service".to_string(),
                repo: Some("git@github.com:org/auth-service.git".into()),
                tags: vec!["backend".to_string()],
                provides: vec!["auth-api".to_string()],
                depends_on: vec!["shared-utils".to_string()],
            },
            ProjectDependencies {
                name: "api-service".to_string(),
                path: "api-service".to_string(),
                repo: Some("git@github.com:org/api-service.git".into()),
                tags: vec!["backend".to_string()],
                provides: vec!["api-v2".to_string()],
                depends_on: vec!["auth-service".to_string(), "shared-utils".to_string()],
            },
            ProjectDependencies {
                name: "web-app".to_string(),
                path: "web-app".to_string(),
                repo: Some("git@github.com:org/web-app.git".into()),
                tags: vec!["frontend".to_string()],
                provides: vec![],
                depends_on: vec!["api-v2".to_string()], // Depends on provided item
            },
        ]
    }

    #[test]
    fn test_build_graph() {
        let projects = create_test_projects();
        let graph = DependencyGraph::build(projects);

        assert_eq!(graph.projects.len(), 4);
        assert!(graph.providers.contains_key("utils"));
        assert!(graph.providers.contains_key("auth-api"));
    }

    #[test]
    fn test_get_dependencies() {
        let projects = create_test_projects();
        let graph = DependencyGraph::build(projects);

        let deps = graph.get_dependencies("api-service");
        assert!(deps.contains(&"auth-service"));
        assert!(deps.contains(&"shared-utils"));
    }

    #[test]
    fn test_impact_analysis() {
        let projects = create_test_projects();
        let graph = DependencyGraph::build(projects);

        let impact = graph.analyze_impact("shared-utils");
        assert_eq!(impact.project, "shared-utils");
        assert!(impact
            .direct_dependents
            .contains(&"auth-service".to_string()));
        assert!(impact
            .direct_dependents
            .contains(&"api-service".to_string()));
        // web-app depends on api-service which depends on shared-utils
        assert!(impact
            .transitive_dependents
            .contains(&"web-app".to_string()));
    }

    #[test]
    fn test_execution_order() {
        let projects = create_test_projects();
        let graph = DependencyGraph::build(projects);

        let order = graph.execution_order().unwrap();

        // shared-utils should come before auth-service
        let shared_pos = order.iter().position(|&n| n == "shared-utils").unwrap();
        let auth_pos = order.iter().position(|&n| n == "auth-service").unwrap();
        assert!(shared_pos < auth_pos);

        // auth-service should come before api-service
        let api_pos = order.iter().position(|&n| n == "api-service").unwrap();
        assert!(auth_pos < api_pos);

        // api-service should come before web-app
        let web_pos = order.iter().position(|&n| n == "web-app").unwrap();
        assert!(api_pos < web_pos);
    }

    #[test]
    fn test_provided_item_resolution() {
        let projects = create_test_projects();
        let graph = DependencyGraph::build(projects);

        // web-app depends on "api-v2" which is provided by api-service
        let deps = graph.get_dependencies("web-app");
        assert!(deps.contains(&"api-service"));
    }

    #[test]
    fn dependents_of_a_name_or_a_capability() {
        let project = |name: &str, provides: &[&str], depends_on: &[&str]| ProjectDependencies {
            name: name.to_string(),
            path: name.to_string(),
            repo: None,
            tags: vec![],
            provides: provides.iter().map(|s| s.to_string()).collect(),
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
        };
        let graph = DependencyGraph::build(vec![
            project("loop_lib", &["loop-lib"], &[]),
            project("util", &[], &[]),
            project("provider", &["util"], &["provider"]),
            project("meta_cli", &[], &["loop-lib", "loop_lib", "util"]),
            project("plugin", &[], &["loop_lib", "Loop-Lib"]),
            project("db_a", &["db"], &[]),
            project("db_b", &["db"], &[]),
            project("app", &[], &["db"]),
        ]);
        // by name or by what it provides, each dependent once, sorted
        for query in ["loop_lib", "loop-lib"] {
            assert_eq!(graph.dependents_of(query), Some(vec!["meta_cli", "plugin"]));
        }
        // a name wins over a capability; what a project provides counts
        // for it; a project never depends on itself
        assert_eq!(graph.dependents_of("util"), Some(vec!["meta_cli"]));
        assert_eq!(graph.dependents_of("provider"), Some(vec!["meta_cli"]));
        assert_eq!(graph.dependents_of("nonexistent"), None);
        // what several provide counts for each of them
        for provider in ["db_a", "db_b", "db"] {
            assert_eq!(graph.dependents_of(provider), Some(vec!["app"]));
        }
    }
}
