//! What the git plugin takes from a protocol request: the arguments of
//! `meta git clone` and `meta git commit`, and the request's directory.

use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn meta_git(cwd: &Path, command: &str, args: &[&str], options: Value) -> Output {
    meta_git_from(cwd, cwd, command, args, options)
}

/// [`meta_git`], with the plugin process started in `process_dir`.
fn meta_git_from(
    process_dir: &Path,
    cwd: &Path,
    command: &str,
    args: &[&str],
    options: Value,
) -> Output {
    let request = json!({"command": command, "args": args, "cwd": cwd, "options": options});
    let mut child = Command::new(env!("CARGO_BIN_EXE_meta-git"))
        .arg("--meta-plugin-exec")
        .current_dir(process_dir)
        .env_remove("META_WORKTREES")
        .env("HOME", cwd)
        .env("XDG_CONFIG_HOME", cwd.join("xdg"))
        .env("GIT_CONFIG_GLOBAL", cwd.join("no-gitconfig"))
        .env("META_DATA_DIR", cwd.join("meta-data"))
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

fn text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr)
}

#[test]
fn clone_never_passes_a_url_to_git_as_an_option() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("marker");
    let upload_pack = format!("--upload-pack=touch {}; false", marker.display());
    let out = meta_git(
        tmp.path(),
        "git clone",
        &["--", &upload_pack, "file:///nonexistent/source.git"],
        json!({}),
    );
    assert!(!out.status.success(), "{}", text(&out));
    assert!(!marker.exists(), "git ran the URL as an option");
}

#[test]
fn clone_and_commit_refuse_what_they_do_not_take() {
    let tmp = tempfile::tempdir().unwrap();
    for (command, args, problem) in [
        ("git clone", &["--parallel", "0", "u"][..], "invalid value"),
        ("git clone", &["--meta-depth", "x", "u"], "invalid value"),
        ("git clone", &["u", "--branch", "b"], "unexpected argument"),
        ("git commit", &["--amend"], "unexpected argument"),
    ] {
        let out = meta_git(tmp.path(), command, args, json!({}));
        assert!(!out.status.success(), "{args:?}: {}", text(&out));
        assert!(text(&out).contains(problem), "{args:?}: {}", text(&out));
    }
    // nothing was cloned
    assert!(!tmp.path().join("u").exists());
}

#[test]
fn clone_and_commit_have_their_own_help() {
    let tmp = tempfile::tempdir().unwrap();
    // meta sends a help request without its repos, so `-m --help` is help
    // too, never a commit of every repo
    for (command, args) in [
        ("git clone", &["--help"][..]),
        ("git commit", &["--help"]),
        ("git commit", &["-m", "--help"]),
    ] {
        let out = meta_git(tmp.path(), command, args, json!({}));
        assert!(out.status.success(), "{}", text(&out));
        assert!(
            text(&out).contains(&format!("Usage: meta {command}")),
            "{}",
            text(&out)
        );
    }
}

#[test]
fn clone_depth_comes_from_meta_unless_given() {
    let tmp = tempfile::tempdir().unwrap();
    let dry_run = |args: &[&str]| {
        let out = meta_git(
            tmp.path(),
            "git clone",
            args,
            json!({"dry_run": true, "depth": 3}),
        );
        assert!(out.status.success(), "{}", text(&out));
        text(&out)
    };
    assert!(dry_run(&["u"]).contains("git clone --depth 3 -- u u"));
    assert!(dry_run(&["--depth", "1", "u", "d"]).contains("git clone --depth 1 -- u d"));
    // the directory git would name, for an IPv6 host too
    assert!(dry_run(&["git@[::1]:repo.git"]).contains(":repo.git' repo"));
    // a bundle file clones into its name without `.bundle`, as git does
    std::fs::write(tmp.path().join("ws.bundle"), "").unwrap();
    assert!(dry_run(&["ws.bundle"]).contains("-- ws.bundle ws"));
}

#[test]
fn worktree_list_finds_the_workspace_of_the_requests_directory() {
    // not canonicalized: git refuses Windows' `\\?\` form
    let tmp = tempfile::tempdir().unwrap();
    let tmp = tmp.path();
    let workspace = tmp.join("workspace");
    let repo = workspace.join("a");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        workspace.join(".meta.yaml"),
        "projects:\n  a:\n    path: a\n",
    )
    .unwrap();
    // through meta's own git, so no git variable around the test reaches it
    let git = |args: &[&str]| {
        let out = meta_core::git::git(&repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", tmp.join("no-gitconfig"))
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out));
    };
    git(&["init", "-q"]);
    git(&["commit", "-q", "--allow-empty", "-m", "a"]);
    let set_member = workspace.join(".worktrees/s/a");
    git(&[
        "worktree",
        "add",
        "-q",
        "-b",
        "s",
        &set_member.to_string_lossy(),
    ]);
    let elsewhere = tmp.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();

    let out = meta_git_from(
        &elsewhere,
        &workspace,
        "git worktree list",
        &[],
        json!({"json_output": true}),
    );
    assert!(out.status.success(), "{}", text(&out));
    let listed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listed["worktrees"][0]["name"], "s", "{listed}");
}
