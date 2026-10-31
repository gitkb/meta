//! Workspaces for the tests, and calling a tool as a client does.

use crate::tools::{Reply, TOOLS};
use crate::McpServer;
use std::path::Path;
use std::process::Command;

/// git `args` in `dir`, with an identity of its own; its stdout.
pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
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
    let name = dir.file_name().unwrap().to_str().unwrap();
    std::fs::write(dir.join("README"), name).unwrap();
    git(dir, &["add", "README"]);
    git(dir, &["commit", "-qm", "init"]);
}

/// A server for a workspace whose `.meta.yaml` is `config`.
pub(crate) fn server(config: &str) -> (McpServer, tempfile::TempDir) {
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
