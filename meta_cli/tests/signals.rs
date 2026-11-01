//! A signal that stops meta stops what meta started: the commands `meta exec`
//! runs and the plugin a command goes to, with everything they started in
//! turn, and meta ends as that signal ends a program.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Started, then two seconds later (unless stopped) appends to `late`.
const SLOW: &str =
    r#"(sleep 2; echo "$PWD" >> "$ROOT/late") & echo "$PWD" >> "$ROOT/started"; wait"#;

/// A workspace with repos `a` and `b`, and a plugin `meta-slow` on PATH
/// whose command runs [`SLOW`] once.
fn workspace(root: &Path) {
    fs::write(
        root.join(".meta.yaml"),
        "projects:\n  a:\n    path: a\n  b:\n    path: b\n",
    )
    .unwrap();
    for repo in [root.to_path_buf(), root.join("a"), root.join("b")] {
        fs::create_dir_all(&repo).unwrap();
        let init = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", root.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap();
        assert!(init.success());
    }
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let plugin = bin.join("meta-slow");
    fs::write(
        &plugin,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n  --meta-plugin-info) echo '{{\"name\":\"slow\",\"version\":\"1.0.0\",\"commands\":[\"slow\"]}}' ;;\n  --meta-plugin-exec) cat >/dev/null; {SLOW} ;;\nesac\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&plugin, fs::Permissions::from_mode(0o755)).unwrap();
}

/// meta, started as a job of its own (never a terminal's foreground job).
fn start_meta(root: &Path, args: &[&str]) -> Child {
    start(root, env!("CARGO_BIN_EXE_meta"), args)
}

/// `program`, started as [`start_meta`] starts meta.
fn start(root: &Path, program: &str, args: &[&str]) -> Child {
    Command::new(program)
        .args(args)
        .current_dir(root)
        .env("ROOT", root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("xdg"))
        .env("XDG_DATA_HOME", root.join("xdg/data"))
        .env("GIT_CONFIG_GLOBAL", root.join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("META_DATA_DIR", root.join("data"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap()
}

/// Wait until `count` lines say a command started.
fn started(root: &Path, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while fs::read_to_string(root.join("started"))
        .unwrap_or_default()
        .lines()
        .count()
        < count
    {
        assert!(Instant::now() < deadline, "the commands didn't start");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// TERM meta, then check it ended by that signal and nothing it started
/// writes afterwards.
fn terminate(root: &Path, mut meta: Child) {
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&meta),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    let status = meta.wait().unwrap();
    assert_eq!(status.signal(), Some(15), "{status:?}");
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(
        fs::read_to_string(root.join("late")).unwrap_or_default(),
        "",
        "commands meta started went on after it was stopped"
    );
}

#[test]
fn terminating_meta_exec_stops_its_commands_and_theirs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    workspace(&root);
    let meta = start_meta(&root, &["exec", "--parallel", "--", "sh", "-c", SLOW]);
    started(&root, 3);
    terminate(&root, meta);
}

#[test]
fn terminating_meta_stops_the_plugin_and_what_it_started() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    workspace(&root);
    let meta = start_meta(&root, &["slow", "now"]);
    started(&root, 1);
    terminate(&root, meta);
}

/// A signal meta was started ignoring (as under `nohup`) it goes on
/// ignoring, and so do its commands. (Only Linux says, without unsafe code,
/// which signals a program started out ignoring.)
#[cfg(target_os = "linux")]
#[test]
fn a_hangup_meta_was_started_ignoring_stops_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    workspace(&root);
    let quick = r#"echo "$PWD" >> "$ROOT/started"; sleep 1; echo "$PWD" >> "$ROOT/late""#;
    let mut meta = start(
        &root,
        "sh",
        &[
            "-c",
            r#"trap '' HUP; exec "$0" "$@""#,
            env!("CARGO_BIN_EXE_meta"),
            "exec",
            "--",
            "sh",
            "-c",
            quick,
        ],
    );
    started(&root, 1);
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&meta),
        rustix::process::Signal::HUP,
    )
    .unwrap();
    let status = meta.wait().unwrap();
    assert!(status.success(), "{status:?}");
    let late = fs::read_to_string(root.join("late")).unwrap();
    assert_eq!(late.lines().count(), 3, "{late}");
}
