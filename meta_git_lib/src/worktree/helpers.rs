//! Utility functions for worktree management.

use anyhow::{Context, Result};
use meta_core::display::{escape, escape_path};
use std::path::{Path, PathBuf};

use meta_core::config::{ProjectInfo, Workspace};

pub fn validate_worktree_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("Worktree name cannot be empty");
    }
    let shown = escape(name);
    if name.starts_with('.') {
        anyhow::bail!("Invalid worktree name '{shown}': cannot start with '.'");
    }
    if name.contains('/') || name.contains('\\') {
        anyhow::bail!("Invalid worktree name '{shown}': cannot contain path separators");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!("Invalid worktree name '{shown}': only ASCII alphanumeric characters, hyphens, and underscores allowed");
    }
    Ok(())
}

/// The configs a worktree command reads, each loaded once: `here`, the
/// nearest at or above where it runs, and `owner`, whose sets and settings
/// (`worktrees_dir`, hooks) these are: inside a set, the checkout the set
/// stands for, which outlives it; else `here`.
#[derive(Debug, Clone)]
pub struct Configs {
    pub here: Workspace,
    pub owner: Workspace,
}

impl Configs {
    /// The configs of the workspace at or above `start`; an error outside one.
    pub fn find(start: &Path) -> Result<Self> {
        Self::find_optional(start)?
            .ok_or_else(|| anyhow::anyhow!("Not inside a meta project (no .meta.yaml found)"))
    }

    /// Like [`Configs::find`], `None` outside a workspace.
    pub fn find_optional(start: &Path) -> Result<Option<Self>> {
        Workspace::find(start, None)?.map(Self::of).transpose()
    }

    /// The configs for `here`. A source checkout without a config is still
    /// the owner, with no settings of its own.
    pub fn of(here: Workspace) -> Result<Self> {
        let Some(source) = meta_core::config::set_source(&here.dir).filter(|s| s.is_dir()) else {
            return Ok(Self {
                owner: here.clone(),
                here,
            });
        };
        let owner = Workspace::in_dir(&source)?.unwrap_or_else(|| Workspace {
            config_path: source.join(meta_core::config::META_CONFIG_NAME),
            dir: source,
            projects: Vec::new(),
            ignore: Vec::new(),
            defaults: Default::default(),
            worktrees_dir: None,
            worktree: Default::default(),
        });
        Ok(Self { here, owner })
    }
}

/// Where worktree sets go: `META_WORKTREES`, else the `worktrees_dir` of
/// `owner`'s config (checked when it was loaded), else its `.worktrees`;
/// outside a workspace, `.worktrees` in `cwd`.
pub fn resolve_worktree_root(owner: Option<&Workspace>, cwd: &Path) -> Result<PathBuf> {
    if let Ok(env_path) = std::env::var("META_WORKTREES") {
        return Ok(PathBuf::from(env_path));
    }
    let Some(owner) = owner else {
        return Ok(cwd.join(".worktrees"));
    };
    if let Some(configured) = &owner.worktrees_dir {
        return Ok(owner.dir.join(configured));
    }
    // The default is held to the same rules: a committed symlink there must
    // not send worktrees (and their removal) outside the workspace or into
    // .git
    let root = owner.dir.join(".worktrees");
    meta_core::config::relative_dir(&owner.dir, ".worktrees").map_err(|why| {
        anyhow::anyhow!(
            "Refusing to keep worktrees in {}: {why}",
            escape_path(&root)
        )
    })?;
    Ok(root)
}

/// The directory of the worktree set `name` under `worktree_root`. A symlink
/// there (say one tracked in a repo that holds `worktrees_dir`) must not
/// lead commands, or a removal, outside the worktree root.
pub fn worktree_set_dir(worktree_root: &Path, name: &str) -> Result<PathBuf> {
    let wt_dir = worktree_root.join(name);
    // meta makes set directories itself; one that is a link (even to another
    // set) is someone else's doing
    if wt_dir
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        anyhow::bail!(
            "Refusing to use worktree '{}': {} is a symlink",
            meta_core::display::escape(name),
            escape_path(&wt_dir)
        );
    }
    if worktree_root.exists() {
        meta_core::config::resolve_inside(worktree_root, name).map_err(|why| {
            anyhow::anyhow!(
                "Refusing to use worktree '{}': {why}",
                meta_core::display::escape(name)
            )
        })?;
    }
    Ok(worktree_root.join(name))
}

