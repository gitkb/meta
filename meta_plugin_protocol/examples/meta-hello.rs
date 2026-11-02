//! A whole meta plugin: `meta hello NAME` says hello in each selected repo.
//!
//! Build it with `cargo build --example meta-hello` and put
//! `target/debug/examples/meta-hello` on PATH or in a workspace's
//! `.meta/plugins/`: its file name makes it serve `meta hello`.

use meta_plugin_protocol::{
    has_help_token, run_plugin, CommandResult, PlannedCommand, PluginDefinition, PluginHelp,
    PluginInfo, PluginRequest,
};

fn main() {
    run_plugin(PluginDefinition {
        info: info(),
        execute,
    });
}

/// What the plugin answers to `--meta-plugin-info`.
pub(crate) fn info() -> PluginInfo {
    PluginInfo {
        name: "hello".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        commands: vec!["hello".to_string()],
        description: Some("Say hello in each repo".to_string()),
        help: Some(PluginHelp {
            usage: "meta hello NAME".to_string(),
            examples: vec![
                "meta hello world".to_string(),
                "meta --tag backend hello team".to_string(),
            ],
            ..PluginHelp::default()
        }),
    }
}

/// `--meta-plugin-exec`: a plan of one command per selected repo, which meta
/// runs. `request.projects` holds the repos' directories, selection applied.
pub(crate) fn execute(request: PluginRequest) -> CommandResult {
    // `meta hello world --help` reaches the plugin like any other arguments
    if has_help_token(&request.command, &request.args) {
        return CommandResult::ShowHelp(None);
    }
    // a bare `meta hello` (or `meta hello --help`) never gets here: meta
    // prints the plugin's help
    let [name] = request.args.as_slice() else {
        return CommandResult::Usage("meta hello takes one name".to_string());
    };
    let commands = request
        .projects
        .iter()
        .map(|dir| PlannedCommand {
            dir: dir.clone(),
            argv: vec!["echo".to_string(), format!("hello {name}")],
            env: None,
        })
        .collect();
    CommandResult::Plan(commands, None)
}
