//! meta-project subprocess plugin

use meta_plugin::{run_plugin, PluginDefinition};

fn main() {
    run_plugin(PluginDefinition {
        info: meta_project_cli::info(),
        execute: meta_project_cli::execute,
    });
}
