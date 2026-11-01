//! `meta exec` acts only in project folders that are repositories of their
//! own, and names the others.

use meta_core::git::run;
use std::fs;
use tempfile::tempdir;

#[test]
fn exec_runs_only_in_projects_that_are_their_own_repository() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    fs::write(
        root.join(".meta.yaml"),
        "projects:\n  cloned: u1\n  empty: u2\n  plain: u3\n  missing: u4\n",
    )
    .unwrap();
    run(&root, ["init", "-q"]).unwrap();
    for project in ["cloned", "empty", "plain"] {
        fs::create_dir(root.join(project)).unwrap();
    }
    run(&root.join("cloned"), ["init", "-q"]).unwrap();
    fs::write(root.join("plain/notes.txt"), "mine").unwrap();

    let meta = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_meta"))
            .args(args)
            .current_dir(&root)
            .env("HOME", &root)
            .env("XDG_CONFIG_HOME", root.join("xdg"))
            .env("XDG_DATA_HOME", root.join("xdg-data"))
            .env("GIT_CONFIG_GLOBAL", root.join("gitconfig"))
            .env("META_DATA_DIR", root.join("data"))
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
    for (_, reason) in why {
        assert!(stdout.contains(reason), "{stdout}");
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

/// `meta args` in `dir`, with no user configuration.
fn meta_in(dir: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_meta"))
        .args(args)
        .current_dir(dir)
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("xdg"))
        .env("XDG_DATA_HOME", dir.join("xdg-data"))
        .env("GIT_CONFIG_GLOBAL", dir.join("gitconfig"))
        .env("META_DATA_DIR", dir.join("data"))
        .output()
        .unwrap()
}

#[cfg(unix)]
#[test]
fn git_a_command_starts_never_reaches_the_repository_around_the_workspace() {
    // a workspace that is no repository, in one that is (a dotfiles repo in
    // HOME, say), with a project outside it
    let tmp = tempdir().unwrap();
    let outer = tmp.path().canonicalize().unwrap();
    run(&outer, ["init", "-q"]).unwrap();
    let ws = outer.join("ws");
    fs::create_dir_all(ws.join("api")).unwrap();
    run(&ws.join("api"), ["init", "-q"]).unwrap();
    fs::create_dir(outer.join("sib")).unwrap();
    run(&outer.join("sib"), ["init", "-q"]).unwrap();
    fs::write(
        ws.join(".meta.yaml"),
        "projects:\n  api: u1\n  sib:\n    repo: u2\n    path: ../sib\n",
    )
    .unwrap();

    let out = meta_in(
        &ws,
        &["exec", "--", "sh", "-c", "git rev-parse --show-toplevel"],
    );
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
    let tmp = tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    run(&root, ["init", "-q"]).unwrap();
    fs::create_dir(root.join("plain")).unwrap();
    fs::write(root.join(".meta.yaml"), "projects:\n  plain: u\n").unwrap();
    let out = meta_in(
        &root,
        &[
            "--silent",
            "--sequential",
            "-i",
            "plain",
            "exec",
            "--",
            "git",
            "status",
        ],
    );
    assert!(!out.status.success());
    assert!(
        out.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}
