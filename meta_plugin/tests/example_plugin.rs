//! The example plugin the plugin guide points to does what the guide says.

#![expect(clippy::expect_used, reason = "test helpers: a panic is a failed test")]

#[expect(dead_code, reason = "the example's main runs it as a program")]
#[path = "../examples/meta-hello.rs"]
mod hello;

use meta_plugin::{Answer, Ctx, ErrorCode, Request};

fn request(args: &[&str]) -> Request {
    serde_json::from_value(serde_json::json!({
        "command": "hello",
        "args": args,
        "cwd": "/ws",
        "scope": "selection",
        "projects": [
            {"name": "api", "path": "api", "abs": "/ws/api", "present": true},
            {"name": "web", "path": "apps/web", "abs": "/ws/apps/web", "present": true},
        ],
        "plugin_dir": "/p",
        "data_dir": "/d",
        "meta": {},
    }))
    .expect("a request")
}

fn call(args: &[&str]) -> anyhow::Result<meta_plugin::Response> {
    hello::execute(&Ctx::new(Default::default()), request(args))
}

#[test]
fn it_serves_its_file_word_and_plans_one_step_per_selected_repo() {
    let info = hello::info();
    assert_eq!(info.commands, ["hello"]);
    let planned: Vec<_> = match call(&["team"]).expect("a plan").answer {
        Answer::Plan(plan) => plan
            .steps
            .into_iter()
            .map(|s| (s.project, s.dir, s.argv))
            .collect(),
        _ => Vec::new(),
    };
    let argv = hello::echo("hello team");
    assert_eq!(
        planned,
        [
            (Some("api".to_string()), "/ws/api".into(), argv.clone()),
            (Some("apps/web".to_string()), "/ws/apps/web".into(), argv)
        ]
    );
}

#[test]
fn a_wrong_command_line_is_a_usage_error() {
    let error = call(&["a", "b"]).expect_err("refused");
    assert_eq!(meta_plugin::envelope::code_of(&error), ErrorCode::Usage);
}
