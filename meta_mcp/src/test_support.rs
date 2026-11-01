//! Workspaces for the tests, and calling a tool as a client does.

#![expect(
    clippy::disallowed_methods,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]

use crate::tools::{Reply, TOOLS};
use crate::McpServer;
use std::path::Path;
use std::process::Command;

/// Gives this test process (and the git and tools it runs) a home, git
/// config and meta data dir of its own, empty, so the user's settings
/// (hooks, identity, aliases) can't change a test. Once, before any test
/// here runs git; every value is the same whichever test sets it first,
/// and every test process shares them.
fn isolate() {
    static ISOLATED: std::sync::Once = std::sync::Once::new();
    ISOLATED.call_once(|| {
        let home = std::env::temp_dir().join("meta-mcp-tests");
        std::fs::create_dir_all(&home).unwrap();
        for (key, value) in [
            ("HOME", home.clone()),
            ("XDG_CONFIG_HOME", home.join("config")),
            ("XDG_DATA_HOME", home.join("data")),
            ("META_DATA_DIR", home.join("meta")),
        ] {
            std::env::set_var(key, value);
        }
        // a file no one can write, where there is one
        let global = if cfg!(unix) {
            std::path::PathBuf::from("/dev/null")
        } else {
            home.join("gitconfig")
        };
        std::env::set_var("GIT_CONFIG_GLOBAL", global);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    });
}

/// git `args` in `dir`, with an identity of its own; its stdout.
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    isolate();
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

/// A git repo at `dir` with one commit of `README`, and an identity of its
/// own for the commits tools make there.
pub(crate) fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "t"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    let name = dir.file_name().unwrap().to_string_lossy();
    std::fs::write(dir.join("README"), name.as_bytes()).unwrap();
    git(dir, &["add", "README"]);
    git(dir, &["commit", "-qm", "init"]);
}

/// A server for a workspace whose `.meta.yaml` is `config`.
pub(crate) fn server(config: &str) -> (McpServer, tempfile::TempDir) {
    isolate();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(".meta.yaml"), config).unwrap();
    (McpServer::in_dir(Some(tmp.path().to_path_buf())), tmp)
}

/// A meta workspace with projects `a` (tag `x`) and `b`, each a git repo
/// with one commit of `README`.
pub(crate) fn workspace() -> (McpServer, tempfile::TempDir) {
    let (server, tmp) = server(r#"{"projects":{"a":{"repo":"x","tags":["x"]},"b":"x"}}"#);
    for name in ["a", "b"] {
        init_repo(&tmp.path().join(name));
    }
    (server, tmp)
}

/// Calls `tool` with `arguments` as a client does.
pub(crate) fn call(
    server: &McpServer,
    tool: &str,
    arguments: serde_json::Value,
) -> anyhow::Result<Reply> {
    TOOLS
        .iter()
        .find(|t| t.name == tool)
        .unwrap()
        .call(server, arguments)
}

/// A tool's text as JSON, when it succeeded.
pub(crate) fn json(reply: anyhow::Result<Reply>) -> serde_json::Value {
    let reply = reply.unwrap();
    assert!(!reply.is_error, "{}", reply.text);
    serde_json::from_str(&reply.text).unwrap()
}

/// Waits until `done` holds, failing the test after 20 seconds.
#[cfg(unix)]
pub(crate) fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "waited too long for {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Whether the process `pid` is gone: not running, and reaped.
#[cfg(unix)]
pub(crate) fn gone(pid: &str) -> bool {
    !Command::new("kill")
        .args(["-0", pid])
        .output()
        .unwrap()
        .status
        .success()
}
