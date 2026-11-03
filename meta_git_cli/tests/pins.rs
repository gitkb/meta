//! Pins: a `depends_on` entry `- lib: <sha or tag>` starts a worktree set's
//! checkout of `lib` there, and `meta git update` in the set moves it there
//! when that loses and rewrites nothing; the workspace's own checkouts
//! follow their branches.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]

use meta_test_support::Sandbox;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Output, Stdio},
};

/// A workspace `ws` listing `app` and `lib`, each cloned; `lib`'s origin
/// has commits tagged `v1` and `v2` (its `main` at `v2`). `deps` is app's
/// `depends_on`, as YAML. The commits of v1 and v2.
fn workspace(sandbox: &Sandbox, deps: &str) -> (PathBuf, String, String) {
    let root = sandbox.work();
    let up = root.join("up");
    let seed = sandbox.repo(&root.join("seed-lib"));
    let mut tagged = Vec::new();
    for tag in ["v1", "v2"] {
        sandbox.git(&seed, &["commit", "-q", "--allow-empty", "-m", tag]);
        sandbox.git(&seed, &["tag", tag]);
        tagged.push(
            sandbox
                .git(&seed, &["rev-parse", "HEAD"])
                .trim()
                .to_string(),
        );
    }
    fs::create_dir_all(&up).unwrap();
    sandbox.git(
        &up,
        &["clone", "-q", "--bare", seed.to_str().unwrap(), "lib.git"],
    );
    sandbox.bare(&up.join("app.git"));
    let ws = root.join("ws");
    sandbox.repo(&ws);
    for name in ["app", "lib"] {
        let url = up.join(format!("{name}.git"));
        sandbox.git(&ws, &["clone", "-q", url.to_str().unwrap(), name]);
    }
    let url = |name: &str| up.join(format!("{name}.git")).display().to_string();
    write_config(
        sandbox,
        &ws,
        &format!(
            "projects:\n  app:\n    repo: {}\n    depends_on: {deps}\n  lib:\n    repo: {}\n",
            url("app"),
            url("lib")
        ),
    );
    fs::write(ws.join(".gitignore"), "app\nlib\n.worktrees/\n").unwrap();
    sandbox.git(&ws, &["add", "."]);
    sandbox.git(&ws, &["commit", "-qm", "meta"]);
    let [v1, v2] = <[String; 2]>::try_from(tagged).unwrap();
    (ws, v1, v2)
}

fn write_config(sandbox: &Sandbox, dir: &Path, text: &str) {
    fs::write(dir.join(".meta.yaml"), text).unwrap();
    // the set's branch holds the change, as a pulled pin bump would
    if dir.join(".git").exists() && !sandbox.git(dir, &["log", "-1", "--format=%H"]).is_empty() {
        sandbox.git(dir, &["add", ".meta.yaml"]);
        sandbox.git(dir, &["commit", "-qm", "pin"]);
    }
}

