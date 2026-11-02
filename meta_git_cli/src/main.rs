//! meta-git subprocess plugin
//!
//! Logging is handled by `run_plugin()` which initializes env_logger.
//! Use RUST_LOG=meta_git_cli=debug for debug output.

use indexmap::IndexMap;
use meta_plugin_protocol::{
    run_plugin, CommandResult, PluginDefinition, PluginHelp, PluginInfo, PluginRequest,
};

fn main() {
    // Build command sections for help display
    let mut command_sections = IndexMap::new();

    // Adapted Commands - meta-specific behavior that differs from plain git
    let mut adapted = IndexMap::new();
    adapted.insert(
        "clone".to_string(),
        "Clone a meta repo and all child repos recursively".to_string(),
    );
    adapted.insert(
        "commit".to_string(),
        "Commit changes with optional per-repo messages".to_string(),
    );
    adapted.insert(
        "update".to_string(),
        "Fast-forward cloned repos (git pull --ff-only) and clone missing ones".to_string(),
    );
    adapted.insert(
        "worktree".to_string(),
        "Create isolated worktree sets for multi-repo branches".to_string(),
    );
    command_sections.insert(
        "Adapted Commands (meta-specific behavior)".to_string(),
        adapted,
    );

    // Pass-through Commands - runs git command in each repo
    let mut passthrough = IndexMap::new();
    passthrough.insert(
        "status".to_string(),
        "Show git status across all repos".to_string(),
    );
    command_sections.insert(
        "Pass-through Commands (runs git command in each repo)".to_string(),
        passthrough,
    );

    run_plugin(PluginDefinition {
        info: PluginInfo {
            name: "git".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            commands: vec![
                "git clone".to_string(),
                "git status".to_string(),
                "git update".to_string(),
                "git commit".to_string(),
                "git worktree".to_string(),
                "git worktree create".to_string(),
                "git worktree add".to_string(),
                "git worktree remove".to_string(),
                "git worktree list".to_string(),
                "git worktree status".to_string(),
                "git worktree diff".to_string(),
                "git worktree exec".to_string(),
                "git worktree prune".to_string(),
            ],
            description: Some("Git operations for meta repositories".to_string()),
            help: Some(PluginHelp {
                usage: "Usage: meta git <COMMAND> [OPTIONS]".to_string(),
                commands: IndexMap::new(), // Using command_sections instead
                command_sections,
                examples: vec![
                    "meta git clone https://github.com/org/meta-repo.git".to_string(),
                    "meta git status".to_string(),
                    "meta git commit -m \"Update all repos\"".to_string(),
                    "meta git commit --edit              # Per-repo messages".to_string(),
                    "meta git worktree create feature-x --repo api --repo web".to_string(),
                ],
                note: Some(
                    "For git commands not listed above, use: meta exec -- git <command>"
                        .to_string(),
                ),
            }),
        },
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
