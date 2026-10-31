//! meta's git: run from a git hook it acts on its workspace's repos, not
//! the hook's; and what it reads from each kind of repository.

use meta_core::git::run;
use std::path::Path;
use std::process::{Command, Output};

/// A repository at `dir` with one commit, on `branch`. Set up through
/// meta's own git, so a hook running this test can't commit into its repo.
fn repo(dir: &Path, branch: &str) {
    std::fs::create_dir_all(dir).unwrap();
    run(dir, ["init", "-q", "-b", branch]).unwrap();
    run(
        dir,
        [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "first",
        ],
    )
    .unwrap();
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

#[test]
fn context_shows_the_branch_of_a_bare_project() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().canonicalize().unwrap();
    run(&workspace, ["init", "-q", "--bare", "-b", "main", "mirror"]).unwrap();
    std::fs::write(
        workspace.join(".meta.yaml"),
        "projects:\n  mirror:\n    path: mirror\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_meta"))
        .args(["context", "--json", "--no-cache"])
        .current_dir(&workspace)
        .env("HOME", &workspace)
        .env("XDG_CONFIG_HOME", workspace.join("xdg"))
        .env("GIT_CONFIG_GLOBAL", workspace.join("no-gitconfig"))
        .env("META_DATA_DIR", workspace.join("meta-data"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let context: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(context["repos"][0]["branch"], "main", "{context}");
}

#[test]
fn context_shows_the_branch_of_a_checkout_git_calls_bare() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().canonicalize().unwrap();
    repo(&workspace.join("checkout"), "on-checkout");
    run(&workspace.join("checkout"), ["config", "core.bare", "true"]).unwrap();
    std::fs::write(
        workspace.join(".meta.yaml"),
        "projects:\n  checkout:\n    path: checkout\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_meta"))
        .args(["context", "--json", "--no-cache"])
        .current_dir(&workspace)
        .env("HOME", &workspace)
        .env("XDG_CONFIG_HOME", workspace.join("xdg"))
        .env("GIT_CONFIG_GLOBAL", workspace.join("no-gitconfig"))
        .env("META_DATA_DIR", workspace.join("meta-data"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let context: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    // its branch, and no word on changes git can't report
    assert_eq!(context["repos"][0]["branch"], "on-checkout", "{context}");
    assert!(context["repos"][0].get("dirty").is_none(), "{context}");
}
