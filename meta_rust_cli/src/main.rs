//! meta-cargo subprocess plugin

use indexmap::IndexMap;
use meta_plugin::{run_plugin, Ctx, PluginDefinition, PluginHelp, PluginInfo, Request, Response};

fn main() {
    run_plugin(PluginDefinition {
        info: PluginInfo {
            name: "cargo".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            commands: vec!["cargo".to_string()],
            description: Some("Run cargo across the Rust repos of a meta workspace".to_string()),
            help: Some(PluginHelp {
                usage: "Usage: meta [OPTIONS] cargo <CARGO_ARGS>...".to_string(),
                commands: IndexMap::new(),
                runs_as_typed: Some("cargo".to_string()),
                examples: vec![
                    "meta cargo build --release".to_string(),
                    "meta cargo fmt --check".to_string(),
                    "meta --include api cargo clippy --all-targets -- -D warnings".to_string(),
                    "meta cargo --version".to_string(),
                ],
                note: Some(
                    "It runs in each selected repo that has a Cargo.toml. One without is \
                     skipped; one not cloned is a warning (with --strict, an error). A Cargo \
                     workspace that spans repos runs in each of them; for one run from its \
                     root, use plain cargo there."
                        .to_string(),
                ),
            }),
            answers_targeting: false,
        },
        execute,
        targeting: None,
    });
}

fn execute(ctx: &Ctx, request: Request) -> meta_plugin::Result<Response> {
    meta_rust_cli::execute_command(ctx, &request)
}
