//! Workspaces for the tests, and calling a tool as a client does.

use crate::tools::{Reply, TOOLS};
use crate::McpServer;
use meta_plugin::testing::Sandbox;
#[cfg(unix)]
pub(crate) use meta_plugin::testing::{gone, wait_for};
use std::path::PathBuf;

/// Have the tools call the `meta` built beside the tests (in the target
/// folder, above `deps/`). Each test that does runs in a process of its own
/// (`isolated!`).
pub(crate) fn use_built_meta() {
    let exe = std::env::current_exe().unwrap();
    let meta = exe
        .ancestors()
        .nth(2)
        .unwrap()
        .join(format!("meta{}", std::env::consts::EXE_SUFFIX));
    assert!(
        meta.is_file(),
        "{} is not built: cargo build --workspace --bins first",
        meta.display()
    );
    // its plugins too, whatever another meta on PATH brings
    let built = meta.parent().unwrap().to_path_buf();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::iter::once(built).chain(std::env::split_paths(&path));
    std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());
    std::env::set_var(meta_plugin::client::EXE_VAR, meta);
}

/// A server for a workspace whose `.meta.yaml` is `config`.
pub(crate) fn server(sandbox: &Sandbox, config: &str) -> (McpServer, PathBuf) {
    let root = sandbox.work().join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join(".meta.yaml"), config).unwrap();
    (McpServer::at(root.clone()).unwrap(), root)
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

/// A workspace whose root is a repo that ignores its children, with `web`
/// and the nested meta repo `platform` holding `core`, `api` (depends on
/// `core`) and `svc` (depends on `api`), the last three tagged `deep`.
pub(crate) fn nested(sandbox: &Sandbox) -> PathBuf {
    let root = sandbox.work().join("workspace");
    sandbox.repo(&root);
    for dir in [
        "web",
        "platform",
        "platform/core",
        "platform/api",
        "platform/svc",
    ] {
        sandbox.repo(&root.join(dir));
    }
    let write = |path: &str, text: &str| std::fs::write(root.join(path), text).unwrap();
    write(
        ".meta.yaml",
        r#"{"projects":{"web":"x","platform":{"repo":"x","meta":true}}}"#,
    );
    write(
        ".gitignore",
        "web
platform
.worktrees
",
    );
    sandbox.git(&root, &["add", ".meta.yaml", ".gitignore"]);
    sandbox.git(&root, &["commit", "-qm", "meta"]);
    write(
        "platform/.meta.yaml",
        r#"{"projects":{"core":{"repo":"x","tags":["deep"]},
            "api":{"repo":"x","tags":["deep"],"depends_on":["core"]},
            "svc":{"repo":"x","tags":["deep"],"depends_on":["api"]}}}"#,
    );
    write("platform/.gitignore", "core\napi\nsvc\n");
    sandbox.git(&root.join("platform"), &["add", ".meta.yaml", ".gitignore"]);
    sandbox.git(&root.join("platform"), &["commit", "-qm", "meta"]);
    root
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
