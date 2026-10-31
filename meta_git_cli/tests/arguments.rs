//! The arguments `meta git clone` and `meta git commit` take, through the
//! plugin protocol.

use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn meta_git(cwd: &Path, command: &str, args: &[&str], options: Value) -> Output {
    let request = json!({"command": command, "args": args, "cwd": cwd, "options": options});
    let mut child = Command::new(env!("CARGO_BIN_EXE_meta-git"))
        .arg("--meta-plugin-exec")
        .current_dir(cwd)
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
    for command in ["git clone", "git commit"] {
        let out = meta_git(tmp.path(), command, &["--help"], json!({}));
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
    assert!(dry_run(&["u"]).contains("git clone --depth 3 u u"));
    assert!(dry_run(&["--depth", "1", "u", "d"]).contains("git clone --depth 1 u d"));
}
