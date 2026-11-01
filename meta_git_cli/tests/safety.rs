//! Commands that delete or report must check what they act on.

use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

/// Run a Git fixture command and surface diagnostics on failure.
fn git(path: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A bare repo at `path` with one commit on `main`.
fn upstream(path: &Path) {
    let seed = path.with_extension("seed");
    fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "-q", "-b", "main"]);
    fs::write(seed.join("README"), "hello\n").unwrap();
    git(&seed, &["add", "README"]);
    git(&seed, &["commit", "-qm", "init"]);
    git(
        &seed,
        &["clone", "-q", "--bare", ".", path.to_str().unwrap()],
    );
}

/// A meta project at `root/ws` (itself a repo) with the given projects cloned.
fn workspace(root: &Path, projects: &[&str]) -> std::path::PathBuf {
    let ws = root.join("ws");
    fs::create_dir_all(&ws).unwrap();
    git(&ws, &["init", "-q", "-b", "main"]);
    let mut config = serde_json::Map::new();
    for name in projects {
        let url = root.join("up").join(format!("{name}.git"));
        upstream(&url);
        git(&ws, &["clone", "-q", url.to_str().unwrap(), name]);
        config.insert(name.to_string(), json!(url));
    }
    fs::write(
        ws.join(".meta.yaml"),
        json!({ "projects": config }).to_string(),
    )
    .unwrap();
    fs::write(
        ws.join(".gitignore"),
        format!("{}\n.worktrees/\n", projects.join("\n")),
    )
    .unwrap();
    git(&ws, &["add", "."]);
    git(&ws, &["commit", "-qm", "meta"]);
    ws
}

/// Invoke the plugin protocol in `cwd` with an isolated meta data dir under `root/data`.
fn meta_git(root: &Path, cwd: &Path, command: &str, args: &[&str], options: Value) -> Output {
    meta_git_in(root, cwd, command, args, options, None)
}

/// [`meta_git`], with `META_WORKTREES` set to `worktrees` if given.
fn meta_git_in(
    root: &Path,
    cwd: &Path,
    command: &str,
    args: &[&str],
    options: Value,
    worktrees: Option<&Path>,
) -> Output {
    let request = json!({"command": command, "args": args, "cwd": cwd, "options": options});
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_meta-git"));
    if root.join("bin").is_dir() {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let paths = std::iter::once(root.join("bin")).chain(std::env::split_paths(&path));
        cmd.env("PATH", std::env::join_paths(paths).unwrap());
    }
    let mut child = cmd
        .arg("--meta-plugin-exec")
        .current_dir(cwd)
        .env("META_DATA_DIR", root.join("data"))
        .env_remove("META_WORKTREES")
        .envs(worktrees.map(|dir| ("META_WORKTREES", dir)))
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

fn worktree(root: &Path, ws: &Path, args: &[&str]) -> Output {
    let (sub, rest) = args.split_first().unwrap();
    meta_git(root, ws, &format!("git worktree {sub}"), rest, json!({}))
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn unreadable_members_are_unknown_in_status_and_list() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["r1", "r2"]);
    let out = worktree(root, &ws, &["create", "w", "--all"]);
    assert!(out.status.success(), "{}", stderr(&out));
    fs::write(
        ws.join(".worktrees/w/r1/.git"),
        format!("gitdir: {}\n", root.join("gone").display()),
    )
    .unwrap();

    let mut observations = Vec::new();
    for (command, args) in [("status", vec!["w"]), ("list", vec![])] {
        let out = meta_git(
            root,
            &ws,
            &format!("git worktree {command}"),
            &args,
            json!({"json_output": true}),
        );
        assert!(out.status.success(), "{}", stderr(&out));
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        let repos = if command == "status" {
            &value["repos"]
        } else {
            &value["worktrees"][0]["repos"]
        };
        let repos = repos.as_array().unwrap();
        let healthy = repos.iter().find(|r| r["alias"] == "r2").unwrap();
        assert_eq!(healthy["dirty"], false, "{command}: {value}");
        let broken = repos.iter().find(|r| r["alias"] == "r1").unwrap();
        observations.push((command, broken.clone()));
    }
    assert!(
        observations.iter().all(|(_, repo)| {
            repo.get("dirty") == Some(&Value::Null) && repo.get("branch") == Some(&Value::Null)
        }),
        "{observations:?}"
    );
}

#[test]
fn status_names_a_missing_member_and_fails() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["r1", "r2"]);
    let out = worktree(root, &ws, &["create", "w", "--all"]);
    assert!(out.status.success(), "{}", stderr(&out));
    fs::remove_dir_all(ws.join(".worktrees/w/r2")).unwrap();

    let out = meta_git(
        root,
        &ws,
        "git worktree status",
        &["w"],
        json!({"json_output": true}),
    );
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["missing"], json!(["r2"]), "{value}");
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));

    let out = worktree(root, &ws, &["status", "w"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.lines()
            .any(|line| line.contains("r2") && line.contains("missing")),
        "{text}"
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
}

#[test]
fn text_list_warns_about_an_unreadable_set_and_lists_healthy_sets() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["r1"]);
    for name in ["broken", "healthy"] {
        let out = worktree(root, &ws, &["create", name, "--all"]);
        assert!(out.status.success(), "{}", stderr(&out));
    }
    fs::write(ws.join(".worktrees/broken/.git"), "garbage\n").unwrap();

    let out = worktree(root, &ws, &["list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.lines().any(|line| line.contains("healthy")), "{text}");
    let error = stderr(&out);
    assert!(
        error
            .lines()
            .any(|line| { line.starts_with("Warning: broken:") && line.contains("git") }),
        "{error}"
    );
}

#[test]
fn create_all_warns_about_broken_git_entries_and_creates_healthy_members() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["r1", "r2", "r3"]);
    fs::remove_dir_all(ws.join("r1/.git")).unwrap();
    fs::write(ws.join("r1/.git"), "gitdir: /nonexistent\n").unwrap();
    fs::remove_dir_all(ws.join("r2/.git")).unwrap();
    fs::create_dir(ws.join("r2/.git")).unwrap();

    let out = worktree(root, &ws, &["create", "w", "--all"]);
    let error = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "{error}");
    for name in ["r1", "r2"] {
        assert!(
            error
                .lines()
                .any(|line| { line.contains(name) && line.contains("not a git repository") }),
            "{error}"
        );
        assert!(!ws.join(".worktrees/w").join(name).exists());
    }
    assert!(ws.join(".worktrees/w/r3/.git").is_file());
    git(
        &ws.join(".worktrees/w/r3"),
        &["rev-parse", "--verify", "HEAD"],
    );
}

