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

    // an unknown argument is refused without echoing control characters
    let out = meta_mcp(tmp.path(), &["--bad\x1b]0;x\x07\nFORGED"], "", &[]);
    assert!(!out.status.success());
    let shown = String::from_utf8_lossy(&out.stderr);
    assert!(
        !shown.contains('\x1b') && !shown.contains("\nFORGED"),
        "{shown}"
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
    write(
        ".gitignore",
        "/a/\n/bad/\n/broken/\n/elsewhere/\n/m/\n/pm/\n/plain/\n",
    );
    // a project named like another's path is still told apart by its own
    write(
        ".meta.yaml",
        "projects:\n  a: {}\n  bad: {meta: true}\n  broken: {}\n  suite: {path: m, meta: true}\n  \
         m/inner: {path: elsewhere}\n  pm: {meta: true, tags: [outer]}\n  plain: {}\n",
    );
    write("elsewhere/e.rs", "--flag\n");
    // a meta project whose config can't be read: its error, the others' matches
    write("bad/.meta.yaml", "{broken");
    // and one two levels down: the healthy projects beside it are searched
    write("pm/deep/.meta.yaml", "{broken");
    // a plain meta project, with a repository nested in it
    write(
        "pm/.meta.yaml",
        "projects:\n  c: {}\n  deep: {meta: true}\n",
    );
    repo("pm/c");
    write("pm/c/z.rs", "--flag\n");
    // a project whose .git can't be read: its error, and the others' matches
    write("broken/.git", "not a gitdir\n");
    repo("a");
    // settings that would change what git grep prints, or refuse it
    for (key, value) in [
        ("submodule.recurse", "true"),
        ("grep.column", "true"),
        ("color.grep", "always"),
        ("core.quotePath", "true"),
    ] {
        git(&ws.join("a"), &["config", key, value]);
    }
    write("a/\u{540d}.rs", "--flag\n");
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

    let found = call(
        ws,
        "meta_search_code",
        serde_json::json!({"pattern": "--flag"}),
    );
    let found = found.as_array().unwrap();
    let entry = |project: &str| found.iter().find(|entry| entry["project"] == project);
    let matches = |project: &str| entry(project).map(|entry| entry["matches"].clone());
    assert_eq!(
        matches("a"),
        Some(serde_json::json!([
            "notes.txt:1:--flag",
            "src/lib.rs:1:--flag",
            "\u{540d}.rs:1:--flag"
        ])),
        "{found:?}"
    );
    assert_eq!(
        matches("m/inner"),
        Some(serde_json::json!(["y.rs:1:--flag"]))
    );
    assert_eq!(matches("plain"), Some(serde_json::json!(["x.rs:1:--flag"])));
    assert_eq!(
        matches("elsewhere"),
        Some(serde_json::json!(["e.rs:1:--flag"]))
    );
    // a nested project's files once, as its own
    assert_eq!(matches("pm/c"), Some(serde_json::json!(["z.rs:1:--flag"])));
    assert!(entry("pm").is_none(), "{found:?}");
    for broken in ["bad", "pm/deep"] {
        assert!(
            entry(broken).is_some_and(|entry| entry["error"].as_str().is_some()),
            "{found:?}"
        );
    }
    // a config that can't be read is reported whatever the tags: the
    // projects it hides may have them
    let filtered = call(
        ws,
        "meta_search_code",
        serde_json::json!({"pattern": "--flag", "tag": "outer"}),
    );
    assert!(
        filtered
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["project"] == "pm/deep" && entry["error"].is_string()),
        "{filtered}"
    );
    // the broken project's error, in place of matches
    assert!(
        entry("broken").is_some_and(|entry| entry.get("matches").is_none()
            && entry["error"]
                .as_str()
                .is_some_and(|e| e.contains("broken"))),
        "{found:?}"
    );
    assert_eq!(found.len(), 8, "{found:?}");

    // a file git can't read: the matches it could, and what it couldn't
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        write("plain/secret.rs", "--flag\n");
        let secret = ws.join("plain/secret.rs");
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o000)).unwrap();
        // root reads it anyway
        if std::fs::read(&secret).is_err() {
            let found = call(
                ws,
                "meta_search_code",
                serde_json::json!({"pattern": "--flag"}),
            );
            let plain = found
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["project"] == "plain")
                .cloned();
            let plain = plain.unwrap_or_default();
            assert_eq!(
                plain["matches"],
                serde_json::json!(["x.rs:1:--flag"]),
                "{found}"
            );
            assert!(
                plain["error"]
                    .as_str()
                    .is_some_and(|e| e.contains("secret.rs")),
                "{found}"
            );
        }
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let found = call(
        ws,
        "meta_search_code",
        serde_json::json!({"pattern": "--flag", "file_pattern": "*.rs"}),
    );
    let a = found
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["project"] == "a");
    assert_eq!(
        a.map(|a| a["matches"].clone()),
        Some(serde_json::json!([
            "src/lib.rs:1:--flag",
            "\u{540d}.rs:1:--flag"
        ])),
        "{found}"
    );
    // a search git refuses is reported, not taken for no match
    let found = call(ws, "meta_search_code", serde_json::json!({"pattern": "("}));
    assert_eq!(found.as_array().unwrap().len(), 10, "{found}");
    assert!(
        found[0]["error"].as_str().is_some_and(|e| !e.is_empty()),
        "{found}"
    );
}

/// A plain project's search keeps out of the workspace repo by a ceiling,
/// which a path with the list separator would split: an error, not a
/// silent "no match".
#[test]
#[cfg(unix)]
fn search_reports_a_plain_project_it_cannot_fence_off() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("a:b");
    std::fs::create_dir_all(ws.join("plain")).unwrap();
    std::fs::write(ws.join("plain/x.rs"), "--flag\n").unwrap();
    std::fs::write(ws.join(".meta.yaml"), "projects:\n  plain: {}\n").unwrap();
    let found = call(
        &ws,
        "meta_search_code",
        serde_json::json!({"pattern": "--flag"}),
    );
    assert!(
        found[0]["error"]
            .as_str()
            .is_some_and(|e| e.contains("path list separator")),
        "{found}"
    );
}

/// A project nested through a link is searched once, as its own, not also
/// where the link leads inside its parent.
#[test]
#[cfg(unix)]
fn search_leaves_a_linked_nested_project_to_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    std::fs::create_dir_all(ws.join("pm/real")).unwrap();
    std::fs::write(ws.join("pm/real/z.rs"), "--flag\n").unwrap();
    std::os::unix::fs::symlink("real", ws.join("pm/alias")).unwrap();
    std::fs::write(ws.join("pm/.meta.yaml"), "projects:\n  c: {path: alias}\n").unwrap();
    std::fs::write(
        ws.join(".meta.yaml"),
        "projects:\n  pm: {meta: true, tags: [outer]}\n",
    )
    .unwrap();
    let found = call(
        ws,
        "meta_search_code",
        serde_json::json!({"pattern": "--flag"}),
    );
    assert_eq!(
        found,
        serde_json::json!([{"project": "pm/alias", "matches": ["z.rs:1:--flag"]}])
    );
    let found = call(
        ws,
        "meta_search_code",
        serde_json::json!({"pattern": "--flag", "tag": "outer"}),
    );
    assert_eq!(found, serde_json::json!([]));
}
