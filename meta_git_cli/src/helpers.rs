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

/// The repos of the workspace at or above `cwd`, its root first, as the
/// host lists them: from `cwd` where they are inside it, else in full.
pub(crate) fn get_project_directories(cwd: &Path) -> anyhow::Result<Vec<String>> {
    // Without a .meta.yaml there is only the root; a .meta.yaml that is refused is an
    // error, not a reason to act on the root alone
    let Some(workspace) = config::Workspace::find(cwd, None)? else {
        return Ok(vec![".".to_string()]);
    };
    let shown = |path: std::path::PathBuf| match path.strip_prefix(cwd) {
        Ok(inside) if inside.as_os_str().is_empty() => ".".to_string(),
        Ok(inside) => inside.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    };
    let mut projects: Vec<&str> = workspace.projects.iter().map(|p| p.path.as_str()).collect();
    projects.sort_unstable();
    Ok(std::iter::once(shown(workspace.dir.clone()))
        .chain(projects.into_iter().map(|p| shown(workspace.dir.join(p))))
        .collect())
}
