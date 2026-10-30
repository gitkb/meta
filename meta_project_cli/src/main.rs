//! meta-project subprocess plugin

use indexmap::IndexMap;
use meta_cli::display::escape_lines;
use meta_plugin_protocol::{
    run_plugin, CommandResult, PluginDefinition, PluginHelp, PluginInfo, PluginRequest,
};
use std::path::PathBuf;

fn main() {
    let mut help_commands = IndexMap::new();
    help_commands.insert(
        "list".to_string(),
        "List all projects defined in .meta.yaml (alias: ls)".to_string(),
    );
    help_commands.insert(
        "check".to_string(),
        "Verify all projects are cloned and consistent".to_string(),
    );
    help_commands.insert(
        "dependents".to_string(),
        "List projects that depend on a given project".to_string(),
    );

    run_plugin(PluginDefinition {
        info: PluginInfo {
            name: "project".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            commands: vec![
                "project list".to_string(),
                "project ls".to_string(),
                "project check".to_string(),
                "project dependents".to_string(),
            ],
            description: Some("Project inspection for meta repositories".to_string()),
            help: Some(PluginHelp {
                usage: "meta project <command> [args...]".to_string(),
                commands: help_commands,
                command_sections: IndexMap::new(),
                examples: vec![
                    "meta project list".to_string(),
                    "meta project list --json".to_string(),
                    "meta project list --recursive".to_string(),
                    "meta project check".to_string(),
                ],
                note: Some("To clone missing projects, use: meta git update".to_string()),
            }),
        },
        execute,
    });
}

/// Errors carry paths and names from git and the filesystem, so they are
/// escaped before they reach the terminal.
fn execute(request: PluginRequest) -> CommandResult {
    match execute_request(request) {
        CommandResult::Error(e) => CommandResult::Error(escape_lines(&e).into_owned()),
        other => other,
    }
}

fn execute_request(request: PluginRequest) -> CommandResult {
    let cwd = if request.cwd.is_empty() {
        match std::env::current_dir() {
            Ok(d) => d,
            Err(e) => return CommandResult::Error(format!("Failed to get working directory: {e}")),
        }
    } else {
        PathBuf::from(&request.cwd)
    };

    let options = meta_project_cli::ExecuteOptions {
        dry_run: request.options.dry_run,
        json_output: request.options.json_output,
        recursive: request.options.recursive,
        depth: request.options.depth,
        verbose: request.options.verbose,
        parallel: request.options.parallel,
        filtered: request.options.include_filters.is_some()
            || request.options.exclude_filters.is_some(),
    };

    meta_project_cli::execute_command(
        &request.command,
        &request.args,
        &options,
        &request.projects,
        &cwd,
    )
}