#[test]
fn diff_reports_a_missing_base_and_keeps_other_repos_including_binary_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a", "b"]);
    git(&ws.join("a"), &["branch", "only-a"]);
    let out = worktree(
        root,
        &ws,
        &["create", "t", "--repo", "a", "--repo", "b", "--no-deps"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let a = ws.join(".worktrees/t/a");
    fs::write(a.join("added"), "new line\n").unwrap();
    fs::write(a.join("binary"), b"\0\xff\x01").unwrap();
    git(&a, &["add", "added", "binary"]);
    git(&a, &["commit", "-qm", "change"]);

    let out = meta_git(
        root,
        &ws,
        "git worktree diff",
        &["t", "--base", "only-a"],
        json!({"json_output": true}),
    );
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    let repos = value["repos"].as_array().unwrap();
    assert_eq!(repos.len(), 2, "{value}");
    let good = repos.iter().find(|r| r["alias"] == "a").unwrap();
    let bad = repos.iter().find(|r| r["alias"] == "b").unwrap();
    assert!(good.get("error").is_none(), "{value}");
    assert_eq!(good["files_changed"], 2, "{value}");
    assert_eq!(good["insertions"], 1, "{value}");
    assert_eq!(good["deletions"], 0, "{value}");
    assert!(good["files"].as_array().unwrap().contains(&json!("binary")));
    assert!(bad["error"].as_str().unwrap().contains("only-a"), "{value}");
    assert!(bad["files_changed"].is_null(), "{value}");
    assert_eq!(value["totals"]["repos_changed"], 1, "{value}");
    assert_eq!(value["totals"]["files_changed"], 2, "{value}");
    let out = worktree(root, &ws, &["diff", "t", "--base", "only-a"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.lines()
            .any(|line| line.contains('a') && line.contains("+1")),
        "{text}"
    );
    assert!(
        text.lines()
            .any(|line| line.contains('b') && line.contains("only-a")),
        "{text}"
    );
    assert!(!text.contains("No changes"), "{text}");

    // No observed changes still doesn't mean clean when one repo failed.
    git(&a, &["branch", "-f", "only-a", "HEAD"]);
    let out = worktree(root, &ws, &["diff", "t", "--base", "only-a"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("No changes"));
    git(&ws.join("b"), &["branch", "only-a"]);
    let out = worktree(root, &ws, &["diff", "t", "--base", "only-a"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("No changes"));
}

#[cfg(unix)]
#[test]
fn diff_reports_failed_commands_and_malformed_numstat_instead_of_zero_counts() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);
    let out = worktree(root, &ws, &["create", "t", "--repo", "a", "--no-deps"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let path = std::env::var("PATH").unwrap();
    let quote = meta_core::display::shell_quote;
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    for numstat in [
        Some("bad\t0\tREADME"),
        Some("1\tbad\tREADME"),
        Some("missing-fields"),
        Some("999999999999999999999999\t0\tREADME"),
        None,
    ] {
        let answer = numstat.map_or_else(
            || "echo 'diff failed' >&2; exit 42".to_string(),
            |text| format!("printf '%s\\n' {}", quote(text)),
        );
        fs::write(bin.join("git"), format!(
            "#!/bin/sh\nif [ \"$1\" = diff ]; then\n  {answer}\nelse\n  PATH={} exec git \"$@\"\nfi\n",
            quote(&path)
        )).unwrap();
        fs::set_permissions(bin.join("git"), fs::Permissions::from_mode(0o700)).unwrap();
        let out = meta_git(
            root,
            &ws,
            "git worktree diff",
            &["t", "--base", "main"],
            json!({"json_output": true}),
        );
        assert_eq!(out.status.code(), Some(1), "{numstat:?}: {}", stderr(&out));
        let value: Value = serde_json::from_slice(&out.stdout).unwrap();
        let error = value["repos"][0]["error"].as_str().unwrap();
        assert!(error.contains("numstat"), "{value}");
        assert!(error.contains(".worktrees/t/a"), "{value}");
        if numstat.is_none() {
            assert!(error.contains("diff failed"), "{value}");
        }
        assert!(value["repos"][0]["files_changed"].is_null(), "{value}");
    }
}

#[test]
fn remove_refuses_directories_meta_did_not_create() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);

    // A plain directory where a worktree would be.
    fs::create_dir_all(ws.join(".worktrees/notawt")).unwrap();
    fs::write(ws.join(".worktrees/notawt/data"), "keep").unwrap();
    let out = worktree(root, &ws, &["remove", "notawt", "--force"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("no record"), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("holds no worktrees"),
        "{}",
        stderr(&out)
    );
    assert!(ws.join(".worktrees/notawt/data").exists());

    // A worktree made with git, not meta, carries no record of meta making it.
    git(
        &ws.join("a"),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "byhand",
            "../.worktrees/byhand",
        ],
    );
    let out = worktree(root, &ws, &["remove", "byhand", "--force"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("no record"), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("git worktree remove"),
        "{}",
        stderr(&out)
    );
    assert!(ws.join(".worktrees/byhand/README").exists());

    // worktrees_dir "." would make the primary clone `a` look like a worktree.
    // Such a config is refused, so the primary checkout is never deleted.
    let config = json!({"worktrees_dir": ".", "projects": {"a": "unused"}});
    fs::write(ws.join(".meta.yaml"), config.to_string()).unwrap();
    let out = worktree(root, &ws, &["remove", "a", "--force"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("Invalid worktrees_dir"),
        "{}",
        stderr(&out)
    );
    assert!(ws.join("a/README").exists());

    // META_WORKTREES may point anywhere, the workspace root included: there
    // the primary clone `a` looks like a set. The removal check refuses a
    // primary checkout.
    fs::write(
        ws.join(".meta.yaml"),
        json!({"projects": {"a": "unused"}}).to_string(),
    )
    .unwrap();
    let out = meta_git_in(
        root,
        &ws,
        "git worktree remove",
        &["a", "--force"],
        json!({}),
        Some(&ws),
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("not a linked worktree"),
        "{}",
        stderr(&out)
    );
    assert!(ws.join("a/README").exists());
}

#[test]
fn remove_checks_repos_nested_in_other_repos() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // ws -> sub (a nested meta repo) -> leaf
    let up = root.join("up");
    for name in ["sub", "leaf"] {
        upstream(&up.join(format!("{name}.git")));
    }
    let ws = workspace(root, &[]);
    git(
        &ws,
        &["clone", "-q", up.join("sub.git").to_str().unwrap(), "sub"],
    );
    let sub = ws.join("sub");
    git(
        &sub,
        &["clone", "-q", up.join("leaf.git").to_str().unwrap(), "leaf"],
    );
    let leaf_url = up.join("leaf.git");
    fs::write(
        sub.join(".meta.yaml"),
        json!({"projects": {"leaf": leaf_url}}).to_string(),
    )
    .unwrap();
    fs::write(sub.join(".gitignore"), "leaf/\n").unwrap();
    git(&sub, &["add", "."]);
    git(&sub, &["commit", "-qm", "meta"]);
    let config = json!({"projects": {"sub": {"repo": up.join("sub.git"), "meta": true}}});
    fs::write(ws.join(".meta.yaml"), config.to_string()).unwrap();
    fs::write(ws.join(".gitignore"), "sub/\n.worktrees/\n").unwrap();
    git(&ws, &["commit", "-qam", "sub"]);

    let out = meta_git(
        root,
        &ws,
        "git worktree create",
        &["t1", "--repo", "sub/leaf"],
        json!({"recursive": true}),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let work = ws.join(".worktrees/t1/sub/leaf/NEW_WORK.txt");
    fs::write(&work, "precious").unwrap();

    let out = worktree(root, &ws, &["remove", "t1"]);
    assert!(!out.status.success());
    let message = stderr(&out).replace('\\', "/");
    assert!(
        message.contains("sub/leaf: 1 file untracked: NEW_WORK.txt"),
        "{message}"
    );
    assert!(work.exists());

    // --force discards the work, but never deletes what git still has registered.
    let leaf_wt = ws.join(".worktrees/t1/sub/leaf");
    git(
        &sub.join("leaf"),
        &["worktree", "lock", leaf_wt.to_str().unwrap()],
    );
    let out = worktree(root, &ws, &["remove", "t1", "--force"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("locked"), "{}", stderr(&out));
    assert!(work.exists());
    git(
        &sub.join("leaf"),
        &["worktree", "unlock", leaf_wt.to_str().unwrap()],
    );

    fs::remove_file(&work).unwrap();
    let out = worktree(root, &ws, &["remove", "t1"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!ws.join(".worktrees/t1").exists());
    // The nested repo's worktree was unregistered, not just deleted.
    let list = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(sub.join("leaf"))
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&list.stdout)
            .matches("worktree ")
            .count(),
        1
    );
}

// A read-only directory needs Unix permissions.
#[cfg(unix)]
#[test]
fn prune_keeps_worktrees_that_hold_work() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);

    // A root-only worktree is not "orphaned": "." is never in the project list.
    let out = worktree(
        root,
        &ws,
        &["create", "rootonly", "--repo", ".", "--no-deps"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    fs::write(ws.join(".worktrees/rootonly/NOTES.md"), "notes").unwrap();
    // An expired worktree with uncommitted work is kept too.
    let out = worktree(
        root,
        &ws,
        &[
            "create",
            "expired",
            "--repo",
            "a",
            "--no-deps",
            "--ttl",
            "0",
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    fs::write(ws.join(".worktrees/expired/a/README"), "edited").unwrap();
    // Expired clean ones are pruned, unless git has one locked (kept) or
    // removing one fails.
    for name in ["clean", "locked", "stuck"] {
        let out = worktree(
            root,
            &ws,
            &["create", name, "--repo", "a", "--no-deps", "--ttl", "0"],
        );
        assert!(out.status.success(), "{}", stderr(&out));
    }
    let locked = ws.join(".worktrees/locked/a");
    git(
        &ws.join("a"),
        &["worktree", "lock", locked.to_str().unwrap()],
    );
    // git can't delete the files of a checkout whose directory is read-only
    let stuck = ws.join(".worktrees/stuck/a");
    let mode = |m| {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&stuck, fs::Permissions::from_mode(m)).unwrap()
    };
    mode(0o555);

    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"json_output": true}),
    );
    mode(0o755);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("Failed to prune: stuck"),
        "{}",
        stderr(&out)
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let names = |key: &str| -> Vec<String> {
        let mut names: Vec<String> = report[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_string())
            .collect();
        names.sort();
        names
    };
    assert_eq!(names("removed"), ["clean"]);
    assert_eq!(names("kept"), ["expired", "locked", "stuck"]);
    assert!(report.to_string().contains("git has it locked"), "{report}");
    assert!(locked.exists());
    assert!(ws.join(".worktrees/rootonly/NOTES.md").exists());
    assert!(ws.join(".worktrees/expired/a/README").exists());
    assert!(!ws.join(".worktrees/clean").exists());
}

#[cfg(unix)]
#[test]
fn hook_with_large_stderr_output_does_not_hang_create() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);
    // More than any pipe buffer (64 KiB by default, 1 MiB at most).
    let hook = "head -c 4000000 /dev/zero >&2";
    let config = json!({"projects": {"a": "unused"}, "worktree": {"hooks": {"post-create": hook}}});
    fs::write(ws.join(".meta.yaml"), config.to_string()).unwrap();

    let (tx, rx) = std::sync::mpsc::channel();
    let (root_path, ws_path) = (root.to_path_buf(), ws.clone());
    std::thread::spawn(move || {
        let _ = tx.send(worktree(
            &root_path,
            &ws_path,
            &["create", "w", "--repo", "a", "--no-deps"],
        ));
    });
    let out = rx
        .recv_timeout(std::time::Duration::from_secs(120))
        .expect("worktree create hung on a hook's stderr output");
    let head = out.stderr.get(..200).unwrap_or(&out.stderr);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(head));
    assert!(out.stderr.len() >= 4_000_000);
}

