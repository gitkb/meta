//! The meta-mcp binary: its own flags, and the environment its tools run in.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// `command` in `dir` with no user configuration: none of git's, none of
/// meta's.
fn isolated(mut command: Command, dir: &Path) -> Command {
    command
        .current_dir(dir)
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("xdg"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env("GIT_CONFIG_GLOBAL", dir.join("no-gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("META_DATA_DIR", dir.join("meta-data"));
    command
}

/// git `args` in `dir`, with no user configuration.
fn git(dir: &Path, args: &[&str]) {
    let mut git = isolated(meta_core::git::git(dir), dir);
    let status = git
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

/// meta-mcp in `dir` with `args`, fed `input` on stdin, in an environment
/// with no user configuration.
fn meta_mcp(dir: &Path, args: &[&str], input: &str, env: &[(&str, &Path)]) -> Output {
    let mut child = isolated(Command::new(env!("CARGO_BIN_EXE_meta-mcp")), dir)
        .args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn answers_help_and_version() {
    let tmp = tempfile::tempdir().unwrap();
    let out = meta_mcp(tmp.path(), &["--version"], "", &[]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("meta-mcp {}", env!("CARGO_PKG_VERSION"))
    );

    let out = meta_mcp(tmp.path(), &["--help"], "", &[]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Usage: meta-mcp"));

    let out = meta_mcp(
        tmp.path(),
        &[],
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        &[],
    );
    let reply: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        reply["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION"),
        "{reply}"
    );
}

/// Run from a git hook, the server's build, test and clean still work in
/// each project's own repository.
#[test]
#[cfg(unix)]
fn build_test_and_clean_run_in_each_projects_own_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp = tmp.path().canonicalize().unwrap();
    let workspace = tmp.join("workspace");
    let project = workspace.join("a");
    std::fs::create_dir_all(&project).unwrap();
    git(&project, &["init", "-q"]);
    std::fs::write(
        workspace.join(".meta.yaml"),
        "projects:\n  a:\n    path: a\n",
    )
    .unwrap();
    std::fs::write(
        project.join("Makefile"),
        "all test clean:\n\t@git rev-parse --show-toplevel > $@\n",
    )
    .unwrap();
    let hook = tmp.join("hook");
    std::fs::create_dir(&hook).unwrap();
    git(&hook, &["init", "-q"]);

    for (tool, target) in [
        ("meta_build", "all"),
        ("meta_run_tests", "test"),
        ("meta_clean", "clean"),
    ] {
        let request = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": tool, "arguments": {}}});
        let out = meta_mcp(
            &workspace,
            &[],
            &request.to_string(),
            &[("GIT_DIR", &hook.join(".git")), ("GIT_WORK_TREE", &hook)],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let toplevel = std::fs::read_to_string(project.join(target)).unwrap();
        assert_eq!(Path::new(toplevel.trim()), project, "{tool}");
    }
}

/// One tool call's text, as JSON.
fn call(dir: &Path, tool: &str, arguments: serde_json::Value) -> serde_json::Value {
    let request = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": tool, "arguments": arguments}});
    let out = meta_mcp(dir, &[], &request.to_string(), &[]);
    let reply: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let text = reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    serde_json::from_str(text)
        .map_err(|e| format!("{e}: {reply}"))
        .unwrap()
}

#[test]
fn search_reads_the_pattern_as_a_pattern_and_skips_ignored_files() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let write = |path: &str, content: &str| {
        let path = ws.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    };
    let repo = |path: &str| {
        std::fs::create_dir_all(ws.join(path)).unwrap();
        git(&ws.join(path), &["init", "-q"]);
    };
    // the workspace's repository ignores its projects, as workspaces do
    repo(".");
    write(".gitignore", "/a/\n/broken/\n/m/\n/plain/\n");
    write(
        ".meta.yaml",
        "projects:\n  a: {}\n  broken: {}\n  m: {meta: true}\n  plain: {}\n",
    );
    // a project whose .git can't be read: its error, and the others' matches
    write("broken/.git", "not a gitdir\n");
    repo("a");
    write("a/.gitignore", "target/\n");
    write("a/src/lib.rs", "--flag\n");
    write("a/notes.txt", "--flag\n");
    write("a/target/out.rs", "--flag\n");
    // a repository nested in a meta project
    repo("m");
    write("m/.meta.yaml", "projects:\n  inner: {}\n");
    repo("m/inner");
    write("m/inner/y.rs", "--flag\n");
    // a project that is no repository of its own
    write("plain/x.rs", "--flag\n");

    assert_eq!(
        call(
            ws,
            "meta_search_code",
            serde_json::json!({"pattern": "--flag"})
        ),
        serde_json::json!([
            {"project": "a", "matches": ["notes.txt:1:--flag", "src/lib.rs:1:--flag"]},
            {"project": "broken", "error": format!(
                "Invalid .git file format at {}", ws.join("broken/.git").display()
            )},
            {"project": "m/inner", "matches": ["y.rs:1:--flag"]},
            {"project": "plain", "matches": ["x.rs:1:--flag"]}
        ])
    );
    let found = call(
        ws,
        "meta_search_code",
        serde_json::json!({"pattern": "--flag", "file_pattern": "*.rs"}),
    );
    assert_eq!(
        found[0]["matches"],
        serde_json::json!(["src/lib.rs:1:--flag"])
    );
    // a search git refuses is reported, not taken for no match
    let found = call(
        ws,
        "meta_search_code",
        serde_json::json!({"pattern": "\\("}),
    );
    assert_eq!(found.as_array().unwrap().len(), 5, "{found}");
    assert!(
        found[0]["error"].as_str().is_some_and(|e| !e.is_empty()),
        "{found}"
    );
}
