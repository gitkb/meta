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

/// Waits until `done` holds, failing the test after 20 seconds.
#[cfg(unix)]
fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "waited too long for {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
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

/// A plain project that is a link into a repository elsewhere in the
/// workspace is searched by its real place: that repository's ignore rules
/// don't hide it.
#[test]
#[cfg(unix)]
fn search_fences_off_a_linked_plain_project_by_its_real_path() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let nested = ws.join("nested");
    std::fs::create_dir_all(nested.join("plain")).unwrap();
    git(&nested, &["init", "-q"]);
    std::fs::write(nested.join(".gitignore"), "plain/\n").unwrap();
    std::fs::write(nested.join("plain/x.rs"), "--flag\n").unwrap();
    std::os::unix::fs::symlink("nested/plain", ws.join("alias")).unwrap();
    std::fs::write(ws.join(".meta.yaml"), "projects:\n  alias: {}\n").unwrap();
    let found = call(
        ws,
        "meta_search_code",
        serde_json::json!({"pattern": "--flag"}),
    );
    assert_eq!(
        found,
        serde_json::json!([{"project": "alias", "matches": ["x.rs:1:--flag"]}])
    );
}

/// Windows' path list separator is `;`, which a directory name may hold.
#[test]
#[cfg(windows)]
fn search_reports_a_plain_project_it_cannot_fence_off() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("a;b");
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

/// While a tool runs, `ping` and `tools/list` are answered; cancelling it
/// stops what it started, and it gets no reply.
#[test]
#[cfg(unix)]
fn a_running_call_can_be_cancelled_and_others_are_answered_meanwhile() {
    use std::io::{BufRead, BufReader};
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    std::fs::create_dir(ws.join("a")).unwrap();
    std::fs::write(ws.join(".meta.yaml"), "projects:\n  a: {}\n").unwrap();
    let mut child = isolated(Command::new(env!("CARGO_BIN_EXE_meta-mcp")), ws)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut replies = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut send = |message: serde_json::Value| writeln!(stdin, "{message}").unwrap();
    let script = "sleep 60 & echo $$ $! > ../pids.tmp && mv ../pids.tmp ../pids; wait";
    send(
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
        "name": "meta_batch_execute", "arguments": {"command": ["sh", "-c", script]}}}),
    );
    let pids = ws.join("pids");
    wait_for("the script to start", || pids.exists());
    let started = std::time::Instant::now();
    send(serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}));
    send(serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "tools/list"}));
    for id in [3, 4] {
        let reply: serde_json::Value =
            serde_json::from_str(&replies.next().unwrap().unwrap()).unwrap();
        assert_eq!(reply["id"], id, "{reply}");
    }
    send(
        serde_json::json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
        "params": {"requestId": 2, "reason": "test"}}),
    );
    drop(send);
    drop(stdin);
    assert!(replies.next().is_none(), "the cancelled call was answered");
    assert!(child.wait().unwrap().success());
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    for pid in std::fs::read_to_string(pids).unwrap().split_whitespace() {
        wait_for(&format!("{pid} to end"), || gone(pid));
    }
}

/// A spawn error reaches the client with its cause (agent-17: `E2BIG`).
#[test]
#[cfg(unix)]
fn a_command_too_long_to_start_says_why() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(".meta.yaml"), "projects: {}\n").unwrap();
    // a `meta` to find, so starting it fails for its arguments' size alone
    let bin = tmp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    std::fs::write(bin.join("meta"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(bin.join("meta"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let request = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "meta_exec",
            "arguments": {"command": ["echo", "y".repeat(3_000_000)]}}});
    let out = meta_mcp(tmp.path(), &[], &request.to_string(), &[("PATH", &bin)]);
    let reply: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reply["result"]["isError"], true, "{reply}");
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("Argument list too long"),
        "{}",
        &text[..text.len().min(300)]
    );
}

/// Whether the process `pid` is gone.
#[cfg(unix)]
fn gone(pid: &str) -> bool {
    !Command::new("kill")
        .args(["-0", pid])
        .status()
        .unwrap()
        .success()
}

/// A repo at `dir` with one commit, `before`, and an identity of its own.
#[cfg(unix)]
fn repo_with_a_commit(dir: &Path) {
    std::fs::create_dir(dir).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.name", "t"],
        &["config", "user.email", "t@example.com"],
        &["commit", "-q", "--allow-empty", "-m", "before"],
    ] {
        git(dir, args);
    }
}

/// The subject of the last commit in `dir`.
#[cfg(unix)]
fn last_commit(dir: &Path) -> String {
    let mut log = isolated(meta_core::git::git(dir), dir);
    let out = log.args(["log", "-1", "--format=%s"]).output().unwrap();
    String::from_utf8(out.stdout).unwrap()
}

