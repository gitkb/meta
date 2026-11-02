//! Workspaces for the tests, and calling a tool as a client does.

use crate::tools::{Reply, TOOLS};
use crate::McpServer;
use meta_test_support::Sandbox;
#[cfg(unix)]
pub(crate) use meta_test_support::{gone, wait_for};
use std::path::PathBuf;

/// A server for a workspace whose `.meta.yaml` is `config`.
pub(crate) fn server(sandbox: &Sandbox, config: &str) -> (McpServer, PathBuf) {
    let root = sandbox.work().join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".meta.yaml"), config).unwrap();
    (McpServer::in_dir(Some(root.clone())), root)
}

/// A meta workspace with projects `a` (tag `x`) and `b`, each a git repo
/// with one commit of `README`.
pub(crate) fn workspace(sandbox: &Sandbox) -> (McpServer, PathBuf) {
    let (server, root) = server(
        sandbox,
        r#"{"projects":{"a":{"repo":"x","tags":["x"]},"b":"x"}}"#,
    );
    for name in ["a", "b"] {
        sandbox.repo(&root.join(name));
    }
    (server, root)
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
