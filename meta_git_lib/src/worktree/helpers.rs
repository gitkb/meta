//! Utility functions for worktree management.

use anyhow::{Context, Result};
use meta_core::display::{escape, escape_path, program_output};
use std::path::{Path, PathBuf};

use meta_core::config::Workspace;

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
        let Some(source) = meta_core::config::set_source(&here.dir)?.filter(|s| s.is_dir()) else {
            return Ok(Self {
                owner: here.clone(),
                here,
            });
        };
        let owner = Workspace::in_dir(&source)?.unwrap_or_else(|| Workspace {
            config_path: source.join(meta_core::config::META_CONFIG_NAME),
            dir: source,
            projects: Vec::new(),
            defaults: Default::default(),
            worktrees_dir: None,
            worktree: Default::default(),
        });
        Ok(Self { here, owner })
    }
}

/// Where worktree sets go: `META_WORKTREES`, else the `worktrees_dir` of
/// `owner`'s config (anywhere; checked when it was loaded), else its
/// `.worktrees`; outside a workspace, `.worktrees` in `cwd`.
pub fn resolve_worktree_root(owner: Option<&Workspace>, cwd: &Path) -> Result<PathBuf> {
    if let Ok(env_path) = std::env::var("META_WORKTREES") {
        return Ok(PathBuf::from(env_path));
    }
    let Some(owner) = owner else {
        return Ok(cwd.join(".worktrees"));
    };
    // A link there is followed, wherever it leads (removal checks what is in
    // a set, not where it is), but not into git's data; a configured
    // worktrees_dir was checked when it was loaded
    let Some(configured) = &owner.worktrees_dir else {
        meta_core::config::check_location(&owner.dir, ".worktrees", false)
            .map_err(|why| anyhow::anyhow!("Refusing to keep worktrees in .worktrees: {why}"))?;
        return Ok(owner.dir.join(".worktrees"));
    };
    Ok(owner.dir.join(configured))
}

/// The directory of the existing worktree set `name`.
pub fn resolve_existing_worktree(name: &str) -> Result<PathBuf> {
    validate_worktree_name(name)?;
    let cwd = std::env::current_dir()?;
    let configs = Configs::find_optional(&cwd)?;
    let worktree_root = resolve_worktree_root(configs.as_ref().map(|c| &c.owner), &cwd)?;
    let wt_dir = worktree_root.join(name);
    if !wt_dir.exists() {
        anyhow::bail!("Worktree '{}' not found at {}", name, escape_path(&wt_dir));
    }
    Ok(wt_dir)
}

/// The directory of the named worktree set and its repos: an error if it
/// doesn't exist or holds none.
pub fn discover_and_validate_worktree(
    name: &str,
) -> Result<(PathBuf, Vec<meta_cli::worktree::WorktreeRepoInfo>)> {
    let wt_dir = resolve_existing_worktree(name)?;
    let repos = meta_cli::worktree::discover_worktree_repos(&wt_dir)?;
    if repos.is_empty() {
        anyhow::bail!("No repos found in worktree '{}'", escape(name));
    }
    Ok((wt_dir, repos))
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
        // a source outside the meta dir (`../sib`) sits at its alias, as
        // [`meta_core::config::ProjectInfo::set_place`] has it
        .filter(|place| {
            !place.as_os_str().is_empty()
                && place
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)))
        })
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

/// The repo (`owner/repo`) and number of a `--from-pr` value `owner/repo#N`.
pub fn parse_from_pr(from_pr: &str) -> Result<(String, u32)> {
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
    Ok((repo_spec.to_string(), pr_num))
}

