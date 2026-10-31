//! `defaults` come from the config meta runs with, wherever it was found.

/// `defaults: {parallel: false}` holds when meta runs from a project's
/// directory, below the config: two repos holding one lock at once would
/// fail.
#[cfg(unix)]
#[test]
fn sequential_default_holds_below_the_config() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp = tmp.path().canonicalize().unwrap();
    let workspace = tmp.join("workspace");
    for project in ["a", "b"] {
        std::fs::create_dir_all(workspace.join(project)).unwrap();
    }
    std::fs::write(
        workspace.join(".meta.yaml"),
        "defaults:\n  parallel: false\nprojects:\n  a:\n    path: a\n  b:\n    path: b\n",
    )
    .unwrap();
    let lock = tmp.join("lock");
    let take_lock = format!(
        "mkdir {0} && sleep 0.5 && rmdir {0}",
        lock.to_string_lossy()
    );
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_meta"))
        .args(["exec", "--", "sh", "-c", &take_lock])
        .current_dir(workspace.join("a"))
        .env("HOME", &tmp)
        .env("XDG_CONFIG_HOME", tmp.join("xdg"))
        .env("GIT_CONFIG_GLOBAL", tmp.join("no-gitconfig"))
        .env("META_DATA_DIR", tmp.join("meta-data"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
