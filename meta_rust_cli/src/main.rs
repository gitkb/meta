//! meta-cargo subprocess plugin

use indexmap::IndexMap;
use meta_plugin::{
    run_plugin, CommandResult, PluginDefinition, PluginHelp, PluginInfo, PluginRequest,
};

fn main() {
    run_plugin(PluginDefinition {
        info: PluginInfo {
            name: "cargo".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            commands: vec!["cargo".to_string()],
            description: Some("Run cargo across the Rust repos of a meta workspace".to_string()),
            help: Some(PluginHelp {
                usage: "Usage: meta cargo <CARGO_ARGS>...".to_string(),
                commands: IndexMap::new(),
                runs_as_typed: Some("cargo".to_string()),
                examples: vec![
                    "meta cargo build --release".to_string(),
                    "meta cargo fmt --check".to_string(),
                    "meta --include api cargo clippy --all-targets -- -D warnings".to_string(),
                    "meta cargo --version".to_string(),
                ],
                note: Some(
                    "Repos without a Cargo.toml are skipped. A Cargo workspace that spans repos \
                     runs in each of them; for one run from its root, use plain cargo there."
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