/// The directory of the existing worktree set `name`.
pub fn resolve_existing_worktree(name: &str) -> Result<PathBuf> {
    validate_worktree_name(name)?;
    let cwd = std::env::current_dir()?;
    let configs = Configs::find_optional(&cwd)?;
    let worktree_root = resolve_worktree_root(configs.as_ref().map(|c| &c.owner), &cwd)?;
    let wt_dir = worktree_set_dir(&worktree_root, name)?;
    if !wt_dir.exists() {
        anyhow::bail!("Worktree '{}' not found at {}", name, escape_path(&wt_dir));
    }
    Ok(wt_dir)
}

/// Resolve, validate existence, and discover repos in a named worktree.
/// Returns a non-empty repo list, or errors if worktree doesn't exist
/// or contains no repos.
pub fn discover_and_validate_worktree(
    name: &str,
) -> Result<Vec<meta_cli::worktree::WorktreeRepoInfo>> {
    let wt_dir = resolve_existing_worktree(name)?;
    let repos = meta_cli::worktree::discover_worktree_repos(&wt_dir)?;
    if repos.is_empty() {
        anyhow::bail!("No repos found in worktree '{}'", escape(name));
    }
    Ok(repos)
}

/// The projects of `workspace`, with its root repo first as "." when
/// `include_root` is set and the workspace directory is itself a git
/// repository.
///
/// The root repo is an implicit dependency of child repos because it contains:
/// - Workspace configuration (Cargo.toml)
/// - Shared config files (.claude/, etc.)
/// - Build scripts and other shared resources
pub fn projects_with_root(workspace: &Workspace, include_root: bool) -> Vec<ProjectInfo> {
    let root = (include_root && workspace.dir.join(".git").exists()).then(|| ProjectInfo {
        name: ".".to_string(),
        path: ".".to_string(),
        repo: None,
        tags: vec![],
        provides: vec![],
        depends_on: vec![],
        meta: false,
    });
    root.into_iter()
        .chain(workspace.projects.iter().cloned())
        .collect()
}

/// Look up a project by alias, returning an error with valid aliases on miss.
pub fn lookup_project<'a>(projects: &'a [ProjectInfo], alias: &str) -> Result<&'a ProjectInfo> {
    projects.iter().find(|p| p.name == alias).ok_or_else(|| {
        let valid: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        anyhow::anyhow!(
            "Unknown repo alias: '{}'. Valid aliases: {}",
            escape(alias),
            escape(&valid.join(", "))
        )
    })
}