/// Parse `--from-pr owner/repo#N` format and resolve the PR's head branch.
/// Returns (owner/repo, pr_number, head_branch_name).
pub fn resolve_from_pr(from_pr: &str) -> Result<(String, u32, String)> {
    use std::process::Command;

    let (repo_spec, pr_num) = parse_from_pr(from_pr)?;
    let repo_spec = repo_spec.as_str();

    // Resolve head branch via gh CLI: its answer is read, never coloured
    let output = Command::new("gh")
        .env_remove("CLICOLOR_FORCE")
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
            "Failed to resolve PR #{} in {}:{}",
            pr_num,
            escape(repo_spec),
            program_output(&stderr)
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
/// applied (an origin of `gh:repo` may be GitHub's `org/repo`). A repo
/// without `origin`, or not cloned, is no match.
pub fn repo_matches_spec(repo_path: &Path, spec: &str) -> bool {
    meta_core::git::run(repo_path, ["remote", "get-url", "origin"])
        .ok()
        .and_then(|url| meta_core::git::GitUrl::from(url).owner_repo())
        .is_some_and(|owner_repo| owner_repo.eq_ignore_ascii_case(spec))
}

/// Keep the worktrees folder `worktree_root` out of the git status of
/// `owner`, the checkout whose sets it holds, when it is inside it: as
/// written (a `.worktrees` link to another disk is still in it), or as the
/// two resolve (an absolute `worktrees_dir` spelled another way). It goes in
/// the repository's `info/exclude`, which is local and untracked, so nothing
/// the workspace commits changes. A folder outside `owner`, or an `owner`
/// that isn't a repository of its own (or one git can't use: it has no
/// status to keep the folder out of), needs nothing.
pub fn exclude_worktrees(owner: &Path, worktree_root: &Path) -> Result<()> {
    let Ok(Some(git_dir)) = meta_core::gitdir::common_dir(owner) else {
        return Ok(());
    };
    let plain = |rel: &Path| {
        rel.components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
    };
    let real = |path: &Path| path.canonicalize().ok();
    let inside = worktree_root
        .strip_prefix(owner)
        .ok()
        .filter(|rel| plain(rel))
        .map(Path::to_path_buf)
        .or_else(|| {
            let (root, owner) = (real(worktree_root)?, real(owner)?);
            Some(root.strip_prefix(owner).ok()?.to_path_buf()).filter(|rel| plain(rel))
        });
    let Some(rel) = inside
        .as_deref()
        .and_then(Path::to_str)
        .filter(|r| !r.is_empty())
    else {
        return Ok(());
    };
    // the path as a literal pattern from the top of the repository, for a
    // folder or a link alike: git reads `*?[\` as globs, and drops a
    // trailing space
    let mut pattern = String::from("/");
    for c in rel.replace('\\', "/").chars() {
        if matches!(c, '*' | '?' | '[' | '\\') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    if pattern.ends_with(' ') {
        pattern.insert(pattern.len() - 1, '\\');
    }
    let exclude = git_dir.join("info/exclude");
    let shown = || escape_path(&exclude);
    let content = match std::fs::read_to_string(&exclude) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("Cannot read {}", shown())),
    };
    if content.lines().any(|line| line == pattern) {
        return Ok(());
    }
    let separator = if content.is_empty() || content.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    if let Some(info) = exclude.parent() {
        std::fs::create_dir_all(info).with_context(|| format!("Cannot create {}", shown()))?;
    }
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude)
        .and_then(|mut file| writeln!(file, "{separator}{pattern}"))
        .with_context(|| format!("Cannot write {}", shown()))
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

    // ── exclude_worktrees ───────────────────────────────────

    fn exclude_of(repo: &Path) -> String {
        std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap_or_default()
    }

    #[test]
    fn worktrees_go_to_info_exclude_not_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        meta_core::git::run(tmp.path(), ["init", "-q"]).unwrap();
        std::fs::create_dir(tmp.path().join(".worktrees")).unwrap();
        std::fs::write(tmp.path().join(".worktrees/f"), "").unwrap();
        exclude_worktrees(tmp.path(), &tmp.path().join(".worktrees")).unwrap();
        exclude_worktrees(tmp.path(), &tmp.path().join(".worktrees")).unwrap();
        assert_eq!(exclude_of(tmp.path()).matches("/.worktrees").count(), 1);
        assert!(!tmp.path().join(".gitignore").exists());
        let status = meta_core::git::run(tmp.path(), ["status", "--porcelain"]).unwrap();
        assert_eq!(status, "");
    }

    #[test]
    fn a_gitignore_is_left_as_it_is() {
        let tmp = tempfile::tempdir().unwrap();
        meta_core::git::run(tmp.path(), ["init", "-q"]).unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "node_modules/").unwrap();
        // an exclude file without a last newline keeps its last line
        std::fs::write(tmp.path().join(".git/info/exclude"), "*.log").unwrap();
        std::fs::create_dir(tmp.path().join(".worktrees")).unwrap();
        exclude_worktrees(tmp.path(), &tmp.path().join(".worktrees")).unwrap();
        assert_eq!(exclude_of(tmp.path()), "*.log\n/.worktrees\n");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap(),
            "node_modules/"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_worktrees_link_is_excluded_by_its_name() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("ws");
        std::fs::create_dir_all(tmp.path().join("elsewhere")).unwrap();
        meta_core::git::run(tmp.path(), ["init", "-q", "ws"]).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("elsewhere"), repo.join(".worktrees")).unwrap();
        exclude_worktrees(&repo, &repo.join(".worktrees")).unwrap();
        assert!(exclude_of(&repo).lines().any(|l| l == "/.worktrees"));
        let status = meta_core::git::run(&repo, ["status", "--porcelain"]).unwrap();
        assert_eq!(status, "");
    }

    #[test]
    fn worktrees_outside_or_without_a_repository_need_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        exclude_worktrees(&ws, &ws.join(".worktrees")).unwrap();
        assert!(!ws.join(".git").exists());
        meta_core::git::run(&ws, ["init", "-q"]).unwrap();
        exclude_worktrees(&ws, &tmp.path().join("sets")).unwrap();
        assert!(!exclude_of(&ws).contains("sets"));
    }
}
