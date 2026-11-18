#![expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]

mod common;

use common::{as_meta_starts, meta_git_with, Ran};
use meta_plugin::testing::Sandbox;
use serde_json::{json, Value};
use std::{fs, path::Path};

/// Whether `stdout` is the Envelope of a failed `--json` command: an error
/// in place of its report.
fn is_error(stdout: &[u8]) -> bool {
    serde_json::from_slice::<Value>(stdout).is_ok_and(|v| {
        v["success"] == false
            && v["error"]["message"].is_string()
            && v["results"].as_array().is_some_and(|r| r.is_empty())
    })
}

fn request_of(sandbox: &Sandbox, root: &Path, command: &str, strict: bool, explicit: bool) -> Ran {
    let args = if command == "status" {
        vec!["example"]
    } else {
        vec![]
    };
    request_args(sandbox, root, command, args, strict, explicit)
}

/// Invoke the public plugin protocol with isolated worktree and data roots.
fn request_args(
    sandbox: &Sandbox,
    root: &Path,
    command: &str,
    args: Vec<&str>,
    strict: bool,
    explicit: bool,
) -> Ran {
    // the strict scan is meta's --strict, whether the caller asked for it
    // through the option or typed it (before the command, as meta sends it)
    let mut executable = sandbox.command(env!("CARGO_BIN_EXE_meta-git"));
    if root.join("bin").is_dir() {
        executable.env("PATH", root.join("bin"));
    }
    executable
        .current_dir(root)
        .env("META_WORKTREES", root.join("sets"));
    meta_git_with(
        &mut executable,
        root,
        &format!("git worktree {command}"),
        &args,
        json!({"json": true, "strict": strict || explicit}),
    )
}

#[test]
/// Verify real nested members, option forwarding and command-specific error output.
fn public_list_status_recursive_protocol_and_explicit_flags() {
    let sandbox = Sandbox::new();
    let temp = sandbox.work();
    let root = &temp;
    let set = root.join("sets").join("example");
    for (index, alias) in [".", "open-source", "open-source/atc", "other/atc"]
        .iter()
        .enumerate()
    {
        let source = root.join(format!("source{index}"));
        fs::create_dir(&source).unwrap();
        sandbox.git(&source, &["init", "-q"]);
        sandbox.git(&source, &["commit", "--allow-empty", "-qm", "initial"]);
        let destination = set.join(alias);
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        sandbox.git(
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
    // a repo nested in a member is one of its .meta.yaml's projects
    fs::write(set.join("open-source/.meta.yaml"), "projects:\n  atc: u\n").unwrap();
    for command in ["list", "status"] {
        for (recursive, explicit) in [(false, false), (true, false), (false, true), (true, true)] {
            let out = request_of(&sandbox, root, command, recursive, explicit);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let value: Value = serde_json::from_slice(&out.stdout).unwrap();
            if command == "list" {
                let named =
                    request_args(&sandbox, root, "list", vec!["example"], recursive, explicit);
                assert!(named.status.success());
                assert_eq!(
                    serde_json::from_slice::<Value>(&named.stdout).unwrap(),
                    value
                );
            }
            // list's one set, status's set
            let rows = &value["results"][0]["data"]["repos"];
            let aliases: Vec<_> = rows
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row["alias"].as_str().unwrap().replace('\\', "/"))
                .collect();
            // every member is listed, nested ones too, with or without -r
            assert_eq!(
                aliases,
                [".", "open-source", "open-source/atc", "other/atc"]
            );
            if recursive || explicit {
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
            }
        }
    }
    fs::write(set.join("open-source/atc/.git"), "broken").unwrap();
    let healthy_set = root.join("sets").join("healthy");
    sandbox.git(
        &root.join("source0"),
        &[
            "worktree",
            "add",
            "-qb",
            "healthy",
            healthy_set.to_str().unwrap(),
        ],
    );
    let out = request_of(&sandbox, root, "list", true, false);
    assert!(out.status.success());
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    let worktrees: Vec<&Value> = value["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| &row["data"])
        .collect();
    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0]["name"], "healthy");
    assert_eq!(worktrees[0]["root"], healthy_set.to_str().unwrap());
    assert_eq!(
        worktrees[0]["repos"],
        json!([{"alias": ".", "branch": "healthy", "dirty": false}])
    );
    // the set that can't be read is named, with why
    assert_eq!(value["warnings"].as_array().unwrap().len(), 1);
    let warning = value["warnings"][0]["message"].as_str().unwrap();
    assert!(
        warning.starts_with("example: ") && warning.contains("Invalid .git file format"),
        "{warning}"
    );

    let out = request_of(&sandbox, root, "status", true, false);
    assert!(!out.status.success());
    assert!(
        is_error(&out.stdout),
        "named status must not emit an incomplete successful inventory"
    );
    let selected = request_args(&sandbox, root, "list", vec!["example"], true, false);
    assert!(!selected.status.success());
    assert!(is_error(&selected.stdout));
}