#[test]
fn a_failing_hook_is_a_warning_naming_it_and_an_error_under_strict() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);
    let hooks = json!({"post-create": "exit 3", "post-destroy": "exit 4"});
    let config = json!({"projects": {"a": "unused"}, "worktree": {"hooks": hooks}});
    fs::write(ws.join(".meta.yaml"), config.to_string()).unwrap();
    let run = |args: &[&str], strict: bool| {
        let (sub, rest) = args.split_first().unwrap();
        let command = format!("git worktree {sub}");
        meta_git(root, &ws, &command, rest, json!({"strict": strict}))
    };

    let out = run(&["create", "w", "--repo", "a", "--no-deps"], false);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("post-create hook `exit 3` exited with code 3"),
        "{}",
        stderr(&out)
    );
    let out = run(&["remove", "w"], true);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("code 4"), "{}", stderr(&out));
    // what the command did is done, and said, before the hook
    assert!(!ws.join(".worktrees/w").exists());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Removed worktree"));

    let out = run(&["create", "s", "--repo", "a", "--no-deps"], true);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("code 3"), "{}", stderr(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Created worktree"));
    assert!(ws.join(".worktrees/s/a").exists());
}

#[test]
fn a_corrupt_record_is_an_error_naming_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);
    // with the root, whose checkout holds the set's config
    let out = worktree(root, &ws, &["create", "w", "--repo", "a"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let record = ws.join(".git/worktrees/w/meta-worktree.json");
    fs::write(&record, "{not json").unwrap();

    // inside the set, meta can't tell which checkout it stands for
    let set = ws.join(".worktrees/w");
    let out = worktree(root, &set, &["create", "x", "--repo", "a", "--no-deps"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("Cannot parse") && stderr(&out).contains("meta-worktree.json"),
        "{}",
        stderr(&out)
    );
    assert!(!set.join(".worktrees").exists());
}

#[test]
fn a_name_with_a_slash_names_its_project_not_a_place() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["actual-api", "b"]);
    let url = |name: &str| root.join("up").join(format!("{name}.git"));
    let config = json!({"projects": {
        "group/api": {"repo": url("actual-api"), "path": "actual-api"},
        "b": url("b"),
    }});
    fs::write(ws.join(".meta.yaml"), config.to_string()).unwrap();

    for (set, args) in [("c", &["--no-deps"][..]), ("d", &[]), ("r", &["-r"])] {
        let mut create = vec!["create", set, "--repo", "group/api"];
        create.extend_from_slice(args);
        let out = worktree(root, &ws, &create);
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        assert!(ws
            .join(".worktrees")
            .join(set)
            .join("actual-api/README")
            .exists());
    }
    let out = worktree(root, &ws, &["create", "e", "--repo", "b", "--no-deps"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = worktree(root, &ws, &["add", "e", "--repo", "group/api"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(ws.join(".worktrees/e/actual-api/README").exists());
}

#[test]
fn with_recursive_all_every_place_is_a_place_whatever_it_names() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["bar", "baz"]);
    let url = |name: &str| root.join("up").join(format!("{name}.git"));
    // the project named `bar` is at `baz`; another is at `bar`
    let config = json!({"projects": {
        "foo": {"repo": url("bar"), "path": "bar"},
        "bar": {"repo": url("baz"), "path": "baz"},
    }});
    fs::write(ws.join(".meta.yaml"), config.to_string()).unwrap();
    let out = worktree(root, &ws, &["create", "x", "--all", "-r"]);
    assert!(out.status.success(), "{}", stderr(&out));
    for place in ["bar", "baz"] {
        assert!(
            ws.join(".worktrees/x").join(place).join("README").exists(),
            "{place}"
        );
    }
}

#[test]
fn a_root_git_cannot_use_is_no_reason_to_fail_a_create_without_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);
    fs::remove_dir_all(ws.join(".git")).unwrap();
    fs::write(
        ws.join(".git"),
        format!("gitdir: {}\n", root.join("gone").display()),
    )
    .unwrap();
    let out = worktree(root, &ws, &["create", "n", "--repo", "a", "--no-deps"]);
    assert!(out.status.success(), "{}", stderr(&out));
    // --all names the root it skips
    let out = worktree(root, &ws, &["create", "l", "--all"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("'.' at"), "{}", stderr(&out));
    for set in ["n", "l"] {
        assert!(ws.join(".worktrees").join(set).join("a/README").exists());
    }
}

#[test]
fn an_ephemeral_exec_removes_the_set_it_made_wherever_the_config_moves_sets() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);
    // the command moves the workspace's worktrees elsewhere meanwhile
    let mut config: Value =
        serde_json::from_str(&fs::read_to_string(ws.join(".meta.yaml")).unwrap()).unwrap();
    config["worktrees_dir"] = json!("elsewhere");
    fs::write(root.join("moved.yaml"), config.to_string()).unwrap();
    let command = format!(
        "cp '{}' '{}'",
        root.join("moved.yaml").display(),
        ws.join(".meta.yaml").display()
    );
    let out = worktree(
        root,
        &ws,
        &[
            "exec",
            "eph",
            "--ephemeral",
            "--repo",
            "a",
            "--",
            "sh",
            "-c",
            &command,
        ],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!ws.join(".worktrees/eph").exists(), "{}", stderr(&out));
}

