//! The example plugin the plugin guide points to does what the guide says.

#![expect(clippy::expect_used, reason = "test helpers: a panic is a failed test")]

#[expect(dead_code, reason = "the example's main runs it as a program")]
#[path = "../examples/meta-hello.rs"]
mod hello;

use meta_plugin_protocol::{CommandResult, PluginRequest};

fn request(args: &[&str]) -> PluginRequest {
    serde_json::from_value(serde_json::json!({
        "command": "hello",
        "args": args,
        "projects": ["/ws/api", "/ws/apps/web"],
    }))
    .expect("a request")
}

#[test]
fn it_serves_its_file_word_and_plans_one_command_per_selected_repo() {
    let info = hello::info();
    assert_eq!(info.commands, ["hello"]);
    let planned = match hello::execute(request(&["team"])) {
        CommandResult::Plan(commands, None) => {
            commands.into_iter().map(|c| (c.dir, c.argv)).collect()
        }
        _ => Vec::new(),
    };
    let argv = hello::echo("hello team");
    assert_eq!(
        planned,
        [
            ("/ws/api".to_string(), argv.clone()),
            ("/ws/apps/web".to_string(), argv)
        ]
    );
}

#[test]
fn a_wrong_command_line_is_a_usage_error() {
    assert!(matches!(
        hello::execute(request(&["a", "b"])),
        CommandResult::Usage(_)
    ));
}
