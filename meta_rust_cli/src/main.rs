//! meta-rust subprocess plugin

use indexmap::IndexMap;
use meta_plugin_protocol::{
    run_plugin, CommandResult, PluginDefinition, PluginHelp, PluginInfo, PluginRequest,
};

fn main() {
    let mut help_commands = IndexMap::new();
    help_commands.insert(
        "cargo".to_string(),
        "Run cargo with your args in each selected repo that has a Cargo.toml".to_string(),
    );

    run_plugin(PluginDefinition {
        info: PluginInfo {
            name: "rust".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            commands: vec!["cargo".to_string()],
            description: Some("Run cargo across the Rust repos of a meta workspace".to_string()),
            help: Some(PluginHelp {
                usage: "meta cargo <cargo args>".to_string(),
                commands: help_commands,
                command_sections: IndexMap::new(),
                examples: vec![
                    "meta cargo build --release".to_string(),
                    "meta cargo fmt --check".to_string(),
                    "meta --include api cargo clippy --all-targets -- -D warnings".to_string(),
                    "meta cargo --version".to_string(),
                ],
                note: Some(
                    "Repos without a Cargo.toml are skipped. A Cargo workspace that spans repos \
                     runs in each of them; for one run from its root, use plain cargo there. \
                     For cargo's own help: cargo help <command>"
                        .to_string(),
                ),
            }),
        },
        execute,
    });
}

fn execute(request: PluginRequest) -> CommandResult {
    meta_rust_cli::execute_command(
        &request.args,
        &request.options,
        &request.projects,
        &request.cwd,
    )
}
