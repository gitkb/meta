use crate::git_env;
use crate::helpers::get_project_directories_with_fallback;
use meta_plugin_protocol::{CommandResult, PlannedCommand, PluginRequestOptions};
use std::path::Path;

pub(crate) fn execute_git_status(
    projects: &[String],
    options: &PluginRequestOptions,
    cwd: &Path,
) -> anyhow::Result<CommandResult> {
    // Return an execution plan - let loop_lib handle execution, dry-run, and JSON output
    // Use projects from meta_cli if available (enables --recursive), otherwise read local .meta.yaml
    let dirs = get_project_directories_with_fallback(projects, cwd)?;

    // Set git-specific environment variables (pager, colors, prompts)
    let git_env = Some(git_env::git_env());

    let commands: Vec<PlannedCommand> = dirs
        .into_iter()
        .map(|dir| PlannedCommand {
            dir,
            argv: vec!["git".to_string(), "status".to_string()],
            env: git_env.clone(),
        })
        .collect();

    Ok(CommandResult::Plan(commands, Some(options.parallel)))
}
