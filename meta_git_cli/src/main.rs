//! meta-git subprocess plugin
//!
//! Logging is handled by `run_plugin()` which initializes env_logger.
//! Use RUST_LOG=meta_git_cli=debug for debug output.

use meta_plugin::{run_plugin, Ctx, PluginDefinition, Request, Response};

fn main() {
    run_plugin(PluginDefinition {
        info: meta_git_cli::plugin_info(),
        execute,
        targeting: Some(meta_git_cli::targeting),
    });
}

fn execute(ctx: &Ctx, request: Request) -> meta_plugin::Result<Response> {
    meta_git_cli::execute_command(ctx, &request)
}