/// A missing configured root is empty for list and an error for named status.
#[test]
fn missing_worktree_root_preserves_list_and_status_contracts() {
    let sandbox = Sandbox::new();
    let temp = sandbox.work();
    for recursive in [false, true] {
        let out = request_of(&sandbox, &temp, "list", recursive, false);
        assert!(out.status.success());
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(value["success"], true, "{value}");
        assert_eq!(value["results"], json!([]), "{value}");
        let out = request_of(&sandbox, &temp, "status", recursive, false);
        assert!(!out.status.success());
        assert!(is_error(&out.stdout));
    }
}

#[cfg(unix)]
#[test]
fn membership_only_protocol_runs_without_git_and_rejects_partial_membership() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    let temp = sandbox.work();
    let root = temp.canonicalize().unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    sandbox.git(&source, &["init", "-q"]);
    sandbox.git(&source, &["commit", "--allow-empty", "-qm", "initial"]);
    let set = root.join("sets/example");
    fs::create_dir_all(set.parent().unwrap()).unwrap();
    sandbox.git(
        &source,
        &["worktree", "add", "-qb", "topic", set.to_str().unwrap()],
    );
    fs::write(set.join("untracked"), "dirty, deliberately unobserved").unwrap();
    let nested = set.join("nested/child");
    fs::create_dir_all(nested.parent().unwrap()).unwrap();
    sandbox.git(
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
            &sandbox,
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
        let value = &value["results"][0]["data"];
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
        &sandbox,
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
    let aggregate = request_args(
        &sandbox,
        &root,
        "list",
        vec!["--metadata-only"],
        true,
        false,
    );
    assert!(aggregate.status.success());
    let value: Value = serde_json::from_slice(&aggregate.stdout).unwrap();
    assert!(value["results"].as_array().unwrap().is_empty());
    assert_eq!(value["warnings"].as_array().unwrap().len(), 1);
    assert!(!bin.join("git.calls").exists());
}

/// Inference selects the enclosing physical set, including from deep descendants,
/// without consulting a different configured worktree root or launching Git.
#[cfg(unix)]
#[test]
fn list_infers_enclosing_set_and_metadata_only_avoids_git() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    let temp = sandbox.work();
    let root = temp.canonicalize().unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    sandbox.git(&source, &["init", "-q"]);
    sandbox.git(&source, &["commit", "--allow-empty", "-qm", "initial"]);
    let set = root.join(".worktrees/example");
    fs::create_dir_all(set.parent().unwrap()).unwrap();
    sandbox.git(
        &source,
        &["worktree", "add", "-qb", "topic", set.to_str().unwrap()],
    );
    let nested = set.join("deep/directory");
    fs::create_dir_all(&nested).unwrap();
    fs::write(set.join("untracked"), "dirty").unwrap();
    let other = root.join("configured/other");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    sandbox.git(
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
        let mut command = sandbox.command(env!("CARGO_BIN_EXE_meta-git"));
        command
            .current_dir(&source)
            .env(
                "PATH",
                if args.contains(&"--metadata-only") {
                    bin.clone().into_os_string()
                } else {
                    std::env::var_os("PATH").unwrap()
                },
            )
            .env("META_WORKTREES", root.join("configured"));
        meta_git_with(
            &mut command,
            cwd,
            "git worktree list",
            &args,
            json!({"json": true, "strict": recursive}),
        )
    };
    // the sets listed, one row each
    let sets = |value: &Value| -> Vec<Value> {
        value["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["data"].clone())
            .collect()
    };
    for cwd in [&set, &nested] {
        for (args, recursive) in [
            (vec!["--metadata-only"], true),
            (vec!["--metadata-only"], false),
        ] {
            let out = invoke(cwd, args, recursive);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let value: Value = serde_json::from_slice(&out.stdout).unwrap();
            let sets = sets(&value);
            assert_eq!(sets.len(), 1);
            assert_eq!(sets[0]["name"], "example");
            assert_eq!(sets[0]["repos"][0]["path"], set.to_str().unwrap());
        }
    }
    let explicit = invoke(&nested, vec!["other", "--metadata-only"], true);
    assert!(explicit.status.success());
    let value: Value = serde_json::from_slice(&explicit.stdout).unwrap();
    assert_eq!(sets(&value)[0]["name"], "other");
    assert_eq!(sets(&value)[0]["repos"][0]["path"], other.to_str().unwrap());

    // Outside a worktree, omission still lists all configured sets.
    let outside = invoke(&source, vec!["--metadata-only"], true);
    assert!(outside.status.success());
    let value: Value = serde_json::from_slice(&outside.stdout).unwrap();
    assert_eq!(sets(&value)[0]["name"], "other");
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
        let sets = sets(&value);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0]["name"], "example");
        assert_eq!(sets[0]["repos"][0]["branch"], "topic");
        assert_eq!(sets[0]["repos"][0]["dirty"], true);
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

