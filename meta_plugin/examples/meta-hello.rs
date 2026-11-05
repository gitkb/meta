//! A whole meta plugin: `meta hello NAME` says hello in each selected repo.
//!
//! Build it with `cargo build --example meta-hello` and put
//! `target/debug/examples/meta-hello` on PATH or in a workspace's
//! `.meta/plugins/`: its file name makes it serve `meta hello`.

use meta_plugin::{
    has_help_token, run_plugin, Ctx, Plan, PluginDefinition, PluginHelp, PluginInfo, Request,
    Response, Step,
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

/// `--meta-plugin-exec`: a plan of one step per selected repo, which meta
/// runs. `request.projects` holds the repos, selection applied.
pub(crate) fn execute(_ctx: &Ctx, request: Request) -> anyhow::Result<Response> {
    // `meta hello world --help` reaches the plugin like any other arguments
    if has_help_token(&request.command, &request.args) {
        return Ok(Response::message(meta_plugin::help_text("hello", &info())));
    }
    // a bare `meta hello` (or `meta hello --help`) never gets here: meta
    // prints the plugin's help
    let [name] = request.args.as_slice() else {
        return Err(meta_plugin::usage("meta hello takes one name"));
    };
    let steps = Step::each(&request, &echo(&format!("hello {name}")));
    Ok(Plan {
        steps,
        ..Plan::default()
    }
    .into())
}

/// A command that prints `text`: meta runs plans with no shell, and on
/// Windows `echo` is part of cmd.
pub(crate) fn echo(text: &str) -> Vec<String> {
    let echo: &[&str] = if cfg!(windows) {
        &["cmd", "/c", "echo"]
    } else {
        &["echo"]
    };
    echo.iter()
        .map(|word| word.to_string())
        .chain([text.to_string()])
        .collect()
}