#[test]
fn a_commit_ish_no_repo_has_creates_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);

    let out = worktree(
        root,
        &ws,
        &["create", "r1", "--repo", "a", "--no-deps", "no-such-ref"],
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("no worktree was created"),
        "{}",
        stderr(&out)
    );
    assert!(!ws.join(".worktrees/r1").exists());
}

#[cfg(unix)]
#[test]
fn from_pr_applies_only_to_the_exact_repo() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // Remotes end in org/api and org/api-legacy, as GitHub URLs would.
    let up = root.join("up/org");
    let ws = root.join("ws");
    fs::create_dir_all(&ws).unwrap();
    git(&ws, &["init", "-q", "-b", "main"]);
    for (alias, repo) in [("legacy", "api-legacy"), ("service", "api")] {
        let url = up.join(format!("{repo}.git"));
        upstream(&url);
        git(&ws, &["clone", "-q", url.to_str().unwrap(), alias]);
    }
    git(
        &ws.join("service"),
        &["push", "-q", "origin", "main:feature-x"],
    );
    let config =
        json!({"projects": {"legacy": up.join("api-legacy.git"), "service": up.join("api.git")}});
    fs::write(ws.join(".meta.yaml"), config.to_string()).unwrap();
    fs::write(ws.join(".gitignore"), "legacy\nservice\n.worktrees/\n").unwrap();
    git(&ws, &["add", "."]);
    git(&ws, &["commit", "-qm", "meta"]);
    // A stand-in `gh` that reports the PR's head branch.
    fs::create_dir(root.join("bin")).unwrap();
    fs::write(root.join("bin/gh"), "#!/bin/sh\necho feature-x\n").unwrap();
    fs::set_permissions(root.join("bin/gh"), fs::Permissions::from_mode(0o755)).unwrap();

    let out = meta_git(
        root,
        &ws,
        "git worktree create",
        &["pr1", "--from-pr", "org/api#5", "--all"],
        json!({"json_output": true}),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let created: Value = serde_json::from_slice(&out.stdout).unwrap();
    let branch_of = |alias: &str| {
        created["repos"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["alias"] == alias)
            .map(|r| r["branch"].as_str().unwrap().to_string())
    };
    assert_eq!(branch_of("service").as_deref(), Some("feature-x"));
    assert_eq!(branch_of("legacy").as_deref(), Some("pr1"));

    // A PR whose repo isn't selected is an error, not a silent no-op.
    let out = worktree(
        root,
        &ws,
        &[
            "create",
            "pr2",
            "--from-pr",
            "org/api#5",
            "--repo",
            "legacy",
            "--no-deps",
        ],
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("No selected repo has origin 'org/api'"),
        "{}",
        stderr(&out)
    );
    assert!(!ws.join(".worktrees/pr2").exists());
}

