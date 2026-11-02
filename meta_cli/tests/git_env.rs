//! meta's git: run from a git hook it acts on its workspace's repos, not
//! the hook's; and what it reads from each kind of repository.

use meta_test_support::Sandbox;
use std::process::Output;

#[test]
fn a_hooks_repository_variables_do_not_reach_metas_git() {
    let sandbox = Sandbox::new();
    let tmp = sandbox.work();
    let workspace = tmp.join("workspace");
    sandbox.repo(&workspace);
    sandbox.git(&workspace, &["branch", "-m", "on-workspace"]);
    sandbox.repo(&workspace.join("a"));
    sandbox.git(&workspace.join("a"), &["branch", "-m", "on-a"]);
    std::fs::write(
        workspace.join(".meta.yaml"),
        "projects:\n  a:\n    path: a\n",
    )
    .unwrap();
    // The repository whose hook runs meta, as git sets it up for one
    let hook = tmp.join("hook");
    sandbox.repo(&hook);
    sandbox.git(&hook, &["branch", "-m", "on-hook"]);
    let meta = |args: &[&str]| -> Output {
        let out = sandbox
            .command(env!("CARGO_BIN_EXE_meta"))
            .args(args)
            .current_dir(&workspace)
            .env("GIT_DIR", hook.join(".git"))
            .env("GIT_WORK_TREE", &hook)
            .env("GIT_INDEX_FILE", hook.join(".git/index"))
            // the user's own settings, which git passes on to submodules too
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "meta.test")
            .env("GIT_CONFIG_VALUE_0", "kept")
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
        serde_json::from_slice(&meta(&["context", "--json"]).stdout).unwrap();
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
    let sandbox = Sandbox::new();
    let workspace = sandbox.work();
    sandbox.git(
        &workspace,
        &["init", "-q", "--bare", "-b", "main", "mirror"],
    );
    std::fs::write(
        workspace.join(".meta.yaml"),
        "projects:\n  mirror:\n    path: mirror\n",
    )
    .unwrap();
    let out = sandbox
        .command(env!("CARGO_BIN_EXE_meta"))
        .args(["context", "--json"])
        .current_dir(&workspace)
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
    let sandbox = Sandbox::new();
    let workspace = sandbox.work();
    sandbox.repo(&workspace.join("checkout"));
    sandbox.git(
        &workspace.join("checkout"),
        &["branch", "-m", "on-checkout"],
    );
    sandbox.git(
        &workspace.join("checkout"),
        &["config", "core.bare", "true"],
    );
    std::fs::write(
        workspace.join(".meta.yaml"),
        "projects:\n  checkout:\n    path: checkout\n",
    )
    .unwrap();
    let out = sandbox
        .command(env!("CARGO_BIN_EXE_meta"))
        .args(["context", "--json"])
        .current_dir(&workspace)
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
