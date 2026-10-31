//! The meta-mcp binary: its own flags, and the environment its tools run in.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// meta-mcp in `dir` with `args`, fed `input` on stdin, in an environment
/// with no user configuration.
fn meta_mcp(dir: &Path, args: &[&str], input: &str, env: &[(&str, &Path)]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_meta-mcp"))
        .args(args)
        .current_dir(dir)
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("xdg"))
        .env("GIT_CONFIG_GLOBAL", dir.join("no-gitconfig"))
        .env("META_DATA_DIR", dir.join("meta-data"))
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
    let git = |dir: &Path, args: &[&str]| {
        let status = meta_core::git::git(dir).args(args).status().unwrap();
        assert!(status.success());
    };
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