#[test]
fn update_reports_failed_clones_and_exits_nonzero() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = root.join("ws");
    fs::create_dir_all(&ws).unwrap();
    upstream(&root.join("good.git"));
    let config = json!({"projects": {
        "good": root.join("good.git"),
        "missing": root.join("does-not-exist.git"),
    }});
    fs::write(ws.join(".meta.yaml"), config.to_string()).unwrap();

    let out = meta_git(root, &ws, "git update", &[], json!({}));
    assert!(!out.status.success());
    let message = stderr(&out);
    assert!(
        message.contains("1 of 2 repositories failed to clone: missing"),
        "{message}"
    );
    assert!(message.contains("does-not-exist.git"), "{message}");
    assert!(ws.join("good/README").exists());
}

#[test]
fn failed_create_rolls_back_what_it_created() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a", "b"]);
    // a only has `origin/t`, so creating `t` makes a local tracking branch.
    git(&ws.join("a"), &["push", "-q", "origin", "main:t"]);
    git(&ws.join("a"), &["fetch", "-q"]);
    // b's branch `t` is checked out elsewhere, so creating `t` fails at b.
    git(&ws.join("b"), &["branch", "t"]);
    let elsewhere = root.join("elsewhere");
    git(
        &ws.join("b"),
        &["worktree", "add", "-q", elsewhere.to_str().unwrap(), "t"],
    );

    let args = ["create", "t", "--repo", "a", "--repo", "b", "--no-deps"];
    let out = worktree(root, &ws, &args);
    assert!(!out.status.success());
    assert!(!ws.join(".worktrees/t").exists(), "{}", stderr(&out));
    let list = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(ws.join("a"))
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&list.stdout)
            .matches("worktree ")
            .count(),
        1
    );

    // The branch this call created in a is gone too.
    let branch = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/t"])
        .current_dir(ws.join("a"))
        .output()
        .unwrap();
    assert!(!branch.status.success());

    // Nothing is stranded: the same command from a ref can be run again.
    git(&ws.join("a"), &["tag", "v1"]);
    let args = [
        "create",
        "t",
        "v1",
        "--repo",
        "a",
        "--repo",
        "b",
        "--no-deps",
        "--strict",
    ];
    let out = worktree(root, &ws, &args);
    assert!(!out.status.success(), "b has no v1, so --strict fails");
    assert!(!ws.join(".worktrees/t").exists(), "{}", stderr(&out));
    let out = worktree(
        root,
        &ws,
        &["create", "t", "v1", "--repo", "a", "--no-deps"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn commit_ish_create_fails_on_anything_but_a_missing_ref() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a", "b"]);
    git(&ws.join("a"), &["tag", "v1"]);
    git(&ws.join("b"), &["tag", "v1"]);
    // b has the ref, but its branch `t` already exists, so `-b t` fails there.
    git(&ws.join("b"), &["branch", "t"]);

    let args = [
        "create",
        "t",
        "v1",
        "--repo",
        "a",
        "--repo",
        "b",
        "--no-deps",
    ];
    let out = worktree(root, &ws, &args);
    assert!(
        !out.status.success(),
        "a real failure is not a skipped repo"
    );
    assert!(!ws.join(".worktrees/t").exists(), "{}", stderr(&out));
    let branch = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/t"])
        .current_dir(ws.join("a"))
        .output()
        .unwrap();
    assert!(!branch.status.success(), "a's new branch was rolled back");
}

#[test]
fn commit_ish_create_fails_when_a_repo_cannot_be_read() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a", "b"]);
    git(&ws.join("a"), &["tag", "v1"]);
    git(&ws.join("b"), &["tag", "v1"]);
    let config = ws.join("b/.git/config");
    let mut broken = fs::read_to_string(&config).unwrap();
    broken.push_str("[broken\n");
    fs::write(&config, broken).unwrap();

    let args = [
        "create",
        "t",
        "v1",
        "--repo",
        "a",
        "--repo",
        "b",
        "--no-deps",
    ];
    let out = worktree(root, &ws, &args);
    assert!(
        !out.status.success(),
        "an unreadable repo is not a missing ref"
    );
    assert!(stderr(&out).contains("config"), "{}", stderr(&out));
    assert!(!ws.join(".worktrees/t").exists());
}