/// Where the repo at `source`, found from `meta_dir` under `alias`, goes in
/// a worktree set: its path from the meta dir, so a set lays its repos out
/// as the workspace does (its copied config stays right, and relative
/// references between repos, such as Cargo workspace members, hold). The
/// alias stands in for a source outside the meta dir.
pub fn place_in_set(meta_dir: &Path, alias: &str, source: &Path) -> String {
    // The configured path, as written (a link inside the workspace stays the
    // link); resolved only when the two are spelled differently
    let resolve = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    source
        .strip_prefix(meta_dir)
        .map(Path::to_path_buf)
        .or_else(|_| {
            resolve(source)
                .strip_prefix(resolve(meta_dir))
                .map(Path::to_path_buf)
        })
        .ok()
        .filter(|place| !place.as_os_str().is_empty())
        .map(|place| {
            place
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
        .unwrap_or_else(|| alias.to_string())
}

/// Where the repo placed at `place` is checked out in the set at `wt_dir`:
/// there, or where a link a checkout already put there leads, as in the
/// workspace. Refused when a link leads outside the set.
pub fn dest_in_set(wt_dir: &Path, place: &str) -> Result<PathBuf, String> {
    let resolved = meta_core::config::resolve_inside(wt_dir, place)?;
    let plain = wt_dir.canonicalize().map(|dir| dir.join(place)).ok();
    Ok(if plain.as_ref() == Some(&resolved) {
        wt_dir.join(place)
    } else {
        resolved
    })
}

/// Look up a project by alias, supporting nested paths like "vendor/tree-sitter-markdown".
///
/// For simple aliases (no `/`), uses flat lookup from the current .meta.yaml.
/// For nested paths, walks the meta tree recursively to find the project.
///
/// Returns the resolved path and project info.
pub fn lookup_nested_project(workspace: &Workspace, alias: &str) -> Result<(PathBuf, ProjectInfo)> {
    let meta_dir = workspace.dir.as_path();
    // If alias contains '/', use recursive lookup
    if alias.contains('/') {
        let tree = meta_core::config::walk_projects(meta_dir, &workspace.projects, None)?;

        // Build a map of full path -> ProjectInfo
        let project_map = meta_core::config::build_project_map(&tree, meta_dir, "");

        project_map.get(alias).cloned().ok_or_else(|| {
            // Use keys from the map we already built (avoids re-walking the tree)
            let mut valid_paths: Vec<_> = project_map.keys().collect();
            valid_paths.sort();
            anyhow::anyhow!(
                "Unknown nested repo: '{}'. Valid nested paths: {}",
                escape(alias),
                valid_paths
                    .into_iter()
                    .map(|path| escape(path))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    } else {
        // Existing flat lookup for simple aliases
        let project = lookup_project(&workspace.projects, alias)?;
        Ok((meta_dir.join(&project.path), project.clone()))
    }
}

pub fn resolve_branch(
    task_name: &str,
    branch_flag: Option<&str>,
    per_repo_branch: Option<&str>,
) -> String {
    per_repo_branch
        .or(branch_flag)
        .map(|s| s.to_string())
        .unwrap_or_else(|| task_name.to_string())
}

/// Parse `--from-pr owner/repo#N` format and resolve the PR's head branch.
/// Returns (owner/repo, pr_number, head_branch_name).
pub fn resolve_from_pr(from_pr: &str) -> Result<(String, u32, String)> {
    use std::process::Command;

    // Parse format: owner/repo#N
    let hash_pos = from_pr.rfind('#').ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid --from-pr format: '{}'. Expected: owner/repo#N",
            escape(from_pr)
        )
    })?;

    let repo_spec = &from_pr[..hash_pos];
    // Validate repo spec format: must be owner/repo
    if !repo_spec.contains('/') || repo_spec.starts_with('/') || repo_spec.ends_with('/') {
        anyhow::bail!(
            "Invalid repo spec '{}' in --from-pr. Expected: owner/repo#N",
            escape(repo_spec)
        );
    }
    let pr_num: u32 = from_pr[hash_pos + 1..]
        .parse()
        .with_context(|| format!("Invalid PR number in '{}'", escape(from_pr)))?;

    // Resolve head branch via gh CLI
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            &pr_num.to_string(),
            "--repo",
            repo_spec,
            "--json",
            "headRefName",
            "-q",
            ".headRefName",
        ])
        .output()
        .with_context(|| "Failed to run 'gh' CLI. Is it installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "Failed to resolve PR #{} in {}: {}",
            pr_num,
            escape(repo_spec),
            escape(stderr.trim())
        );
    }

    let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if branch.is_empty() {
        anyhow::bail!(
            "Empty head branch for PR #{pr_num} in {}",
            escape(repo_spec)
        );
    }

    Ok((repo_spec.to_string(), pr_num, branch))
}

/// Check if a repo's `origin` remote is exactly the given owner/repo.
/// The URL git fetches from is what counts here, `insteadOf` rewrites
/// applied (an origin of `gh:repo` may be GitHub's `org/repo`).
pub fn repo_matches_spec(repo_path: &Path, spec: &str) -> bool {
    meta_core::git::query(repo_path, ["remote", "get-url", "origin"])
        .and_then(|url| url.parse::<meta_core::git::GitUrl>().ok()?.owner_repo())
        .is_some_and(|owner_repo| owner_repo.eq_ignore_ascii_case(spec))
}

