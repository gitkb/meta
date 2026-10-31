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
    let mut paths: Vec<String> = config::Workspace::find(cwd, None)?
        .map(|workspace| workspace.projects.into_iter().map(|p| p.path).collect())
        .unwrap_or_default();
    paths.sort();
    paths.insert(0, ".".to_string());
    Ok(paths)
}

/// `value` quoted for a POSIX shell, for commands meta prints for the user
/// to paste: plain words stay as they are.
pub(crate) fn shell_quote(value: &str) -> String {
    shlex::try_quote(value)
        .map(|quoted| quoted.into_owned())
        .unwrap_or_default()
}