#[test]
fn a_set_holding_another_recorded_set_is_not_deleted() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);
    let out = worktree(root, &ws, &["create", "outer", "--repo", ".", "--no-deps"]);
    assert!(out.status.success(), "{}", stderr(&out));
    // A set placed inside the outer one (from inside a set, sets go where
    // the workspace keeps them, so the place is given)
    let outer = ws.join(".worktrees/outer");
    let out = meta_git_in(
        root,
        &outer,
        "git worktree create",
        &["inner", "--repo", ".", "--no-deps"],
        json!({}),
        Some(&outer.join(".worktrees")),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(outer.join(".worktrees/inner").exists());

    let out = worktree(root, &ws, &["remove", "outer", "--force"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("another worktree set"),
        "{}",
        stderr(&out)
    );
    assert!(outer.join(".worktrees/inner/.git").exists());
}

#[cfg(unix)]
#[test]
fn prune_leaves_a_dangling_link_at_a_set_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);
    let out = worktree(
        root,
        &ws,
        &["create", "gone", "--repo", "a", "--no-deps", "--ttl", "0"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    // Replace the set with a link to storage that has since disappeared.
    let set = ws.join(".worktrees/gone");
    let check = std::fs::canonicalize(&set).unwrap();
    fs::remove_dir_all(&set).unwrap();
    std::os::unix::fs::symlink(root.join("vanished"), &set).unwrap();
    assert_eq!(check, ws.canonicalize().unwrap().join(".worktrees/gone"));

    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"json_output": true}),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    // The record still names the path, but prune neither follows the link
    // nor takes the link for a missing set: it deletes and forgets nothing
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["removed"], json!([]), "{report}");
    assert!(set.symlink_metadata().is_ok());
    assert!(ws.join("a/.git/worktrees/a/meta-worktree.json").exists());
}

#[cfg(unix)]
#[test]
fn remove_works_when_the_worktrees_dir_is_a_link() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);
    // worktrees_dir must stay inside the workspace; a link there still
    // resolves to another path than the one written
    fs::create_dir(ws.join("real")).unwrap();
    std::os::unix::fs::symlink(ws.join("real"), ws.join("link")).unwrap();
    let config = json!({"worktrees_dir": "link", "projects": {"a": "unused"}});
    fs::write(ws.join(".meta.yaml"), config.to_string()).unwrap();

    let out = worktree(root, &ws, &["create", "t", "--repo", "a", "--no-deps"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = worktree(root, &ws, &["remove", "t"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!ws.join("link/t").exists());
    // git deleted the worktree's admin directory, and its record with it
    let admins = fs::read_dir(ws.join("a/.git/worktrees")).map_or(0, |d| d.count());
    assert_eq!(admins, 0);
}

#[cfg(unix)]
#[test]
fn prune_keeps_a_worktree_it_cannot_inspect() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);
    let out = worktree(
        root,
        &ws,
        &["create", "x", "--repo", "a", "--no-deps", "--ttl", "0"],
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let parent = ws.join(".worktrees");
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&parent).is_ok() {
        // Running as root: permissions don't apply.
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }

    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"json_output": true}),
    );
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["removed"], json!([]), "{report}");
    let kept: Vec<&str> = report["kept"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    // The set, found through its repository (prune doesn't list the root)
    assert_eq!(kept, ["x"], "{report}");
    assert!(ws.join(".worktrees/x/a/README").exists());
}

#[cfg(unix)]
#[test]
fn rollback_keeps_a_branch_a_checkout_hook_committed_on() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a", "b"]);
    // Use this repo's hooks dir whatever the global git config says.
    git(&ws.join("a"), &["config", "core.hooksPath", ".git/hooks"]);
    fs::create_dir_all(ws.join("a/.git/hooks")).unwrap();
    let hook = ws.join("a/.git/hooks/post-checkout");
    fs::write(
        &hook,
        "#!/bin/sh\ngit -c user.name=h -c user.email=h@example.com commit -q --allow-empty -m hook\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    git(&ws.join("b"), &["branch", "t"]);
    let elsewhere = root.join("elsewhere");
    git(
        &ws.join("b"),
        &["worktree", "add", "-q", elsewhere.to_str().unwrap(), "t"],
    );

    let args = ["create", "t", "--repo", "a", "--repo", "b", "--no-deps"];
    let out = worktree(root, &ws, &args);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("kept branches"), "{}", stderr(&out));
    // The hook's commit is still on a's branch t.
    let log = Command::new("git")
        .args(["log", "-1", "--format=%s", "refs/heads/t"])
        .current_dir(ws.join("a"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "hook");
}

#[test]
fn records_live_beside_git_and_no_store_is_written() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a", "b"]);
    let args = ["create", "t", "--repo", "a", "--no-deps", "--ttl", "1h"];
    let out = meta_git(
        root,
        &ws,
        "git worktree create",
        &[&args[1..], &["--meta", "k=v"]].concat(),
        json!({}),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!root.join("data/worktree.json").exists());
    let read = |repo: &str| -> Value {
        let path = ws
            .join(repo)
            .join(".git/worktrees")
            .join(repo)
            .join("meta-worktree.json");
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    };
    let record = read("a");
    let set = ws.join(".worktrees/t").canonicalize().unwrap();
    assert_eq!(record["set"], json!(set), "{record}");
    assert_eq!(record["ttl_seconds"], 3600, "{record}");
    assert_eq!(record["custom"], json!({"k": "v"}), "{record}");

    // A repo added later joins the same record
    let out = worktree(root, &ws, &["add", "t", "--repo", "b"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(read("b"), record);

    let out = meta_git(
        root,
        &ws,
        "git worktree list",
        &[],
        json!({"json_output": true}),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let list: Value = serde_json::from_slice(&out.stdout).unwrap();
    let entry = &list["worktrees"][0];
    assert_eq!(entry["custom"], json!({"k": "v"}), "{list}");
    assert!(
        entry["ttl_remaining_seconds"].as_i64().unwrap() > 3000,
        "{list}"
    );

    // A set whose directory is gone loses only meta's records; git's own
    // entries stay for `git worktree prune`
    fs::remove_dir_all(ws.join(".worktrees/t")).unwrap();
    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"json_output": true}),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["removed"][0]["name"], "t", "{report}");
    assert_eq!(
        report["removed"][0]["reason"],
        "orphaned (missing directory)"
    );
    for repo in ["a", "b"] {
        let admin = ws.join(repo).join(".git/worktrees").join(repo);
        assert!(admin.is_dir() && !admin.join("meta-worktree.json").exists());
    }
    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"json_output": true}),
    );
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["removed"], json!([]), "{report}");
}

