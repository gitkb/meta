//! meta-git subprocess plugin
//!
//! Logging is handled by `run_plugin()` which initializes env_logger.
//! Use RUST_LOG=meta_git_cli=debug for debug output.

use meta_plugin_protocol::{run_plugin, CommandResult, PluginDefinition, PluginRequest};

fn main() {
    run_plugin(PluginDefinition {
        info: meta_git_cli::plugin_info(),
        execute,
    });
}

fn execute(request: PluginRequest) -> CommandResult {
    meta_git_cli::execute_command(
        &request.command,
        &request.args,
        &request.projects,
        &request.options,
        &request.cwd,
    )
}
