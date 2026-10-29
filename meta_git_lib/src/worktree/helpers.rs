//! Utility functions for worktree management.

use anyhow::{Context, Result};
use meta_core::display::{escape, escape_path};
use std::path::{Path, PathBuf};

use super::types::WorktreeContext;

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

pub fn resolve_worktree_root(meta_dir: Option<&Path>) -> Result<PathBuf> {
    // 1. Check META_WORKTREES env var
    if let Ok(env_path) = std::env::var("META_WORKTREES") {
        return Ok(PathBuf::from(env_path));
    }
    // 2. Check worktrees_dir in .meta.yaml. In a worktree set, of the
    // checkout it stands for: its sets live where that one's do
    if let Some(dir) = meta_dir.map(owner_dir) {
        let dir = dir.as_path();
        // The config's rules for the worktree directory (inside the
        // workspace, not inside a project, not into .git) hold here too,
        // whoever ran this plugin
        if let Some(config) = meta_core::config::find_meta_config_in(dir)? {
            meta_core::config::parse_meta_config(&config)?;
        }
        if let Some(configured) = read_worktrees_dir_from_config(dir)? {
            // Worktrees are created and removed there, so a committed config
            // may not point it outside the workspace
            let relative = meta_core::config::relative_dir(dir, &configured).map_err(|why| {
                anyhow::anyhow!(
                    "Invalid worktrees_dir in the .meta.yaml of {}: {why} (use META_WORKTREES \
                     for a directory elsewhere)",
                    escape_path(dir)
                )
            })?;
            return Ok(dir.join(relative));
        }
        // 3. Default: .worktrees/ relative to meta root, held to the same
        // rules: a committed symlink there must not send worktrees (and their
        // removal) outside the workspace or into .git
        let root = dir.join(".worktrees");
        meta_core::config::relative_dir(dir, ".worktrees").map_err(|why| {
            anyhow::anyhow!(
                "Refusing to keep worktrees in {}: {why}",
                escape_path(&root)
            )
        })?;
        return Ok(root);
    }
    // Fallback if no meta dir found
    let cwd = std::env::current_dir()?;
    Ok(cwd.join(".worktrees"))
}

/// The config file in `meta_dir` (see
/// [`meta_core::config::find_meta_config_in`]) as a JSON value, if there is
/// one. One that can't be read or parsed is an error.
pub fn read_meta_config_value(meta_dir: &Path) -> Result<Option<serde_json::Value>> {
    let Some(path) = meta_core::config::find_meta_config_in(meta_dir)? else {
        return Ok(None);
    };
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Cannot read {}", escape_path(&path)))?;
    serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&content)
        .map_err(anyhow::Error::from)
        .and_then(|v| serde_json::to_value(v).map_err(anyhow::Error::from))
        .map(Some)
        .with_context(|| format!("Cannot parse {}", escape_path(&path)))
}

pub fn read_worktrees_dir_from_config(meta_dir: &Path) -> Result<Option<String>> {
    Ok(read_meta_config_value(meta_dir)?
        .and_then(|config| config.get("worktrees_dir")?.as_str().map(str::to_string)))
}

pub fn find_meta_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    meta_core::config::nearest_meta_dir(&cwd).ok().flatten()
}

/// Like `find_meta_dir()` but returns an error if not found.
pub fn require_meta_dir() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    meta_core::config::nearest_meta_dir(&cwd)?
        .ok_or_else(|| anyhow::anyhow!("Not inside a meta project (no .meta.yaml found)"))
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

/// Resolve worktree context for a named worktree.
/// Returns meta_dir (for hooks), worktree_root, and the specific worktree directory.
pub fn resolve_worktree_context(name: &str) -> Result<WorktreeContext> {
    let meta_dir = find_meta_dir();
    let worktree_root = resolve_worktree_root(meta_dir.as_deref())?;
    let wt_dir = worktree_set_dir(&worktree_root, name)?;
    Ok(WorktreeContext {
        meta_dir,
        worktree_root,
        wt_dir,
    })
}