#[test]
fn prune_keeps_and_reports_a_record_it_cannot_read() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);
    let out = worktree(root, &ws, &["create", "t", "--repo", "a", "--no-deps"]);
    assert!(out.status.success(), "{}", stderr(&out));
    fs::write(ws.join("a/.git/worktrees/a/meta-worktree.json"), "not json").unwrap();

    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"json_output": true}),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let reason = report["kept"][0]["reason"].as_str().unwrap_or_default();
    assert!(reason.contains("can't be read"), "{report}");
    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"strict": true}),
    );
    assert!(!out.status.success(), "strict prune fails on a kept set");

    let out = worktree(root, &ws, &["remove", "t", "--force"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("can't be used"), "{}", stderr(&out));
    assert!(ws.join(".worktrees/t/a/README").exists());
}

#[test]
fn a_stale_record_does_not_expire_a_set_made_again_at_its_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a", "b"]);
    let elsewhere = root.join("elsewhere");
    let create = |repo: &str, ttl: &[&str]| {
        let args = [&["t", "--repo", repo, "--no-deps"][..], ttl].concat();
        meta_git_in(
            root,
            &ws,
            "git worktree create",
            &args,
            json!({}),
            Some(&elsewhere),
        )
    };
    // A set with a TTL that has run out, deleted by hand: its record stays
    // in a's git dir
    let out = create("a", &["--ttl", "0"]);
    assert!(out.status.success(), "{}", stderr(&out));
    fs::remove_dir_all(elsewhere.join("t")).unwrap();
    // The same path made again, from b, with no TTL
    let out = create("b", &[]);
    assert!(out.status.success(), "{}", stderr(&out));

    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"json_output": true}),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["removed"], json!([]), "{report}");
    assert!(elsewhere.join("t/b/README").exists());
}

#[cfg(unix)]
#[test]
fn prune_reports_a_repository_whose_worktrees_it_cannot_list() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);
    let elsewhere = root.join("elsewhere");
    let out = meta_git_in(
        root,
        &ws,
        "git worktree create",
        &["t", "--repo", "a", "--no-deps", "--ttl", "0"],
        json!({}),
        Some(&elsewhere),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let admins = ws.join("a/.git/worktrees");
    let mode = |m| fs::set_permissions(&admins, fs::Permissions::from_mode(m)).unwrap();
    mode(0o000);
    if fs::read_dir(&admins).is_ok() {
        // Running as root: permissions don't apply.
        mode(0o755);
        return;
    }
    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"json_output": true}),
    );
    let strict = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"strict": true}),
    );
    mode(0o755);
    assert!(out.status.success(), "{}", stderr(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["kept"][0]["name"], "worktrees", "{report}");
    assert!(
        !strict.status.success(),
        "strict prune fails when it can't see every set"
    );
    assert!(elsewhere.join("t/a/README").exists());
}

#[test]
fn prune_reports_repositories_it_could_not_search() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);
    let elsewhere = root.join("elsewhere");
    let out = meta_git_in(
        root,
        &ws,
        "git worktree create",
        &["t", "--repo", "a", "--no-deps", "--ttl", "0"],
        json!({}),
        Some(&elsewhere),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    // A nested meta project whose config can't be parsed stops the walk
    // over the workspace's repositories
    let config = json!({"projects": {"a": "unused", "g": {"repo": "unused", "meta": true}}});
    fs::write(ws.join(".meta.yaml"), config.to_string()).unwrap();
    fs::create_dir(ws.join("g")).unwrap();
    fs::write(ws.join("g/.meta.yaml"), "not json").unwrap();

    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"json_output": true}),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["kept"][0]["name"], "ws", "{report}");
    let strict = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"strict": true}),
    );
    assert!(
        !strict.status.success(),
        "strict prune fails when it can't see every set"
    );
    assert!(elsewhere.join("t/a/README").exists());
}

#[test]
fn prune_finds_sets_of_a_separate_git_dir_repository() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &[]);
    git(
        &ws,
        &[
            "init",
            "-q",
            "--separate-git-dir",
            root.join("sep.git").to_str().unwrap(),
            "s",
        ],
    );
    git(
        &ws.join("s"),
        &["commit", "-q", "--allow-empty", "-m", "init"],
    );
    fs::write(
        ws.join(".meta.yaml"),
        json!({"projects": {"s": "unused"}}).to_string(),
    )
    .unwrap();
    let elsewhere = root.join("elsewhere");
    let out = meta_git_in(
        root,
        &ws,
        "git worktree create",
        &["t", "--repo", "s", "--no-deps", "--ttl", "0"],
        json!({}),
        Some(&elsewhere),
    );
    assert!(out.status.success(), "{}", stderr(&out));

    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"json_output": true}),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["removed"][0]["name"], "t", "{report}");
    assert!(!elsewhere.join("t").exists());
}

#[test]
fn prune_leaves_a_moved_set_and_stale_records_alone() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a", "b"]);
    let elsewhere = root.join("elsewhere");
    let create = |name: &str, repo: &str| {
        let args = [name, "--repo", repo, "--no-deps", "--ttl", "0"];
        meta_git_in(
            root,
            &ws,
            "git worktree create",
            &args,
            json!({}),
            Some(&elsewhere),
        )
    };
    // A set moved after it was made, as ATC moves the sets it stages
    let out = create("staged", "a");
    assert!(out.status.success(), "{}", stderr(&out));
    fs::rename(elsewhere.join("staged"), elsewhere.join("installed")).unwrap();
    git(
        &ws.join("a"),
        &[
            "worktree",
            "repair",
            elsewhere.join("installed/a").to_str().unwrap(),
        ],
    );
    // A set deleted by hand, and the path reused for a worktree made with git
    let out = create("reused", "b");
    assert!(out.status.success(), "{}", stderr(&out));
    fs::remove_dir_all(elsewhere.join("reused")).unwrap();
    fs::create_dir(elsewhere.join("reused")).unwrap();
    git(
        &ws.join("a"),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "byhand",
            "../../elsewhere/reused/a",
        ],
    );

    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"strict": true, "json_output": true}),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["removed"], json!([]), "{report}");
    assert_eq!(report["kept"], json!([]), "{report}");
    // The moved set keeps its record, and says why remove refuses it
    assert!(elsewhere.join("installed/a/README").exists());
    assert!(ws.join("a/.git/worktrees/a/meta-worktree.json").exists());
    let out = meta_git_in(
        root,
        &ws,
        "git worktree remove",
        &["installed"],
        json!({}),
        Some(&elsewhere),
    );
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("was the set moved?"),
        "{}",
        stderr(&out)
    );
    assert!(elsewhere.join("reused/a/README").exists());
}

