#![expect(
    clippy::disallowed_methods,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]

use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

/// Run a Git fixture command and surface diagnostics on failure.
fn git(path: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Invoke the public plugin protocol with isolated worktree and data roots.
/// Whether `stdout` is the JSON error a failed `--json` command prints in
/// place of its report.
fn is_error(stdout: &[u8]) -> bool {
    serde_json::from_slice::<Value>(stdout).is_ok_and(|v| {
        v["success"] == false && v["error"].is_string() && v.get("worktrees").is_none()
    })
}

fn request(root: &Path, command: &str, recursive: bool, explicit: bool) -> std::process::Output {
    let args = if command == "status" {
        vec!["example"]
    } else {
        vec![]
    };
    request_args(root, command, args, recursive, explicit)
}

fn request_args(
    root: &Path,
    command: &str,
    mut args: Vec<&str>,
    recursive: bool,
    explicit: bool,
) -> std::process::Output {
    if explicit {
        args.push("--recursive");
    }
    let request = json!({"command": format!("git worktree {command}"), "args": args,
        "cwd": root, "options": {"json_output": true, "recursive": recursive}});
    let mut executable = Command::new(env!("CARGO_BIN_EXE_meta-git"));
    if root.join("bin").is_dir() {
        executable.env("PATH", root.join("bin"));
    }
    let mut child = executable
        .arg("--meta-plugin-exec")
        .current_dir(root)
        .env("META_WORKTREES", root.join("sets"))
        .env("META_DATA_DIR", root.join("data"))
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

#[test]
/// Verify real nested members, option forwarding and command-specific error output.
fn public_list_status_recursive_protocol_and_explicit_flags() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let set = root.join("sets").join("example");
    for (index, alias) in [".", "open-source", "open-source/atc", "other/atc"]
        .iter()
        .enumerate()
    {
        let source = root.join(format!("source{index}"));
        fs::create_dir(&source).unwrap();
        git(&source, &["init", "-q"]);
        git(
            &source,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
        );
        let destination = set.join(alias);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        git(
            &source,
            &[
                "worktree",
                "add",
                "-qb",
                "topic",
                destination.to_str().unwrap(),
            ],
        );
    }
    fs::write(set.join("open-source/atc/untracked.txt"), "dirty").unwrap();
    for command in ["list", "status"] {
        for (recursive, explicit) in [(false, false), (true, false), (false, true), (true, true)] {
            let out = request(root, command, recursive, explicit);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let value: Value = serde_json::from_slice(&out.stdout).unwrap();
            if command == "list" {
                let named = request_args(root, "list", vec!["example"], recursive, explicit);
                assert!(named.status.success());
                assert_eq!(
                    serde_json::from_slice::<Value>(&named.stdout).unwrap(),
                    value
                );
            }
            let rows = if command == "list" {
                &value["worktrees"][0]["repos"]
            } else {
                &value["repos"]
            };
            let aliases: Vec<_> = rows
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["alias"].as_str().unwrap().replace('\\', "/"))
                .collect();
            if recursive || explicit {
                assert_eq!(
                    aliases,
                    [".", "open-source", "open-source/atc", "other/atc"]
                );
                let nested = rows
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| {
                        row["alias"].as_str().unwrap().replace('\\', "/") == "open-source/atc"
                    })
                    .unwrap();
                assert_eq!(nested["branch"], "topic");
                assert_eq!(nested["dirty"], true);
            } else {
                assert_eq!(aliases, [".", "open-source", "other/atc"]);
            }
        }
    }
    fs::write(set.join("open-source/atc/.git"), "broken").unwrap();
    let healthy_set = root.join("sets").join("healthy");
    git(
        &root.join("source0"),
        &[
            "worktree",
            "add",
            "-qb",
            "healthy",
            healthy_set.to_str().unwrap(),
        ],
    );
    let out = request(root, "list", true, false);
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    let worktrees = value["worktrees"].as_array().unwrap();
    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0]["name"], "healthy");
    assert_eq!(worktrees[0]["root"], healthy_set.to_str().unwrap());
    assert_eq!(
        worktrees[0]["repos"],
        json!([{"alias": ".", "branch": "healthy", "dirty": false}])
    );
    assert_eq!(value["warnings"].as_array().unwrap().len(), 1);
    assert_eq!(value["warnings"][0]["name"], "example");
    assert_eq!(value["warnings"][0]["root"], set.to_str().unwrap());
    assert!(value["warnings"][0]["message"]
        .as_str()
        .unwrap()
        .contains("Invalid .git file format"));

    let out = request(root, "status", true, false);
    assert!(!out.status.success());
    assert!(
        is_error(&out.stdout),
        "named status must not emit an incomplete successful inventory"
    );
    let selected = request_args(root, "list", vec!["example"], true, false);
    assert!(!selected.status.success());
    assert!(is_error(&selected.stdout));
}

