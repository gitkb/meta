//! `meta exec` acts only in project folders that are repositories of their
//! own, and names the others.

use meta_test_support::Sandbox;
use std::fs;

#[test]
fn exec_runs_only_in_projects_that_are_their_own_repository() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    fs::write(
        root.join(".meta.yaml"),
        "projects:\n  cloned: u1\n  empty: u2\n  plain: u3\n  missing: u4\n",
    )
    .unwrap();
    sandbox.git(&root, &["init", "-q"]);
    for project in ["cloned", "empty", "plain"] {
        fs::create_dir(root.join(project)).unwrap();
    }
    sandbox.git(&root.join("cloned"), &["init", "-q"]);
    fs::write(root.join("plain/notes.txt"), "mine").unwrap();

    let meta = |args: &[&str]| {
        sandbox
            .command(env!("CARGO_BIN_EXE_meta"))
            .args(args)
            .current_dir(&root)
            .output()
            .unwrap()
    };
    let out = meta(&["exec", "--", "git", "rev-parse", "--show-toplevel"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{stdout}");
    // the root and the clone answer for themselves; nothing else ran, so
    // the workspace's toplevel is printed once, for the root (compared as
    // folders: git prints `C:/...` where Rust has `\\?\C:\...`)
    let printed: Vec<_> = stdout
        .lines()
        .filter_map(|line| std::path::Path::new(line).canonicalize().ok())
        .collect();
    assert!(printed.contains(&root.join("cloned")), "{stdout}");
    assert_eq!(
        printed.iter().filter(|p| **p == root).count(),
        1,
        "{stdout}"
    );
    let why = [
        ("empty", "not a git repository (folder exists, no .git)"),
        ("plain", "not a git repository (folder exists, no .git)"),
        ("missing", "not cloned (run 'meta git update')"),
    ];
    // on stderr, as a command's own errors are
    let stderr = String::from_utf8_lossy(&out.stderr);
    for (_, reason) in why {
        assert!(stderr.contains(reason), "{stderr}");
    }

    // and each is a failed result in the JSON, never left out of it
    let out = meta(&["--json", "exec", "--", "git", "status"]);
    assert!(!out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["success"], false, "{json}");
    assert_eq!(json["summary"]["failed"], 3, "{json}");
    for (project, reason) in why {
        let result = json["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["directory"].as_str().unwrap().ends_with(project))
            .unwrap();
        assert_eq!(result["success"], false, "{result}");
        assert!(
            result["stderr"].as_str().unwrap().contains(reason),
            "{result}"
        );
    }
}

#[cfg(unix)]
#[test]
fn git_a_command_starts_never_reaches_the_repository_around_the_workspace() {
    // a workspace that is no repository, in one that is (a dotfiles repo in
    // HOME, say), with a project outside it
    let sandbox = Sandbox::new();
    let outer = sandbox.work();
    sandbox.git(&outer, &["init", "-q"]);
    let ws = outer.join("ws");
    fs::create_dir_all(ws.join("api")).unwrap();
    sandbox.git(&ws.join("api"), &["init", "-q"]);
    fs::create_dir(outer.join("sib")).unwrap();
    sandbox.git(&outer.join("sib"), &["init", "-q"]);
    fs::write(
        ws.join(".meta.yaml"),
        "projects:\n  api: u1\n  sib:\n    repo: u2\n    path: ../sib\n",
    )
    .unwrap();

    let out = sandbox
        .command(env!("CARGO_BIN_EXE_meta"))
        .current_dir(&ws)
        .args(["exec", "--", "sh", "-c", "git rev-parse --show-toplevel"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // the root fails on its own; the projects answer for themselves
    assert!(!out.status.success(), "{stdout}");
    let printed: Vec<_> = stdout
        .lines()
        .filter_map(|line| std::path::Path::new(line).canonicalize().ok())
        .collect();
    assert!(!printed.contains(&outer), "{stdout}");
    assert!(
        printed.contains(&ws.join("api")) && printed.contains(&outer.join("sib")),
        "{stdout}"
    );
}

#[test]
fn a_silent_run_prints_nothing_for_a_refused_project() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    sandbox.git(&root, &["init", "-q"]);
    fs::create_dir(root.join("plain")).unwrap();
    fs::write(root.join(".meta.yaml"), "projects:\n  plain: u\n").unwrap();
    let out = sandbox
        .command(env!("CARGO_BIN_EXE_meta"))
        .current_dir(&root)
        .args([
            "--silent",
            "--sequential",
            "-i",
            "plain",
            "exec",
            "--",
            "git",
            "status",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A malformed selected worktree must fail before ordinary config fallback or exec.
#[cfg(unix)]
#[test]
fn malformed_worktree_membership_is_reported_before_dispatch() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    let worktree = root.join(".worktrees/broken");
    fs::create_dir_all(&worktree).unwrap();
    fs::write(root.join(".meta.yaml"), r#"{"projects":{}}"#).unwrap();
    fs::write(worktree.join(".meta.yaml"), r#"{"projects":{}}"#).unwrap();
    fs::write(worktree.join(".git"), "malformed metadata").unwrap();
    let marker = root.join("unexpected-dispatch");
    let output = sandbox
        .command(env!("CARGO_BIN_EXE_meta"))
        .args(["exec", "--", "/usr/bin/touch", marker.to_str().unwrap()])
        .current_dir(&worktree)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Invalid .git file format"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !marker.exists(),
        "invalid worktree dispatched through ordinary config fallback"
    );
}
