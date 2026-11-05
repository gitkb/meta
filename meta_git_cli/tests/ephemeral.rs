//! `meta git worktree exec --ephemeral` removes the set it made however the
//! run ends: stopped by Ctrl-C, nothing it started is left, and neither is
//! the set.

// Unix only: a Ctrl-C to meta's process group
#![cfg(unix)]
#![expect(
    clippy::unwrap_used,
    clippy::disallowed_methods,
    reason = "test helpers: a panic is a failed test, and the test lists processes with pgrep"
)]

use meta_plugin::testing::Sandbox;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Wait up to 20 seconds for `done`.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_cancelled_ephemeral_run_leaves_no_set_and_no_process() {
    // Ctrl-C: SIGINT to meta's process group, as a terminal sends it
    ended_by(|meta| rustix::process::kill_process_group(meta, rustix::process::Signal::INT));
    // meta killed outright, alone: what it started ends with it
    ended_by(|meta| rustix::process::kill_process(meta, rustix::process::Signal::KILL));
    // meta-git itself killed outright: its run is stopped before the set
    // is removed under it
    ended_by(|meta| {
        let children = std::process::Command::new("pgrep")
            .args(["-P", &meta.as_raw_nonzero().to_string()])
            .output()
            .unwrap();
        for pid in String::from_utf8_lossy(&children.stdout).split_whitespace() {
            let pid = rustix::process::Pid::from_raw(pid.parse().unwrap()).unwrap();
            rustix::process::kill_process(pid, rustix::process::Signal::KILL)?;
        }
        Ok(())
    });
}

/// An ephemeral run of a command that would go on for ten minutes, ended
/// by `stop` (given meta's pid, which leads its own process group).
fn ended_by(stop: impl Fn(rustix::process::Pid) -> rustix::io::Result<()>) {
    let sandbox = Sandbox::new();
    let ws = sandbox
        .workspace(&sandbox.work().join("ws"))
        .repo("a")
        .build();
    let ws = ws.canonicalize().unwrap();
    // meta and meta-git as built (`cargo build --workspace --bins` first)
    let bin = Path::new(env!("CARGO_BIN_EXE_meta-git")).parent().unwrap();
    let meta = bin.join("meta");
    assert!(meta.is_file(), "{} is not built", meta.display());
    let path = std::env::var_os("PATH").unwrap_or_default();
    let paths = std::iter::once(bin.to_path_buf()).chain(std::env::split_paths(&path));
    let mut run = sandbox
        .command(&meta)
        .args(["git", "worktree", "exec", "--ephemeral", "t", "--repo", "a"])
        // the command says it started, and which process it is
        .args([
            "--",
            "sh",
            "-c",
            r#"echo $$ > "$0.$(basename "$PWD")"; exec sleep 600"#,
        ])
        .arg(sandbox.work().join("started"))
        .current_dir(&ws)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    let set = ws.join(".worktrees/t");
    let started = sandbox.work().join("started.a");
    wait_for("the set's command", || {
        std::fs::read_to_string(&started).is_ok_and(|pid| pid.ends_with('\n'))
    });
    let pid: i32 = std::fs::read_to_string(&started)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let pid = rustix::process::Pid::from_raw(pid).unwrap();

    stop(rustix::process::Pid::from_child(&run)).unwrap();
    run.wait().unwrap();

    wait_for("the set's removal", || !set.exists());
    wait_for("the end of what the run started", || {
        rustix::process::test_kill_process(pid).is_err()
    });
    assert!(
        ws.join("a/README").exists(),
        "the workspace's own checkout is kept"
    );
}
