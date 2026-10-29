use console::style;
use meta_core::display::escape;
use std::collections::BTreeSet;
use std::path::Path;

/// Discover unique SSH remote URLs from the .meta.yaml in the current directory.
/// Returns the full SSH URLs (preserving user, host, and port) so callers can
/// pass them directly to `establish_ssh_masters` without information loss.
/// Returns an empty list if no .meta.yaml is found or no SSH URLs exist.
pub fn discover_ssh_urls(cwd: &Path) -> Vec<String> {
    let Ok(Some(config_path)) = meta_core::config::find_meta_config(cwd, None) else {
        return vec![];
    };

    let Ok((projects, _ignore)) = meta_core::config::parse_meta_config(&config_path) else {
        return vec![];
    };

    let urls: BTreeSet<String> = projects
        .iter()
        .filter_map(|p| p.repo.as_ref())
        .filter(|repo| meta_git_lib::extract_ssh_host(repo).is_some())
        .cloned()
        .collect();

    urls.into_iter().collect()
}

/// A remote URL mismatch between .meta.yaml and the actual repo.
pub(crate) struct RemoteMismatch {
    pub name: String,
    /// The configured path (may differ from name for custom paths / nested repos).
    pub path: String,
    pub expected: String,
    pub actual: String,
}

/// Check child repos for remote URL mismatches against .meta.yaml.
/// Returns a list of mismatches found (non-interactive, no prompts).
pub(crate) fn find_remote_mismatches(cwd: &Path) -> Vec<RemoteMismatch> {
    let Ok(Some(config_path)) = meta_core::config::find_meta_config(cwd, None) else {
        return vec![];
    };

    let Ok((projects, _ignore)) = meta_core::config::parse_meta_config(&config_path) else {
        return vec![];
    };

    let mut mismatches = Vec::new();

    for project in &projects {
        let Some(expected_url) = &project.repo else {
            continue;
        };

        let repo_path = cwd.join(&project.path);
        if !repo_path.join(".git").exists() && !repo_path.exists() {
            continue;
        }

        let Some(actual_url) = meta_git_lib::get_remote_url(&repo_path) else {
            continue;
        };

        if !meta_git_lib::urls_match(&actual_url, expected_url) {
            mismatches.push(RemoteMismatch {
                name: project.name.clone(),
                path: project.path.clone(),
                expected: expected_url.clone(),
                actual: actual_url,
            });
        }
    }

    mismatches
}

/// Print warnings about remote URL mismatches (non-interactive).
pub(crate) fn warn_remote_mismatches(cwd: &Path) {
    let mismatches = find_remote_mismatches(cwd);

    if mismatches.is_empty() {
        return;
    }

    eprintln!(
        "{} Found {} remote URL mismatch{}:",
        style("⚠").yellow().bold(),
        mismatches.len(),
        if mismatches.len() == 1 { "" } else { "es" }
    );

    for m in &mismatches {
        eprintln!("  {}", style(escape(&m.name)).bold());
        eprintln!("    actual:   {}", style(escape(&m.actual)).red());
        eprintln!("    expected: {}", style(escape(&m.expected)).green());
    }

    eprintln!();
    eprintln!("  Fix manually with:");
    for m in &mismatches {
        eprintln!(
            "    git -C {} remote set-url origin {}",
            escape(&crate::helpers::shell_quote(&m.path)),
            escape(&crate::helpers::shell_quote(&m.expected))
        );
    }
}