/// Resolve worktree context and verify the worktree exists.
pub fn resolve_existing_worktree(name: &str) -> Result<WorktreeContext> {
    validate_worktree_name(name)?;
    let ctx = resolve_worktree_context(name)?;
    if !ctx.wt_dir.exists() {
        anyhow::bail!(
            "Worktree '{}' not found at {}",
            name,
            escape_path(&ctx.wt_dir)
        );
    }
    Ok(ctx)
}

/// Resolve, validate existence, and discover repos in a named worktree.
/// Returns a non-empty repo list, or errors if worktree doesn't exist
/// or contains no repos.
pub fn discover_and_validate_worktree(
    name: &str,
) -> Result<Vec<meta_cli::worktree::WorktreeRepoInfo>> {
    let ctx = resolve_existing_worktree(name)?;
    let repos = meta_cli::worktree::discover_worktree_repos(&ctx.wt_dir)?;
    if repos.is_empty() {
        anyhow::bail!("No repos found in worktree '{}'", escape(name));
    }
    Ok(repos)
}

/// Load and parse the .meta.yaml, returning the project list.
pub fn load_projects(meta_dir: &Path) -> Result<Vec<meta_core::config::ProjectInfo>> {
    let config_path = meta_core::config::find_meta_config(meta_dir, None)?
        .ok_or_else(|| anyhow::anyhow!("No .meta.yaml found in {}", escape_path(meta_dir)))?;
    let (projects, _) = meta_core::config::parse_meta_config(&config_path)?;
    Ok(projects)
}

/// Load projects and optionally include the root repo as ".".
///
/// When `include_root` is true and the meta_dir is itself a git repository,
/// the root repo is prepended to the list as "." (alias "."). This ensures
/// the root repo (which child repos may depend on) is processed first.
///
/// The root repo is an implicit dependency of child repos because it contains:
/// - Workspace configuration (Cargo.toml)
/// - Shared config files (.claude/, etc.)
/// - Build scripts and other shared resources
pub fn load_projects_with_root(
    meta_dir: &Path,
    include_root: bool,
) -> Result<Vec<meta_core::config::ProjectInfo>> {
    let mut projects = load_projects(meta_dir)?;

    if include_root && meta_dir.join(".git").exists() {
        // Prepend root repo so it's processed first (dependencies come first)
        projects.insert(
            0,
            meta_core::config::ProjectInfo {
                name: ".".to_string(),
                path: ".".to_string(),
                repo: None, // Root repo doesn't have a remote URL in this context
                tags: vec![],
                provides: vec![],
                depends_on: vec![],
                meta: false,
            },
        );
    }

    Ok(projects)
}

/// Look up a project by alias, returning an error with valid aliases on miss.
pub fn lookup_project<'a>(
    projects: &'a [meta_core::config::ProjectInfo],
    alias: &str,
) -> Result<&'a meta_core::config::ProjectInfo> {
    projects.iter().find(|p| p.name == alias).ok_or_else(|| {
        let valid: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        anyhow::anyhow!(
            "Unknown repo alias: '{}'. Valid aliases: {}",
            escape(alias),
            escape(&valid.join(", "))
        )
    })
}