pub fn ensure_worktrees_in_gitignore(
    meta_dir: &Path,
    worktrees_dirname: &str,
    quiet: bool,
) -> Result<()> {
    let gitignore_path = meta_dir.join(".gitignore");
    let pattern = format!("{worktrees_dirname}/");

    if gitignore_path.exists() {
        let content = std::fs::read_to_string(&gitignore_path)?;
        if content
            .lines()
            .any(|line| line.trim() == pattern.trim_end_matches('/') || line.trim() == pattern)
        {
            return Ok(()); // already present
        }
        // Append
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&gitignore_path)?;
        writeln!(file, "{pattern}")?;
    } else {
        std::fs::write(&gitignore_path, format!("{pattern}\n"))?;
    }
    if !quiet {
        log::info!(
            "Added '{}' to .gitignore",
            meta_core::display::escape(&pattern)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_worktree_name ──────────────────────────────

    #[test]
    fn validate_name_accepts_simple_alphanumeric() {
        assert!(validate_worktree_name("feature1").is_ok());
    }

    #[test]
    fn validate_name_accepts_hyphens_and_underscores() {
        assert!(validate_worktree_name("my-feature_v2").is_ok());
    }

    #[test]
    fn validate_name_rejects_empty() {
        let err = validate_worktree_name("").unwrap_err();
        assert!(err.to_string().contains("cannot be empty"));
    }

    #[test]
    fn validate_name_rejects_leading_dot() {
        let err = validate_worktree_name(".hidden").unwrap_err();
        assert!(err.to_string().contains("cannot start with '.'"));
    }

    #[test]
    fn validate_name_rejects_forward_slash() {
        let err = validate_worktree_name("a/b").unwrap_err();
        assert!(err.to_string().contains("path separators"));
    }

    #[test]
    fn validate_name_rejects_backslash() {
        let err = validate_worktree_name("a\\b").unwrap_err();
        assert!(err.to_string().contains("path separators"));
    }

    #[test]
    fn validate_name_rejects_special_characters() {
        let err = validate_worktree_name("feat@work").unwrap_err();
        assert!(err.to_string().contains("only ASCII alphanumeric"));
    }

    #[test]
    fn validate_name_rejects_spaces() {
        let err = validate_worktree_name("my feature").unwrap_err();
        assert!(err.to_string().contains("only ASCII alphanumeric"));
    }

    fn workspace(dir: &Path) -> Workspace {
        Workspace::in_dir(dir).unwrap().unwrap()
    }

    /// A user-given name in an error is shown escaped, so the error stays
    /// one line with no control characters.
    fn assert_escaped(err: anyhow::Error) {
        let err = err.to_string();
        assert!(!err.contains('\n') && !err.contains('\x1b'), "{err:?}");
    }

    #[test]
    fn from_pr_errors_escape_the_spec() {
        for spec in ["o/r#1\u{1b}[2J\nFORGED", "bad\u{1b}[2J\nFORGED"] {
            let message = format!("{:#}", resolve_from_pr(spec).unwrap_err());
            assert!(
                !message.contains('\n') && !message.contains('\u{1b}'),
                "{message}"
            );
        }
    }

    #[test]
    fn errors_escape_the_names_they_show() {
        for name in [
            "bad\nFORGED",
            ".bad\nFORGED",
            "a/\x1b]0;x\x07",
            "bad\x1b[2J",
        ] {
            assert_escaped(validate_worktree_name(name).unwrap_err());
        }

        let projects = [meta_core::config::ProjectInfo {
            name: "odd\x1b[2J".to_string(),
            path: "odd".to_string(),
            repo: None,
            tags: vec![],
            provides: vec![],
            depends_on: vec![],
            meta: false,
        }];
        let err = lookup_project(&projects, "a\nFORGED").unwrap_err();
        assert!(err.to_string().contains("'a\\nFORGED'"), "{err}");
        assert_escaped(err);

        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".meta.yaml"),
            r#"{"projects": {"vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}}}"#,
        )
        .unwrap();
        for alias in ["a\nFORGED", "vendor/\nFORGED", "vendor/\x1b[2J"] {
            assert_escaped(lookup_nested_project(&workspace(tmp.path()), alias).unwrap_err());
        }
    }

    #[test]
    fn repo_matches_spec_requires_the_exact_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| meta_core::git::run(tmp.path(), args).unwrap();
        run(&["init", "-q"]);
        run(&[
            "remote",
            "add",
            "origin",
            "git@github.com:Org/api-legacy.git",
        ]);
        assert!(!repo_matches_spec(tmp.path(), "org/api"));
        assert!(!repo_matches_spec(tmp.path(), "api-legacy"));
        assert!(repo_matches_spec(tmp.path(), "org/api-legacy"));
        // through an alias, as git fetches it
        run(&["remote", "set-url", "origin", "gh:api.git"]);
        run(&["config", "url.git@github.com:org/.insteadOf", "gh:"]);
        assert!(repo_matches_spec(tmp.path(), "org/api"));
    }

    // ── resolve_branch ──────────────────────────────────────

    #[test]
    fn resolve_branch_per_repo_takes_priority() {
        assert_eq!(
            resolve_branch("task", Some("flag-branch"), Some("repo-branch")),
            "repo-branch"
        );
    }

    #[test]
    fn resolve_branch_flag_fallback() {
        assert_eq!(
            resolve_branch("task", Some("flag-branch"), None),
            "flag-branch"
        );
    }

    #[test]
    fn resolve_branch_defaults_to_task_name() {
        assert_eq!(resolve_branch("my-task", None, None), "my-task");
    }

    // ── ensure_worktrees_in_gitignore ───────────────────────

    #[test]
    fn gitignore_creates_file_if_absent() {
        let tmp = tempfile::tempdir().unwrap();
        ensure_worktrees_in_gitignore(tmp.path(), ".worktrees", true).unwrap();
        let content = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(content.contains(".worktrees/"));
    }

    #[test]
    fn gitignore_appends_if_not_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "node_modules/\n").unwrap();
        ensure_worktrees_in_gitignore(tmp.path(), ".worktrees", true).unwrap();
        let content = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(content.contains("node_modules/"));
        assert!(content.contains(".worktrees/"));
    }

    #[test]
    fn gitignore_skips_if_already_present() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), ".worktrees/\n").unwrap();
        ensure_worktrees_in_gitignore(tmp.path(), ".worktrees", true).unwrap();
        let content = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        // Should appear exactly once
        assert_eq!(content.matches(".worktrees/").count(), 1);
    }

    // ── lookup_nested_project ───────────────────────────────

    #[test]
    fn lookup_nested_simple_alias_works() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".meta.yaml"),
            r#"{"projects": {"backend": "git@github.com:org/backend.git"}}"#,
        )
        .unwrap();
        std::fs::create_dir(tmp.path().join("backend")).unwrap();

        let (path, info) = lookup_nested_project(&workspace(tmp.path()), "backend").unwrap();
        assert_eq!(info.name, "backend");
        assert_eq!(path, tmp.path().join("backend"));
    }

    #[test]
    fn lookup_nested_nested_path_works() {
        let tmp = tempfile::tempdir().unwrap();
        let vendor = tmp.path().join("vendor");
        let nested = vendor.join("nested-lib");
        std::fs::create_dir_all(&nested).unwrap();

        // Root .meta.yaml - vendor is a nested meta repo (has repo URL + meta: true)
        std::fs::write(
            tmp.path().join(".meta.yaml"),
            r#"{"projects": {"vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}}}"#,
        )
        .unwrap();

        // Nested .meta.yaml inside vendor (simulates what exists after cloning vendor)
        std::fs::write(
            vendor.join(".meta.yaml"),
            r#"{"projects": {"nested-lib": "git@github.com:org/nested-lib.git"}}"#,
        )
        .unwrap();

        let (path, info) =
            lookup_nested_project(&workspace(tmp.path()), "vendor/nested-lib").unwrap();
        assert_eq!(info.name, "nested-lib");
        assert_eq!(path, tmp.path().join("vendor/nested-lib"));
    }

    #[test]
    fn lookup_nested_invalid_nested_path_fails() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("vendor")).unwrap();

        std::fs::write(
            tmp.path().join(".meta.yaml"),
            r#"{"projects": {"vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}}}"#,
        )
        .unwrap();

        let result = lookup_nested_project(&workspace(tmp.path()), "vendor/nonexistent");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown nested repo"));
        assert!(err.contains("vendor/nonexistent"));
    }

    #[test]
    fn lookup_nested_unknown_simple_alias_fails() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".meta.yaml"),
            r#"{"projects": {"backend": "git@github.com:org/backend.git"}}"#,
        )
        .unwrap();

        let result = lookup_nested_project(&workspace(tmp.path()), "nonexistent");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown repo alias"));
    }

    // ── build_project_map (via meta_core::config) ──────────────

    #[test]
    fn build_project_map_handles_nested_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let vendor = tmp.path().join("vendor");
        let nested = vendor.join("lib");
        std::fs::create_dir_all(&nested).unwrap();

        std::fs::write(
            tmp.path().join(".meta.yaml"),
            r#"{"projects": {"vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}}}"#,
        )
        .unwrap();
        std::fs::write(
            vendor.join(".meta.yaml"),
            r#"{"projects": {"lib": "git@github.com:org/lib.git"}}"#,
        )
        .unwrap();

        let tree = meta_core::config::walk_meta_tree(tmp.path(), None).unwrap();
        let map = meta_core::config::build_project_map(&tree, tmp.path(), "");

        // Should contain both vendor and vendor/lib
        assert!(map.contains_key("vendor"));
        assert!(map.contains_key("vendor/lib"));

        let (vendor_path, vendor_info) = map.get("vendor").unwrap();
        assert_eq!(vendor_info.name, "vendor");
        assert_eq!(*vendor_path, tmp.path().join("vendor"));

        let (lib_path, lib_info) = map.get("vendor/lib").unwrap();
        assert_eq!(lib_info.name, "lib");
        assert_eq!(*lib_path, tmp.path().join("vendor/lib"));
    }

    #[test]
    fn lookup_nested_with_custom_path_in_nested_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let vendor = tmp.path().join("vendor");
        let custom_path = vendor.join("packages/mylib");
        std::fs::create_dir_all(&custom_path).unwrap();

        // Root .meta.yaml - vendor is a nested meta repo
        std::fs::write(
            tmp.path().join(".meta.yaml"),
            r#"{"projects": {"vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}}}"#,
        )
        .unwrap();

        // Nested .meta.yaml with custom path
        std::fs::write(
            vendor.join(".meta.yaml"),
            r#"{"projects": {"mylib": {"repo": "git@github.com:org/mylib.git", "path": "packages/mylib"}}}"#,
        )
        .unwrap();

        // Lookup by the full path (vendor/packages/mylib)
        let (path, info) =
            lookup_nested_project(&workspace(tmp.path()), "vendor/packages/mylib").unwrap();
        assert_eq!(info.name, "mylib");
        assert_eq!(path, tmp.path().join("vendor/packages/mylib"));
    }

    #[test]
    fn lookup_nested_with_deeply_nested_meta_repos() {
        // Test 3 levels: root -> vendor -> sub-vendor -> deep-lib
        let tmp = tempfile::tempdir().unwrap();
        let vendor = tmp.path().join("vendor");
        let sub_vendor = vendor.join("sub-vendor");
        let deep_lib = sub_vendor.join("deep-lib");
        std::fs::create_dir_all(&deep_lib).unwrap();

        // Root tracks vendor
        std::fs::write(
            tmp.path().join(".meta.yaml"),
            r#"{"projects": {"vendor": {"repo": "git@github.com:org/vendor.git", "meta": true}}}"#,
        )
        .unwrap();

        // Vendor tracks sub-vendor
        std::fs::write(
            vendor.join(".meta.yaml"),
            r#"{"projects": {"sub-vendor": {"repo": "git@github.com:org/sub-vendor.git", "meta": true}}}"#,
        )
        .unwrap();

        // Sub-vendor tracks deep-lib
        std::fs::write(
            sub_vendor.join(".meta.yaml"),
            r#"{"projects": {"deep-lib": "git@github.com:org/deep-lib.git"}}"#,
        )
        .unwrap();

        // Lookup the deeply nested project
        let (path, info) =
            lookup_nested_project(&workspace(tmp.path()), "vendor/sub-vendor/deep-lib").unwrap();
        assert_eq!(info.name, "deep-lib");
        assert_eq!(path, tmp.path().join("vendor/sub-vendor/deep-lib"));
    }
}