/// meta-mcp in `ws`, leading a process group of its own, with `calls` sent;
/// stdout dropped, stderr kept. Its stdin comes back too: while it is open
/// the server isn't told to end, only signalled.
#[cfg(unix)]
fn server_in_its_group(
    ws: &Path,
    calls: &[serde_json::Value],
) -> (std::process::Child, std::process::ChildStdin) {
    use std::os::unix::process::CommandExt;
    let mut server = isolated(Command::new(env!("CARGO_BIN_EXE_meta-mcp")), ws);
    server
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut server = server.spawn().unwrap();
    let mut stdin = server.stdin.take().unwrap();
    for call in calls {
        writeln!(stdin, "{call}").unwrap();
    }
    (server, stdin)
}

/// SIGTERM to the server's whole process group; its stderr once it has
/// exited, as a signal ends it (128 + 15).
#[cfg(unix)]
fn stop_group(mut server: std::process::Child) -> String {
    let group = format!("-{}", server.id());
    let kill = Command::new("kill").args(["-TERM", "--", &group]).status();
    assert!(kill.unwrap().success());
    let mut status = None;
    wait_for("the server to exit", || {
        status = server.try_wait().unwrap();
        status.is_some()
    });
    assert_eq!(status.unwrap().code(), Some(128 + 15), "{status:?}");
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut server.stderr.take().unwrap(), &mut stderr).unwrap();
    stderr
}

/// An atomic or plain `meta_batch_execute` of `script` in `sh`.
#[cfg(unix)]
fn batch(id: u32, script: &str, atomic: bool) -> serde_json::Value {
    serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": {"name": "meta_batch_execute",
            "arguments": {"command": ["sh", "-c", script], "atomic": atomic}}})
}

/// A signal that ends the server, sent to its process group, also stops the
/// running tool's programs, which run in a group of their own; an atomic
/// batch it stopped is rolled back and on record before the server ends, and
/// a call waiting behind it never starts.
#[test]
#[cfg(unix)]
fn stopping_the_server_stops_its_running_tool() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let a = ws.join("a");
    repo_with_a_commit(&a);
    std::fs::write(ws.join(".meta.yaml"), "projects:\n  a: {}\n").unwrap();
    let script = |file: &str| {
        format!(
            "git commit -q --allow-empty -m during; \
             sleep 60 & echo $! > ../{file}.tmp && mv ../{file}.tmp ../{file}; wait"
        )
    };
    let calls = [
        batch(1, &script("pid"), true),
        batch(2, &script("waiting"), false),
    ];
    let (server, _stdin) = server_in_its_group(ws, &calls);
    let pid = ws.join("pid");
    wait_for("the tool to start", || pid.exists());
    assert_eq!(last_commit(&a), "during\n");
    let stderr = stop_group(server);
    let sleep = std::fs::read_to_string(&pid).unwrap();
    wait_for("the tool's sleep to end", || gone(sleep.trim()));
    assert!(!ws.join("waiting").exists(), "the waiting call started");
    assert_eq!(last_commit(&a), "before\n");
    assert!(
        stderr.contains("request 1 was cancelled") && stderr.contains("rollback"),
        "{stderr}"
    );
    assert!(stderr.contains("request 2 not started"), "{stderr}");
}

/// A signal that comes while an atomic batch is rolling back lets the
/// rollback finish, however long it takes, before the server ends.
#[test]
#[cfg(unix)]
fn stopping_the_server_lets_a_rollback_finish() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let a = ws.join("a");
    repo_with_a_commit(&a);
    std::fs::write(ws.join(".meta.yaml"), "projects:\n  a: {}\n").unwrap();
    // the rollback's git asks this hook, which holds it longer than a
    // cancelled program is given (2 s grace, 4 s at most)
    let hook = ws.join("hook");
    let script = "#!/bin/sh\n[ -e ../in-hook ] || { touch ../in-hook; sleep 6; }\n";
    std::fs::write(&hook, script).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let command = "git commit -q --allow-empty -m during && \
                   git config core.fsmonitor ../hook && exit 1";
    let (server, _stdin) = server_in_its_group(ws, &[batch(1, command, true)]);
    wait_for("the rollback to reach the hook", || {
        ws.join("in-hook").exists()
    });
    let stderr = stop_group(server);
    assert_eq!(last_commit(&a), "before\n");
    assert!(stderr.contains(r#"\"status\": \"success\""#), "{stderr}");
}

/// When its output is gone, the server ends, though its input stays open,
/// and the reply it owed goes to stderr.
#[test]
#[cfg(unix)]
fn a_server_whose_output_is_gone_ends() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(".meta.yaml"), "projects: {}\n").unwrap();
    let mut server = isolated(Command::new(env!("CARGO_BIN_EXE_meta-mcp")), tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(server.stdout.take());
    let mut stdin = server.stdin.take().unwrap();
    let call = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "meta_get_config"}});
    // again until it ends: a child another test forks may hold the read
    // end for a moment, and a reply written then goes through
    let mut status = None;
    wait_for("the server to end", || {
        let _ = writeln!(stdin, "{call}");
        status = server.try_wait().unwrap();
        status.is_some()
    });
    assert_eq!(status.unwrap().code(), Some(141), "{status:?}");
    drop(stdin);
    // the reply it couldn't send is on record
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut server.stderr.take().unwrap(), &mut stderr).unwrap();
    assert!(
        stderr.contains("the reply was:") && stderr.contains(r#""id":1"#),
        "{stderr}"
    );
}

