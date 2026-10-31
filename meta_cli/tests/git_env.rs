//! meta run from a git hook acts on its workspace's repos, not the hook's.

use std::path::Path;
use std::process::{Command, Output};

/// A repository at `dir` with one commit, on `branch`.
fn repo(dir: &Path, branch: &str) {
    std::fs::create_dir_all(dir).unwrap();
    for args in [
        &["init", "-q", "-b", branch][..],
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "first",
        ],
    ] {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", dir.join("no-gitconfig"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn a_hooks_repository_variables_do_not_reach_metas_git() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp = tmp.path().canonicalize().unwrap();
    let workspace = tmp.join("workspace");
    repo(&workspace, "on-workspace");
    repo(&workspace.join("a"), "on-a");
    std::fs::write(
        workspace.join(".meta.yaml"),
        "projects:\n  a:\n    path: a\n",
    )
    .unwrap();
    // The repository whose hook runs meta, as git sets it up for one
    let hook = tmp.join("hook");
    repo(&hook, "on-hook");
    let meta = |args: &[&str]| -> Output {
        let out = Command::new(env!("CARGO_BIN_EXE_meta"))
            .args(args)
            .current_dir(&workspace)
            .env("GIT_DIR", hook.join(".git"))
            .env("GIT_WORK_TREE", &hook)
            .env("GIT_INDEX_FILE", hook.join(".git/index"))
            // the user's own settings, which git passes on to submodules too
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "meta.test")
            .env("GIT_CONFIG_VALUE_0", "kept")
            .env("HOME", &tmp)
            .env("XDG_CONFIG_HOME", tmp.join("xdg"))
            .env("GIT_CONFIG_GLOBAL", tmp.join("no-gitconfig"))
            .env("META_DATA_DIR", tmp.join("meta-data"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    };

    // A query meta makes itself
    let context: serde_json::Value =
        serde_json::from_slice(&meta(&["context", "--json", "--no-cache"]).stdout).unwrap();
    assert_eq!(context["repos"][0]["branch"], "on-a", "{context}");

    // A command meta runs in each repo, with the user's settings
    let out = meta(&["exec", "--", "git", "branch", "--show-current"]);
    let shown = String::from_utf8_lossy(&out.stdout);
    assert!(
        shown.contains("on-a") && shown.contains("on-workspace") && !shown.contains("on-hook"),
        "{shown}"
    );
    let out = meta(&["exec", "--", "git", "config", "meta.test"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).matches("kept").count(),
        2,
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}
