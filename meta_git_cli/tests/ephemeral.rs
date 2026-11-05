//! `meta git worktree exec --ephemeral` removes the set it made however the
//! run ends: stopped by Ctrl-C, nothing it started is left, and neither is
//! the set.

// Unix only: a Ctrl-C to meta's process group
#![cfg(unix)]

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

/// The processes whose working directory is under `dir` (Linux: where
/// /proc tells).
fn running_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            std::fs::read_link(entry.path().join("cwd")).is_ok_and(|cwd| cwd.starts_with(dir))
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn a_cancelled_ephemeral_run_leaves_no_set_and_no_process() {
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
        .args(["--", "sleep", "30"])
        .current_dir(&ws)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    let set = ws.join(".worktrees/t");
    wait_for("the set's command", || {
        !running_in(&set.join("a")).is_empty()
    });

    // Ctrl-C: SIGINT to meta's process group, as a terminal sends it
    let group = rustix::process::Pid::from_child(&run);
    rustix::process::kill_process_group(group, rustix::process::Signal::INT).unwrap();
    run.wait().unwrap();

    wait_for("the set's removal", || !set.exists());
    wait_for("the end of what the run started", || {
        running_in(&sandbox.work()).is_empty()
    });
    assert!(
        ws.join("a/README").exists(),
        "the workspace's own checkout is kept"
    );
}
