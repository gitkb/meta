//! The loop binary: which directories it runs in.

use std::process::Command;

#[test]
#[cfg(unix)]
fn an_exclusion_holds_for_directories_named_outright() {
    let tmp = tempfile::tempdir().unwrap();
    for dir in ["a", "b"] {
        std::fs::create_dir(tmp.path().join(dir)).unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_loop"))
        .args(["-i", "a", "-i", "b", "-e", "a", "--", "pwd"])
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let shown = String::from_utf8_lossy(&out.stdout);
    // pwd's lines, among loop's own
    let root = tmp.path().canonicalize().unwrap();
    let ran: Vec<_> = shown
        .lines()
        .filter_map(|line| std::path::Path::new(line.trim()).strip_prefix(&root).ok())
        .collect();
    assert_eq!(ran, [std::path::Path::new("b")], "{shown}");

    // nothing left is said, not run silently
    let out = Command::new(env!("CARGO_BIN_EXE_loop"))
        .args(["-i", "a", "-e", "a", "--", "pwd"])
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("No directories left after exclusions"));
}