/// A missing configured root is empty for list and an error for named status.
#[test]
fn missing_worktree_root_preserves_list_and_status_contracts() {
    let temp = tempfile::tempdir().unwrap();
    for recursive in [false, true] {
        let out = request(temp.path(), "list", recursive, false);
        assert!(out.status.success());
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value, json!({"worktrees": []}));
        let out = request(temp.path(), "status", recursive, false);
        assert!(!out.status.success());
        assert!(is_error(&out.stdout));
    }
}

#[cfg(unix)]
#[test]
fn membership_only_protocol_runs_without_git_and_rejects_partial_membership() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    git(&source, &["init", "-q"]);
    git(
        &source,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
    );
    let set = root.join("sets/example");
    fs::create_dir_all(set.parent().unwrap()).unwrap();
    git(
        &source,
        &["worktree", "add", "-qb", "topic", set.to_str().unwrap()],
    );
    fs::write(set.join("untracked"), "dirty, deliberately unobserved").unwrap();
    let nested = set.join("nested/child");
    fs::create_dir_all(nested.parent().unwrap()).unwrap();
    git(
        &source,
        &["worktree", "add", "-qb", "nested", nested.to_str().unwrap()],
    );

    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    let fake_git = bin.join("git");
    fs::write(
        &fake_git,
        "#!/bin/sh\nprintf called >> \"$0.calls\"\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&fake_git, fs::Permissions::from_mode(0o700)).unwrap();
    for (recursive, explicit) in [(true, false), (false, true)] {
        let out = request_args(
            &root,
            "list",
            vec!["example", "--metadata-only"],
            recursive,
            explicit,
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        let value = &value["worktrees"][0];
        assert_eq!(value["name"], "example");
        assert_eq!(value["repos"].as_array().unwrap().len(), 2);
        assert_eq!(value["repos"][1]["alias"], "nested/child");
        assert_eq!(value["repos"][1]["path"], nested.to_str().unwrap());
        for member in value["repos"].as_array().unwrap() {
            assert_eq!(member["source_path"], source.to_str().unwrap());
            assert!(member.get("branch").is_none());
            assert!(member.get("dirty").is_none());
        }
        assert_eq!(value["repos"][0]["alias"], ".");
        assert_eq!(value["repos"][0]["path"], set.to_str().unwrap());
        assert!(value["repos"][0].get("branch").is_none());
        assert!(value["repos"][0].get("dirty").is_none());
    }
    assert!(!bin.join("git.calls").exists(), "membership launched Git");
    fs::create_dir(set.join("broken")).unwrap();
    fs::write(set.join("broken/.git"), "broken").unwrap();
    let out = request_args(
        &root,
        "list",
        vec!["example", "--metadata-only"],
        true,
        false,
    );
    assert!(!out.status.success());
    assert!(
        is_error(&out.stdout),
        "must not report a partial successful inventory"
    );
    assert!(!bin.join("git.calls").exists());
    let aggregate = request_args(&root, "list", vec!["--metadata-only"], true, false);
    assert!(aggregate.status.success());
    let value: Value = serde_json::from_slice(&aggregate.stdout).unwrap();
    assert!(value["worktrees"].as_array().unwrap().is_empty());
    assert_eq!(value["warnings"].as_array().unwrap().len(), 1);
    assert!(!bin.join("git.calls").exists());
}

/// Inference selects the enclosing physical set, including from deep descendants,
/// without consulting a different configured worktree root or launching Git.
#[cfg(unix)]
#[test]
fn list_infers_enclosing_set_and_metadata_only_avoids_git() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    git(&source, &["init", "-q"]);
    git(
        &source,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-qm",
            "initial",
        ],
    );
    let set = root.join(".worktrees/example");
    fs::create_dir_all(set.parent().unwrap()).unwrap();
    git(
        &source,
        &["worktree", "add", "-qb", "topic", set.to_str().unwrap()],
    );
    let nested = set.join("deep/directory");
    fs::create_dir_all(&nested).unwrap();
    fs::write(set.join("untracked"), "dirty").unwrap();
    let other = root.join("configured/other");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    git(
        &source,
        &["worktree", "add", "-qb", "other", other.to_str().unwrap()],
    );
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    let fake_git = bin.join("git");
    fs::write(
        &fake_git,
        "#!/bin/sh\nprintf called >> \"$0.calls\"\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&fake_git, fs::Permissions::from_mode(0o700)).unwrap();
    let invoke = |cwd: &Path, args: Vec<&str>, recursive: bool| {
        let mut child = Command::new(env!("CARGO_BIN_EXE_meta-git"))
            .arg("--meta-plugin-exec")
            .current_dir(&source)
            .env(
                "PATH",
                if args.contains(&"--metadata-only") {
                    bin.clone().into_os_string()
                } else {
                    std::env::var_os("PATH").unwrap()
                },
            )
            .env("META_WORKTREES", root.join("configured"))
            .env("META_DATA_DIR", root.join("data"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let request = json!({"command": "git worktree list", "args": args,
            "cwd": cwd, "options": {"json_output": true, "recursive": recursive}});
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    };
    for cwd in [&set, &nested] {
        for (args, recursive) in [
            (vec!["--metadata-only"], true),
            (vec!["--metadata-only", "--recursive"], false),
        ] {
            let out = invoke(cwd, args, recursive);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let value: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(value["worktrees"].as_array().unwrap().len(), 1);
            let value = &value["worktrees"][0];
            assert_eq!(value["name"], "example");
            assert_eq!(value["repos"][0]["path"], set.to_str().unwrap());
        }
    }
    let explicit = invoke(&nested, vec!["other", "--metadata-only"], true);
    assert!(explicit.status.success());
    let value: Value = serde_json::from_slice(&explicit.stdout).unwrap();
    assert_eq!(value["worktrees"][0]["name"], "other");
    assert_eq!(
        value["worktrees"][0]["repos"][0]["path"],
        other.to_str().unwrap()
    );

    // Outside a worktree, omission still lists all configured sets.
    let outside = invoke(&source, vec!["--metadata-only"], true);
    assert!(outside.status.success());
    let value: Value = serde_json::from_slice(&outside.stdout).unwrap();
    assert_eq!(value["worktrees"][0]["name"], "other");
    // Metadata-only is independent of recursion; legacy boundaries remain available.
    assert!(invoke(&set, vec!["--metadata-only"], false)
        .status
        .success());
    // Default observed list also infers the set and retains its existing fields.
    for cwd in [&set, &nested] {
        let out = invoke(cwd, vec![], false);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["worktrees"].as_array().unwrap().len(), 1);
        assert_eq!(value["worktrees"][0]["name"], "example");
        assert_eq!(value["worktrees"][0]["repos"][0]["branch"], "topic");
        assert_eq!(value["worktrees"][0]["repos"][0]["dirty"], true);
    }
    let missing = invoke(&source, vec!["missing", "--metadata-only"], true);
    assert!(!missing.status.success());
    assert!(is_error(&missing.stdout));

    fs::create_dir(set.join("broken")).unwrap();
    fs::write(set.join("broken/.git"), "broken").unwrap();
    let malformed = invoke(&nested, vec!["--metadata-only"], true);
    assert!(!malformed.status.success());
    assert!(
        is_error(&malformed.stdout),
        "inference must not mask invalid membership"
    );
    assert!(!bin.join("git.calls").exists(), "inference launched Git");
    // An invalid enclosing root must not turn into an all-set or empty success.
    fs::write(set.join(".git"), "broken").unwrap();
    let invalid_root = invoke(&nested, vec!["--metadata-only"], true);
    assert!(!invalid_root.status.success());
    assert!(is_error(&invalid_root.stdout));
}
