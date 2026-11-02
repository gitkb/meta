//! What the git plugin takes from a protocol request: the arguments of
//! `meta git clone` and `meta git commit`, and the request's directory.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]

use meta_test_support::Sandbox;
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use std::process::{Output, Stdio};

fn meta_git(sandbox: &Sandbox, cwd: &Path, command: &str, args: &[&str], options: Value) -> Output {
    meta_git_from(sandbox, cwd, cwd, command, args, options)
}

/// [`meta_git`], with the plugin process started in `process_dir`.
fn meta_git_from(
    sandbox: &Sandbox,
    process_dir: &Path,
    cwd: &Path,
    command: &str,
    args: &[&str],
    options: Value,
) -> Output {
    let request = json!({"command": command, "args": args, "cwd": cwd, "options": options});
    let mut child = sandbox
        .command(env!("CARGO_BIN_EXE_meta-git"))
        .arg("--meta-plugin-exec")
        .current_dir(process_dir)
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
    let sandbox = Sandbox::new();
    let tmp = sandbox.work();
    let marker = tmp.join("marker");
    let upload_pack = format!("--upload-pack=touch {}; false", marker.display());
    let out = meta_git(
        &sandbox,
        &tmp,
        "git clone",
        &["--", &upload_pack, "file:///nonexistent/source.git"],
        json!({}),
    );
    assert!(!out.status.success(), "{}", text(&out));
    assert!(!marker.exists(), "git ran the URL as an option");
}

#[test]
fn clone_and_commit_refuse_what_they_do_not_take() {
    let sandbox = Sandbox::new();
    let tmp = sandbox.work();
    for (command, args, problem) in [
        ("git clone", &["--parallel", "0", "u"][..], "invalid value"),
        ("git clone", &["--meta-depth", "x", "u"], "invalid value"),
        ("git clone", &["u", "--branch", "b"], "unexpected argument"),
        ("git commit", &["--amend"], "unexpected argument"),
    ] {
        let out = meta_git(&sandbox, &tmp, command, args, json!({}));
        assert!(!out.status.success(), "{args:?}: {}", text(&out));
        assert!(text(&out).contains(problem), "{args:?}: {}", text(&out));
    }
    // nothing was cloned
    assert!(!tmp.join("u").exists());
}

#[test]
fn clone_into_an_existing_folder_says_so_before_git_runs() {
    let sandbox = Sandbox::new();
    let tmp = sandbox.work();
    std::fs::create_dir(tmp.join("ws")).unwrap();
    std::fs::write(tmp.join("ws/.meta.yaml"), "projects: {}\n").unwrap();
    std::fs::create_dir(tmp.join("other")).unwrap();
    std::fs::write(tmp.join("other/notes"), "").unwrap();
    std::fs::create_dir(tmp.join("empty")).unwrap();
    for (dir, said) in [
        ("ws", "meta git update"),
        ("other", "not an empty directory"),
    ] {
        for options in [json!({}), json!({"dry_run": true})] {
            let out = meta_git(
                &sandbox,
                &tmp,
                "git clone",
                &["file:///gone.git", dir],
                options,
            );
            assert!(!out.status.success(), "{}", text(&out));
            assert!(text(&out).contains(said), "{}", text(&out));
            // before git, and before saying it clones
            assert!(!text(&out).contains("Cloning"), "{}", text(&out));
            assert!(!text(&out).contains("fatal"), "{}", text(&out));
        }
    }
    // git takes an empty directory
    let out = meta_git(
        &sandbox,
        &tmp,
        "git clone",
        &["file:///gone.git", "empty"],
        json!({}),
    );
    assert!(text(&out).contains("fatal"), "{}", text(&out));
}

#[test]
fn clone_and_commit_have_their_own_help() {
    let sandbox = Sandbox::new();
    let tmp = sandbox.work();
    // meta sends a help request without its repos, so `-m --help` is help
    // too, never a commit of every repo
    for (command, args) in [
        ("git clone", &["--help"][..]),
        ("git commit", &["--help"]),
        ("git commit", &["-m", "--help"]),
    ] {
        let out = meta_git(&sandbox, &tmp, command, args, json!({}));
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
    let sandbox = Sandbox::new();
    let tmp = sandbox.work();
    let dry_run = |args: &[&str]| {
        let out = meta_git(
            &sandbox,
            &tmp,
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
    // git reads a `:` as a separator too
    assert!(dry_run(&["git@host:a:b.git"]).contains("-- git@host:a:b.git b\n"));
    #[cfg(unix)]
    {
        std::fs::write(tmp.join("h:ws.bundle"), "").unwrap();
        assert!(dry_run(&["h:ws.bundle"]).contains("-- h:ws.bundle ws\n"));
    }
    // a bundle file clones into its name without `.bundle`, as git does,
    // and keeps a `.git` (from another folder: git won't clone onto it)
    std::fs::create_dir(tmp.join("src")).unwrap();
    for (bundle, dir) in [("src/ws.bundle", "ws"), ("src/other.git", "other.git")] {
        std::fs::write(tmp.join(bundle), "").unwrap();
        assert!(
            dry_run(&[bundle]).contains(&format!("-- {bundle} {dir}")),
            "{bundle}"
        );
    }
    // a linked worktree's `.git` file is no bundle: its checkout's name
    std::fs::create_dir(tmp.join("src/wt")).unwrap();
    std::fs::write(tmp.join("src/wt/.git"), "gitdir: elsewhere\n").unwrap();
    assert!(dry_run(&["src/wt/.git"]).contains("-- src/wt/.git wt"));
}

#[test]
fn worktree_list_finds_the_workspace_of_the_requests_directory() {
    let sandbox = Sandbox::new();
    let tmp = sandbox.work();
    let workspace = tmp.join("workspace");
    let repo = workspace.join("a");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        workspace.join(".meta.yaml"),
        "projects:\n  a:\n    path: a\n",
    )
    .unwrap();
    sandbox.repo(&repo);
    let set_member = workspace.join(".worktrees/s/a");
    sandbox.git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "s",
            &set_member.to_string_lossy(),
        ],
    );
    let elsewhere = tmp.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();

    let out = meta_git_from(
        &sandbox,
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

/// A plugin whose stdout meta no longer reads (meta was killed) ends
/// quietly, without a panic.
#[test]
fn a_closed_stdout_ends_the_plugin_without_a_panic() {
    let sandbox = Sandbox::new();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = sandbox
        .command(env!("CARGO_BIN_EXE_meta-git"))
        .arg("--meta-plugin-info")
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(out.status.code(), Some(141), "{stderr}");
}