/// When the output is found gone while answering a ping, the running tool is
/// stopped too, so the server ends though the tool never would.
#[test]
#[cfg(unix)]
fn a_server_whose_output_is_gone_stops_its_running_tool() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    std::fs::create_dir(ws.join("a")).unwrap();
    std::fs::write(ws.join(".meta.yaml"), "projects:\n  a: {}\n").unwrap();
    let mut server = isolated(Command::new(env!("CARGO_BIN_EXE_meta-mcp")), ws)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    drop(server.stdout.take());
    let mut stdin = server.stdin.take().unwrap();
    let script = "sleep 60 & echo $! > ../pid.tmp && mv ../pid.tmp ../pid; wait";
    writeln!(stdin, "{}", batch(1, script, false)).unwrap();
    let pid = ws.join("pid");
    wait_for("the tool to start", || pid.exists());
    // pings until it ends (see `a_server_whose_output_is_gone_ends`)
    let mut status = None;
    wait_for("the server to end", || {
        let _ = writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"ping"}}"#);
        status = server.try_wait().unwrap();
        status.is_some()
    });
    assert_eq!(status.unwrap().code(), Some(141), "{status:?}");
    let sleep = std::fs::read_to_string(&pid).unwrap();
    wait_for("the tool's sleep to end", || gone(sleep.trim()));
    drop(stdin);
}

/// A server that finds its output gone while sending a tool's reply drops
/// the calls still waiting, each said on stderr.
#[test]
#[cfg(unix)]
fn a_server_whose_output_is_gone_drops_its_waiting_calls() {
    let tmp = tempfile::tempdir().unwrap();
    // a project for the first call to sleep in
    std::fs::create_dir(tmp.path().join("a")).unwrap();
    std::fs::write(tmp.path().join(".meta.yaml"), "projects:\n  a: {}\n").unwrap();
    let mut server = isolated(Command::new(env!("CARGO_BIN_EXE_meta-mcp")), tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(server.stdout.take());
    let mut stdin = server.stdin.take().unwrap();
    // the other calls wait while the first one sleeps; should a reply go
    // through (see `a_server_whose_output_is_gone_ends`), the next fails
    writeln!(stdin, "{}", batch(1, "sleep 1", false)).unwrap();
    for id in 2..5 {
        let call = serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": "meta_get_config"}});
        writeln!(stdin, "{call}").unwrap();
    }
    let mut status = None;
    wait_for("the server to end", || {
        status = server.try_wait().unwrap();
        status.is_some()
    });
    assert_eq!(status.unwrap().code(), Some(141), "{status:?}");
    drop(stdin);
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut server.stderr.take().unwrap(), &mut stderr).unwrap();
    assert!(stderr.contains("not started"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn a_project_outside_the_workspace_is_selected_by_its_name() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    std::fs::create_dir(&ws).unwrap();
    let outside = tmp.path().join("sib");
    std::fs::write(
        ws.join(".meta.yaml"),
        format!(
            "projects:\n  sib:\n    repo: u\n    path: {}\n",
            outside.display()
        ),
    )
    .unwrap();
    // a `meta` that writes down what it was asked to select
    let bin = tmp.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let log = tmp.path().join("args");
    std::fs::write(
        bin.join("meta"),
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\necho '{{}}'\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(bin.join("meta"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let request = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "meta_git_status", "arguments": {"project": "sib"}}});
    meta_mcp(&ws, &[], &request.to_string(), &[("PATH", &bin)]);
    let args = std::fs::read_to_string(&log).unwrap();
    let args: Vec<&str> = args.lines().collect();
    let include = args.iter().position(|a| *a == "--include").unwrap();
    assert_eq!(args[include + 1], "sib", "{args:?}");
}