/// The meta directory whose sets and settings (such as hooks) `meta_dir`'s
/// are: inside a set, the checkout the set stands for, which outlives it.
pub fn owner_dir(meta_dir: &Path) -> PathBuf {
    meta_core::config::set_source(meta_dir)
        .filter(|source| source.is_dir())
        .unwrap_or_else(|| meta_dir.to_path_buf())
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
pub fn lookup_nested_project(
    meta_dir: &Path,
    alias: &str,
) -> Result<(PathBuf, meta_core::config::ProjectInfo)> {
    // If alias contains '/', use recursive lookup
    if alias.contains('/') {
        let tree = meta_core::config::walk_meta_tree(meta_dir, None)?;

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
        let projects = load_projects(meta_dir)?;
        let project = lookup_project(&projects, alias)?;
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

/// Parse a human-friendly duration string to seconds.
/// Supported formats: "30s", "5m", "1h", "2d", "1w", or bare seconds "300".
/// The result always fits in an `i64`, so timestamp arithmetic can't wrap.
pub fn parse_duration(s: &str) -> Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        anyhow::bail!("Empty duration string");
    }

    // Bare number: treat as seconds
    let (num_str, multiplier) = if s.chars().all(|c| c.is_ascii_digit()) {
        (s, 1)
    } else {
        let suffix_at = s.len() - s.chars().last().map_or(0, char::len_utf8);
        let (num_str, suffix) = s.split_at(suffix_at);
        let multiplier = match suffix {
            "s" => 1,
            "m" => 60,
            "h" => 3600,
            "d" => 86400,
            "w" => 604800,
            _ => anyhow::bail!(
                "Invalid duration suffix '{}'. Use s (seconds), m (minutes), h (hours), d (days), or w (weeks)",
                escape(suffix)
            ),
        };
        (num_str, multiplier)
    };
    let num: u64 = num_str.parse().with_context(|| {
        format!(
            "Invalid duration '{}': use a number with an optional s, m, h, d or w suffix",
            escape(s)
        )
    })?;

    num.checked_mul(multiplier)
        .filter(|secs| i64::try_from(*secs).is_ok())
        .ok_or_else(|| anyhow::anyhow!("Duration '{}' is too large", escape(s)))
}

/// Format seconds into a human-friendly duration string.
/// Returns the largest appropriate unit (e.g., "2h" not "7200s").
pub fn format_duration(secs: i64) -> String {
    let abs_secs = secs.unsigned_abs();
    let prefix = if secs < 0 { "-" } else { "" };

    if abs_secs >= 604800 && abs_secs.is_multiple_of(604800) {
        let weeks = abs_secs / 604800;
        format!("{prefix}{weeks}w")
    } else if abs_secs >= 86400 && abs_secs.is_multiple_of(86400) {
        let days = abs_secs / 86400;
        format!("{prefix}{days}d")
    } else if abs_secs >= 3600 && abs_secs.is_multiple_of(3600) {
        let hours = abs_secs / 3600;
        format!("{prefix}{hours}h")
    } else if abs_secs >= 60 && abs_secs.is_multiple_of(60) {
        let mins = abs_secs / 60;
        format!("{prefix}{mins}m")
    } else {
        format!("{prefix}{abs_secs}s")
    }
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
pub fn repo_matches_spec(repo_path: &Path, spec: &str) -> bool {
    use std::process::Command;

    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_path)
        .output();

    match output {
        Ok(o) if o.status.success() => remote_owner_repo(&String::from_utf8_lossy(&o.stdout))
            .is_some_and(|owner_repo| owner_repo.eq_ignore_ascii_case(spec)),
        _ => false,
    }
}

/// The trailing `owner/repo` of a remote URL such as `git@github.com:owner/repo.git`,
/// `https://github.com/owner/repo`, `ssh://git@host/owner/repo.git` or a local path.
fn remote_owner_repo(url: &str) -> Option<String> {
    let path = url.trim().trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.rsplit(['/', ':', '\\']);
    let repo = parts.next().filter(|p| !p.is_empty())?;
    let owner = parts.next().filter(|p| !p.is_empty())?;
    Some(format!("{owner}/{repo}"))
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
            assert_escaped(lookup_nested_project(tmp.path(), alias).unwrap_err());
        }
    }

    // ── parse_duration ──────────────────────────────────────

    #[test]
    fn parse_duration_bare_seconds() {
        assert_eq!(parse_duration("300").unwrap(), 300);
    }

    #[test]
    fn parse_duration_seconds_suffix() {
        assert_eq!(parse_duration("30s").unwrap(), 30);
    }

    #[test]
    fn parse_duration_minutes() {
        assert_eq!(parse_duration("5m").unwrap(), 300);
    }

    #[test]
    fn parse_duration_hours() {
        assert_eq!(parse_duration("2h").unwrap(), 7200);
    }

    #[test]
    fn parse_duration_days() {
        assert_eq!(parse_duration("1d").unwrap(), 86400);
    }

    #[test]
    fn parse_duration_weeks() {
        assert_eq!(parse_duration("1w").unwrap(), 604800);
    }

    #[test]
    fn parse_duration_trims_whitespace() {
        assert_eq!(parse_duration("  5m  ").unwrap(), 300);
    }

    #[test]
    fn parse_duration_rejects_empty() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("   ").is_err());
    }

    #[test]
    fn parse_duration_rejects_invalid_suffix() {
        let err = parse_duration("10x").unwrap_err();
        assert!(err.to_string().contains("Invalid duration suffix"));
    }

    #[test]
    fn parse_duration_rejects_non_numeric() {
        assert!(parse_duration("abcs").is_err());
        assert!(parse_duration("5é").is_err());
    }

    #[test]
    fn parse_duration_rejects_values_that_overflow() {
        for input in [
            "18446744073709551615",
            "30500000000000w",
            "9223372036854775808",
        ] {
            let err = parse_duration(input).unwrap_err();
            assert!(err.to_string().contains("too large"), "{input}: {err}");
        }
        assert_eq!(
            parse_duration("9223372036854775807").unwrap(),
            i64::MAX as u64
        );
    }

    // ── remote_owner_repo ───────────────────────────────────

    #[test]
    fn remote_owner_repo_parses_common_url_forms() {
        for url in [
            "git@github.com:org/api.git",
            "https://github.com/org/api",
            "https://github.com/org/api.git/\n",
            "ssh://git@github.example.com:22/org/api.git",
            "C:\\remotes\\org\\api.git",
        ] {
            assert_eq!(remote_owner_repo(url).as_deref(), Some("org/api"), "{url}");
        }
        assert_eq!(
            remote_owner_repo("git@github.com:org/api-legacy.git").as_deref(),
            Some("org/api-legacy")
        );
        assert_eq!(remote_owner_repo("api"), None);
    }

    #[test]
    fn repo_matches_spec_requires_the_exact_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(tmp.path())
                .output()
                .unwrap()
        };
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
    }

    // ── format_duration ─────────────────────────────────────

    #[test]
    fn format_duration_seconds() {
        assert_eq!(format_duration(45), "45s");
    }

    #[test]
    fn format_duration_exact_minutes() {
        assert_eq!(format_duration(300), "5m");
    }

    #[test]
    fn format_duration_exact_hours() {
        assert_eq!(format_duration(7200), "2h");
    }

    #[test]
    fn format_duration_exact_days() {
        assert_eq!(format_duration(86400), "1d");
    }

    #[test]
    fn format_duration_exact_weeks() {
        assert_eq!(format_duration(604800), "1w");
    }

    #[test]
    fn format_duration_non_exact_falls_to_seconds() {
        // 90 seconds = 1m30s, but since 90 % 60 != 0, falls through to seconds
        assert_eq!(format_duration(90), "90s");
    }

    #[test]
    fn format_duration_zero() {
        assert_eq!(format_duration(0), "0s");
    }

    #[test]
    fn format_duration_negative() {
        assert_eq!(format_duration(-3600), "-1h");
    }

    #[test]
    fn format_duration_negative_seconds() {
        assert_eq!(format_duration(-45), "-45s");
    }

    // ── parse_duration / format_duration round-trip ──────────

    #[test]
    fn duration_round_trip_exact_units() {
        for input in &["30s", "5m", "2h", "1d", "1w"] {
            let secs = parse_duration(input).unwrap();
            let formatted = format_duration(secs as i64);
            assert_eq!(&formatted, input, "round-trip failed for {input}");
        }
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

        let (path, info) = lookup_nested_project(tmp.path(), "backend").unwrap();
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

        let (path, info) = lookup_nested_project(tmp.path(), "vendor/nested-lib").unwrap();
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

        let result = lookup_nested_project(tmp.path(), "vendor/nonexistent");
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

        let result = lookup_nested_project(tmp.path(), "nonexistent");
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
        let (path, info) = lookup_nested_project(tmp.path(), "vendor/packages/mylib").unwrap();
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
        let (path, info) = lookup_nested_project(tmp.path(), "vendor/sub-vendor/deep-lib").unwrap();
        assert_eq!(info.name, "deep-lib");
        assert_eq!(path, tmp.path().join("vendor/sub-vendor/deep-lib"));
    }
}