#[cfg(unix)]
#[test]
fn prune_reports_a_linked_checkout_whose_git_metadata_it_cannot_read() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &[]);
    // Project `a` is itself a linked worktree of a repository elsewhere
    let url = root.join("up/a.git");
    upstream(&url);
    git(root, &["clone", "-q", url.to_str().unwrap(), "main-a"]);
    git(
        &root.join("main-a"),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "ws-a",
            ws.join("a").to_str().unwrap(),
        ],
    );
    fs::write(
        ws.join(".meta.yaml"),
        json!({"projects": {"a": "unused"}}).to_string(),
    )
    .unwrap();
    let elsewhere = root.join("elsewhere");
    let out = meta_git_in(
        root,
        &ws,
        "git worktree create",
        &["t", "--repo", "a", "--no-deps", "--ttl", "0"],
        json!({}),
        Some(&elsewhere),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let commondir = root.join("main-a/.git/worktrees/a/commondir");
    let mode = |m| fs::set_permissions(&commondir, fs::Permissions::from_mode(m)).unwrap();
    mode(0o000);
    if fs::read_to_string(&commondir).is_ok() {
        // Running as root: permissions don't apply.
        mode(0o644);
        return;
    }
    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"json_output": true}),
    );
    let strict = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"strict": true}),
    );
    mode(0o644);
    assert!(out.status.success(), "{}", stderr(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["kept"][0]["name"], "a", "{report}");
    assert!(
        !strict.status.success(),
        "strict prune fails when it can't see every set"
    );
    assert!(elsewhere.join("t/a/README").exists());
}

#[test]
fn prune_finds_sets_of_a_bare_repository() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &[]);
    let url = root.join("up/b.git");
    upstream(&url);
    git(&ws, &["clone", "-q", "--bare", url.to_str().unwrap(), "b"]);
    fs::write(
        ws.join(".meta.yaml"),
        json!({"projects": {"b": "unused"}}).to_string(),
    )
    .unwrap();
    let elsewhere = root.join("elsewhere");
    let out = meta_git_in(
        root,
        &ws,
        "git worktree create",
        &["t", "--repo", "b", "--no-deps", "--ttl", "0"],
        json!({}),
        Some(&elsewhere),
    );
    assert!(out.status.success(), "{}", stderr(&out));

    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"json_output": true}),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["removed"][0]["name"], "t", "{report}");
    assert!(!elsewhere.join("t").exists());
}

#[test]
fn prune_leaves_a_gone_set_alone_while_one_of_its_worktrees_lives_on() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a", "b"]);
    let args = ["create", "t", "--repo", "a", "--repo", "b", "--no-deps"];
    let out = worktree(root, &ws, &args);
    assert!(out.status.success(), "{}", stderr(&out));
    // One worktree moved out of the set, then the set deleted by hand
    let moved = root.join("moved-a");
    git(
        &ws.join("a"),
        &[
            "worktree",
            "move",
            ws.join(".worktrees/t/a").to_str().unwrap(),
            moved.to_str().unwrap(),
        ],
    );
    fs::remove_dir_all(ws.join(".worktrees/t")).unwrap();

    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"json_output": true}),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["removed"], json!([]), "{report}");
    assert!(ws.join("a/.git/worktrees/a/meta-worktree.json").exists());
    assert!(moved.join("README").exists());
}

#[cfg(unix)]
#[test]
fn prune_finds_sets_of_a_repository_whose_git_dir_is_a_link() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["a"]);
    fs::rename(ws.join("a/.git"), root.join("a.git")).unwrap();
    std::os::unix::fs::symlink(root.join("a.git"), ws.join("a/.git")).unwrap();
    let elsewhere = root.join("elsewhere");
    let out = meta_git_in(
        root,
        &ws,
        "git worktree create",
        &["t", "--repo", "a", "--no-deps", "--ttl", "0"],
        json!({}),
        Some(&elsewhere),
    );
    assert!(out.status.success(), "{}", stderr(&out));

    let out = meta_git(
        root,
        &ws,
        "git worktree prune",
        &[],
        json!({"strict": true, "json_output": true}),
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["removed"][0]["name"], "t", "{report}");
    assert_eq!(report["kept"], json!([]), "{report}");
    assert!(!elsewhere.join("t").exists());
}

#[test]
fn a_project_outside_the_workspace_gets_a_worktree_at_its_name() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ws = workspace(root, &["r1"]);
    let url = root.join("up/sib.git");
    upstream(&url);
    git(root, &["clone", "-q", url.to_str().unwrap(), "sib"]);
    fs::write(
        ws.join(".meta.yaml"),
        json!({"projects": {"r1": root.join("up/r1.git"), "sib": {"repo": url, "path": "../sib"}}})
            .to_string(),
    )
    .unwrap();

    let out = worktree(root, &ws, &["create", "w", "--all"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(ws.join(".worktrees/w/sib/.git").is_file());
    assert!(!ws.join(".worktrees/sib").exists());

    // its work is kept as any member's is
    fs::write(ws.join(".worktrees/w/sib/new.txt"), "work").unwrap();
    let out = worktree(root, &ws, &["remove", "w"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("sib"), "{}", stderr(&out));
    fs::remove_file(ws.join(".worktrees/w/sib/new.txt")).unwrap();
    let out = worktree(root, &ws, &["remove", "w"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!ws.join(".worktrees/w").exists());
}