/// `meta --json --dry-run -r git worktree create s --repo <repo>` in `cwd`:
/// the real host and plugin; whether it succeeded, and its Envelope.
fn previewed(sandbox: &Sandbox, cwd: &Path, repo: &str) -> (bool, Value) {
    let bin = Path::new(env!("CARGO_BIN_EXE_meta-git")).parent().unwrap();
    let mut meta = sandbox.command(bin.join("meta"));
    as_meta_starts(&mut meta);
    let output = meta
        .args([
            "--json",
            "--dry-run",
            "-r",
            "git",
            "worktree",
            "create",
            "s",
        ])
        .args(["--repo", repo, "--branch", "b"])
        .current_dir(cwd)
        .output()
        .unwrap();
    let envelope = serde_json::from_slice(&output.stdout).unwrap();
    (output.status.success(), envelope)
}

#[test]
fn a_preview_returns_its_plan_with_nested_repos_in_one_row() {
    let sandbox = Sandbox::new();
    let root = sandbox.work().join("ws");
    sandbox
        .workspace(&root)
        .nested("app", |app| app.repo("lib"))
        .build();

    let (succeeded, envelope) = previewed(&sandbox, &root, "app/lib");

    assert!(succeeded, "{envelope}");
    assert_eq!(
        envelope["results"].as_array().unwrap().len(),
        1,
        "{envelope}"
    );
    let row = &envelope["results"][0];
    assert_eq!(row.as_object().unwrap().len(), 1, "{row}");
    let repos: Vec<(&str, &Path, &Path)> = row["data"]["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|repo| {
            let known = ["alias", "source", "path", "created_branch", "started_from"];
            assert!(
                repo.as_object()
                    .unwrap()
                    .keys()
                    .all(|key| known.contains(&key.as_str())),
                "{repo}"
            );
            let path = |field: &str| Path::new(repo[field].as_str().unwrap());
            (
                repo["alias"].as_str().unwrap(),
                path("source"),
                path("path"),
            )
        })
        .collect();
    let set = root.join(".worktrees/s");
    assert_eq!(
        repos,
        [
            (".", root.as_path(), set.as_path()),
            ("app", root.join("app").as_path(), set.join("app").as_path()),
            (
                "app/lib",
                root.join("app/lib").as_path(),
                set.join("app/lib").as_path()
            ),
        ]
    );
    assert!(!root.join(".worktrees").exists());
}

#[test]
fn a_preview_that_must_clone_first_has_no_plan_and_no_row() {
    let sandbox = Sandbox::new();
    let root = sandbox.work().join("ws");
    sandbox.workspace(&root).uncloned("later").build();

    let (succeeded, envelope) = previewed(&sandbox, &root, "later");

    assert!(succeeded, "{envelope}");
    assert_eq!(envelope["results"], json!([]));
    assert_eq!(
        envelope["skipped"],
        json!([{
            "project": "later",
            "reason": "not cloned: a create clones it first, so a preview has no plan",
            "quiet": true,
        }])
    );
}

#[test]
fn a_preview_names_the_uncloned_meta_repo_on_the_way() {
    let sandbox = Sandbox::new();
    let root = sandbox.work().join("ws");
    sandbox.workspace(&root).uncloned("clients").build();
    std::fs::write(
        root.join(".meta.yaml"),
        "projects:\n  clients:\n    repo: https://example.invalid/clients.git\n    meta: true\n",
    )
    .unwrap();

    let (succeeded, envelope) = previewed(&sandbox, &root, "clients/desktop");

    assert!(succeeded, "{envelope}");
    assert_eq!(envelope["results"], json!([]));
    assert_eq!(envelope["skipped"][0]["project"], "clients", "{envelope}");
}

/// (macOS file systems refuse such names)
#[cfg(target_os = "linux")]
#[test]
fn a_preview_names_a_path_that_is_not_utf8_by_its_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let sandbox = Sandbox::new();
    let root = sandbox.work().join(std::ffi::OsStr::from_bytes(b"ws\xff"));
    sandbox.workspace(&root).repo("app").build();

    let (succeeded, envelope) = previewed(&sandbox, &root, "app");

    assert!(succeeded, "{envelope}");
    let repo = &envelope["results"][0]["data"]["repos"][1];
    let path = |field: &str| {
        meta_plugin::wire_path::from_bytes(repo[field]["bytes"].as_str().unwrap()).unwrap()
    };
    assert_eq!(path("source"), root.join("app"));
    assert_eq!(path("path"), root.join(".worktrees/s/app"));
}
