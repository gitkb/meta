use meta_core::config;
use std::path::Path;

/// Get project directories - uses passed-in list if non-empty, otherwise reads local .meta.yaml
pub(crate) fn get_project_directories_with_fallback(
    projects: &[String],
    cwd: &Path,
) -> anyhow::Result<Vec<String>> {
    if !projects.is_empty() {
        // Use the projects list from meta_cli (supports --recursive)
        Ok(projects.to_vec())
    } else {
        // Fall back to reading local .meta.yaml file
        get_project_directories(cwd)
    }
}

pub(crate) fn get_project_directories(cwd: &Path) -> anyhow::Result<Vec<String>> {
    // Without a .meta.yaml there is only the root; a .meta.yaml that is refused is an
    // error, not a reason to act on the root alone
    if config::find_meta_config(cwd, None)?.is_none() {
        return Ok(vec![".".to_string()]);
    }
    let tree = config::walk_meta_tree(cwd, Some(0))?;
    let mut dirs = vec![".".to_string()];
    let mut paths: Vec<String> = tree.iter().map(|n| n.info.path.clone()).collect();
    paths.sort();
    dirs.extend(paths);
    Ok(dirs)
}

/// `value` quoted for a POSIX shell, for commands meta prints for the user
/// to paste: plain words stay as they are.
pub(crate) fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':'))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