/// `meta-git` in `cwd` through the plugin protocol.
fn meta_git(sandbox: &Sandbox, cwd: &Path, command: &str, args: &[&str], options: Value) -> Output {
    let request = json!({"command": command, "args": args, "cwd": cwd, "options": options});
    let mut child = sandbox
        .command(env!("CARGO_BIN_EXE_meta-git"))
        .arg("--meta-plugin-exec")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn update(sandbox: &Sandbox, cwd: &Path) -> Output {
    meta_git(sandbox, cwd, "git update", &[], json!({}))
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn head(sandbox: &Sandbox, dir: &Path) -> String {
    sandbox.git(dir, &["rev-parse", "HEAD"]).trim().to_string()
}

#[test]
fn a_set_starts_a_pinned_repo_at_its_pin_and_the_rest_at_origin_head() {
    let sandbox = Sandbox::new();
    let (ws, v1, v2) = workspace(&sandbox, "[{lib: v1}]");
    let out = meta_git(
        &sandbox,
        &ws,
        "git worktree create",
        &["t", "--repo", "app"],
        json!({}),
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("lib -> t (new from v1 (pinned by app))"),
        "{}",
        text(&out)
    );
    let set = ws.join(".worktrees/t");
    assert_eq!(head(&sandbox, &set.join("lib")), v1);
    // the workspace's own checkout is where it was
    assert_eq!(head(&sandbox, &ws.join("lib")), v2);

    // status says where each pinned repo is
    let out = meta_git(
        &sandbox,
        &ws,
        "git worktree status",
        &["t"],
        json!({"json_output": true}),
    );
    let status: Value = serde_json::from_slice(&out.stdout).unwrap();
    let lib = status["repos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["alias"] == "lib")
        .unwrap();
    assert_eq!(
        lib["pin"],
        json!({"rev": "v1", "by": ["app"], "at_pin": true})
    );

    // a commit-ish given starts every new branch instead
    let out = meta_git(
        &sandbox,
        &ws,
        "git worktree create",
        &["u", "v2", "--repo", "app", "--repo", "lib"],
        json!({}),
    );
    assert!(
        text(&out).contains("pins on lib aren't applied"),
        "{}",
        text(&out)
    );
    assert_eq!(head(&sandbox, &ws.join(".worktrees/u/lib")), v2);

    // add starts it there too, when a member there already pins it
    let out = meta_git(
        &sandbox,
        &ws,
        "git worktree create",
        &["w", "--no-deps", "--repo", "app"],
        json!({}),
    );
    assert!(out.status.success(), "{}", text(&out));
    let out = meta_git(
        &sandbox,
        &ws,
        "git worktree add",
        &["w", "--repo", "lib"],
        json!({}),
    );
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(head(&sandbox, &ws.join(".worktrees/w/lib")), v1);
}

#[test]
fn a_pin_origin_lacks_or_two_pins_on_one_repo_make_no_set() {
    let sandbox = Sandbox::new();
    let (ws, _, _) = workspace(&sandbox, "[{lib: v9}]");
    let out = meta_git(
        &sandbox,
        &ws,
        "git worktree create",
        &["t", "--repo", "app"],
        json!({}),
    );
    assert!(!out.status.success());
    assert!(
        text(&out).contains("Cannot fetch pin v9 (pinned by app) of 'lib'"),
        "{}",
        text(&out)
    );
    assert!(!ws.join(".worktrees/t").exists());

    // the root pins lib too, elsewhere: the set would hold one checkout
    let config = fs::read_to_string(ws.join(".meta.yaml")).unwrap();
    let config = config.replace("{lib: v9}", "{lib: v1}").replace(
        "  lib:\n",
        &format!(
            "  other:\n    repo: {}\n    depends_on: [{{lib: v2}}]\n  lib:\n",
            sandbox.work().join("up/app.git").display()
        ),
    );
    write_config(&sandbox, &ws, &config);
    let out = meta_git(
        &sandbox,
        &ws,
        "git worktree create",
        &["t", "--repo", "app", "--repo", "other"],
        json!({}),
    );
    assert!(!out.status.success());
    let said = text(&out);
    assert!(
        said.contains("'lib' is pinned at v1 by 'app' and at v2 by 'other'"),
        "{said}"
    );
    assert!(!ws.join(".worktrees/t").exists());
}

#[test]
fn update_in_a_set_moves_a_clean_checkout_to_its_pin_and_never_loses_or_rewrites() {
    let sandbox = Sandbox::new();
    let (ws, v1, v2) = workspace(&sandbox, "[{lib: v1}]");
    let out = meta_git(
        &sandbox,
        &ws,
        "git worktree create",
        &["t", "--repo", "app"],
        json!({}),
    );
    assert!(out.status.success(), "{}", text(&out));
    let set = ws.join(".worktrees/t");
    let lib = set.join("lib");
    // the pin on the set's branch, changed from what it was
    let pinned = std::cell::RefCell::new("v1".to_string());
    let pin = |rev: &str| {
        let config = fs::read_to_string(set.join(".meta.yaml")).unwrap();
        let was = pinned.replace(rev.to_string());
        let config = config.replace(&format!("{{lib: {was}}}"), &format!("{{lib: {rev}}}"));
        write_config(&sandbox, &set, &config);
    };

    // at the pin: nothing to do
    assert!(update(&sandbox, &set).status.success());
    // a pin ahead: fast-forwarded, on the set's branch
    pin(&v2);
    let out = update(&sandbox, &set);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(head(&sandbox, &lib), v2);
    assert_eq!(sandbox.git(&lib, &["branch", "--show-current"]).trim(), "t");
    // a pin behind: moved, as every commit on the branch is on origin
    pin("v1");
    assert!(update(&sandbox, &set).status.success());
    assert_eq!(head(&sandbox, &lib), v1);

    let refused = |why: &str| {
        let out = update(&sandbox, &set);
        assert!(!out.status.success(), "{why}: {}", text(&out));
        assert!(text(&out).contains(why), "{}", text(&out));
    };
    pin(&v2);
    // uncommitted changes stay, and so does the checkout
    fs::write(lib.join("README"), "edited").unwrap();
    refused("it has uncommitted changes");
    assert_eq!(head(&sandbox, &lib), v1);
    sandbox.git(&lib, &["checkout", "--", "README"]);
    // a commit nothing else holds keeps it where it is
    pin("v1");
    sandbox.git(&lib, &["commit", "-q", "--allow-empty", "-m", "mine"]);
    let mine = head(&sandbox, &lib);
    refused("1 commit on no remote, tag or other branch");
    assert_eq!(head(&sandbox, &lib), mine);
    // a branch the set didn't make is the user's: never moved
    sandbox.git(&lib, &["switch", "-q", "-c", "keep"]);
    refused("the set didn't make branch keep");
    assert_eq!(head(&sandbox, &lib), mine);
    // detached at a published commit: moved
    sandbox.git(&lib, &["switch", "-q", "--detach", &v2]);
    assert!(update(&sandbox, &set).status.success());
    assert_eq!(head(&sandbox, &lib), v1);
}

#[test]
fn update_in_the_workspace_leaves_a_pinned_checkout_on_its_branch() {
    let sandbox = Sandbox::new();
    let (ws, _, v2) = workspace(&sandbox, "[{lib: v1}]");
    let out = update(&sandbox, &ws);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(head(&sandbox, &ws.join("lib")), v2);
    assert_eq!(
        sandbox
            .git(&ws.join("lib"), &["branch", "--show-current"])
            .trim(),
        "main"
    );
}

#[test]
fn a_pin_written_in_a_nested_meta_repo_applies_to_the_repos_added_later() {
    let sandbox = Sandbox::new();
    let (ws, v1, _) = workspace(&sandbox, "[]");
    // app moves into the nested meta repo `group`, which pins lib
    let group = sandbox.repo(&ws.join("group"));
    let app_url = sandbox.work().join("up/app.git");
    fs::write(
        group.join(".meta.yaml"),
        format!(
            "projects:\n  app:\n    repo: {}\n    depends_on: [{{lib: v1}}]\n",
            app_url.display()
        ),
    )
    .unwrap();
    fs::write(group.join(".gitignore"), "app\n").unwrap();
    sandbox.git(&group, &["add", "."]);
    sandbox.git(&group, &["commit", "-qm", "group"]);
    let group_url = sandbox.work().join("up/group.git");
    sandbox.git(
        &sandbox.work().join("up"),
        &[
            "clone",
            "-q",
            "--bare",
            group.to_str().unwrap(),
            "group.git",
        ],
    );
    sandbox.git(
        &group,
        &["remote", "add", "origin", group_url.to_str().unwrap()],
    );
    sandbox.git(&group, &["fetch", "-q", "origin"]);
    sandbox.git(&group, &["remote", "set-head", "origin", "main"]);
    sandbox.git(&group, &["clone", "-q", app_url.to_str().unwrap(), "app"]);
    let lib_url = sandbox.work().join("up/lib.git");
    write_config(
        &sandbox,
        &ws,
        &format!(
            "projects:\n  group:\n    repo: {}\n    meta: true\n  lib:\n    repo: {}\n",
            group_url.display(),
            lib_url.display()
        ),
    );

    let out = meta_git(
        &sandbox,
        &ws,
        "git worktree create",
        &["t", "-r", "--no-deps", "--repo", "group/app"],
        json!({}),
    );
    assert!(out.status.success(), "{}", text(&out));
    let out = meta_git(
        &sandbox,
        &ws,
        "git worktree add",
        &["t", "--repo", "lib"],
        json!({}),
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("(new from v1 (pinned by group/app))"),
        "{}",
        text(&out)
    );
    assert_eq!(head(&sandbox, &ws.join(".worktrees/t/lib")), v1);
}
