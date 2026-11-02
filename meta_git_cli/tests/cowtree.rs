//! Worktrees are made by git-cowtree when it is installed, else by git, as
//! git-cowtree's contract (schema `git-cowtree.v1`) says.
#![cfg(unix)] // the stand-in is a shell script; git-cowtree runs on Linux and macOS
#![expect(
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]

use meta_test_support::Sandbox;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Output, Stdio},
};

/// A workspace at `work/ws` (itself a repo) with projects `a`, `b` and `c`
/// cloned.
fn workspace(sandbox: &Sandbox) -> PathBuf {
    let root = sandbox.work();
    let ws = sandbox.repo(&root.join("ws"));
    let mut projects = serde_json::Map::new();
    for name in ["a", "b", "c"] {
        let url = sandbox.bare(&root.join("up").join(format!("{name}.git")));
        sandbox.git(&ws, &["clone", "-q", url.to_str().unwrap(), name]);
        projects.insert(name.to_string(), json!(url));
    }
    fs::write(
        ws.join(".meta.yaml"),
        json!({ "projects": projects }).to_string(),
    )
    .unwrap();
    fs::write(ws.join(".gitignore"), "a\nb\nc\n.worktrees/\n").unwrap();
    sandbox.git(&ws, &["add", "."]);
    sandbox.git(&ws, &["commit", "-qm", "meta"]);
    ws
}

/// A stand-in git-cowtree on the sandbox's PATH whose `version` names
/// `schema` and exits `version_exit`. Its `add` does in each repo what
/// `behave` set for it (the repo folder's name), `reflink` by default:
/// `reflink` or `git` makes the worktree with git and says that backend;
/// `hook` makes it and fails with `hook_failed`; `rollback_failed` makes it,
/// locks it as an interrupted add does, and fails naming it; any other mode
/// fails as the mode says, changing nothing. Every call is logged; returns
/// the log.
fn fake_cowtree(sandbox: &Sandbox, schema: &str, version_exit: u8) -> PathBuf {
    let log = sandbox.work().join("cowtree.log");
    let modes = sandbox.work().join("modes");
    fs::create_dir_all(&modes).unwrap();
    let script = format!(
        r#"#!/bin/sh
printf '%s\n' "$*" >> '{log}'
if [ "$2" = version ]; then echo '{{"schema":"{schema}","version":"0.1.0"}}'; exit {version_exit}; fi
shift 2
mode=$(cat '{modes}'/"$(basename "$PWD")" 2>/dev/null || echo reflink)
fail() {{ printf '{{"schema":"git-cowtree.v1","error":{{"kind":"%s","message":"%s","path":null}}}}\n' "$1" "$2"; exit "$3"; }}
case $mode in
  unsupported) fail unsupported 'no reflinks on this filesystem' 3 ;;
  git_failed) fail git_failed 'git said no' 5 ;;
  io_error_3) fail io_error 'odd exit for this kind' 3 ;;
  unsupported_5) fail unsupported 'odd exit for this kind' 5 ;;
esac
git worktree add -q "$@" >&2 || fail git_failed 'git refused' 5
[ "$mode" = hook ] && fail hook_failed 'the post-checkout hook failed' 5
if [ "$mode" = rollback_failed ]; then
  # the tests' adds are `-b <branch> <dest>`
  git worktree lock --reason "git-cowtree:$$" "$3"
  printf '{{"schema":"git-cowtree.v1","error":{{"kind":"rollback_failed","message":"could not remove it","path":"%s"}}}}\n' "$3"
  exit 5
fi
printf '{{"schema":"git-cowtree.v1","backend":"%s"}}\n' "$mode"
"#,
        log = log.display(),
        modes = modes.display(),
    );
    let path = sandbox.bin().join("git-cowtree");
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    log
}

/// What the stand-in's `add` does in `repo`.
fn behave(sandbox: &Sandbox, repo: &str, mode: &str) {
    fs::write(sandbox.work().join("modes").join(repo), mode).unwrap();
}

/// The stand-in's calls so far, one per line.
fn calls(log: &Path) -> Vec<String> {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

/// `meta git worktree <args>` in `ws` with `options`.
fn worktree(sandbox: &Sandbox, ws: &Path, args: &[&str], options: Value) -> Output {
    let (sub, rest) = args.split_first().unwrap();
    let request = json!({"command": format!("git worktree {sub}"), "args": rest,
        "cwd": ws, "options": options});
    let mut child = sandbox
        .command(env!("CARGO_BIN_EXE_meta-git"))
        .arg("--meta-plugin-exec")
        .current_dir(ws)
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

/// [`worktree`] with `--json`: its JSON, failing the test unless it succeeds.
fn worktree_json(sandbox: &Sandbox, ws: &Path, args: &[&str]) -> Value {
    let out = worktree(sandbox, ws, args, json!({"json_output": true}));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Each repo entry's (alias, backend, fallback_reason).
fn backends(output: &Value) -> Vec<(String, Value, Value)> {
    output["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let field = |name: &str| r.get(name).cloned().unwrap_or(Value::Null);
            (
                r["alias"].as_str().unwrap().to_string(),
                field("backend"),
                field("fallback_reason"),
            )
        })
        .collect()
}

fn has_branch(sandbox: &Sandbox, repo: &Path, branch: &str) -> bool {
    sandbox
        .command("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(repo)
        .output()
        .unwrap()
        .status
        .success()
}

fn worktree_count(sandbox: &Sandbox, repo: &Path) -> usize {
    let list = sandbox.git(repo, &["worktree", "list", "--porcelain"]);
    list.matches("worktree ").count()
}

#[test]
fn without_a_v1_git_cowtree_git_makes_every_worktree() {
    let sandbox = Sandbox::new();
    let ws = workspace(&sandbox);
    // the sandbox's stand-in for none installed, and none speaking v1
    let mut outputs = vec![worktree_json(
        &sandbox,
        &ws,
        &["create", "t", "--repo", "a"],
    )];
    // one that speaks another schema, and one whose `version` fails
    for (schema, exit, repo) in [("git-cowtree.v2", 0, "b"), ("git-cowtree.v1", 5, "c")] {
        let log = fake_cowtree(&sandbox, schema, exit);
        if log.exists() {
            fs::remove_file(&log).unwrap();
        }
        outputs.push(worktree_json(&sandbox, &ws, &["add", "t", "--repo", repo]));
        assert_eq!(
            calls(&log),
            ["--json version"],
            "{schema}: asked, never used"
        );
        assert!(ws.join(".worktrees/t").join(repo).join("README").exists());
    }

    for entry in outputs.iter().flat_map(backends) {
        assert_eq!(entry.1, json!("git"), "{entry:?}");
        assert!(entry.2.is_null(), "{entry:?}");
    }
}

#[test]
fn git_cowtree_makes_each_worktree_from_gits_arguments_and_declines_to_git() {
    let sandbox = Sandbox::new();
    let ws = workspace(&sandbox);
    let log = fake_cowtree(&sandbox, "git-cowtree.v1", 0);
    behave(&sandbox, "b", "git");
    behave(&sandbox, "c", "unsupported");

    let created = worktree_json(
        &sandbox,
        &ws,
        &["create", "t", "--repo", "a", "--repo", "b", "--repo", "c"],
    );
    let set = ws.join(".worktrees/t");
    assert_eq!(
        backends(&created),
        [
            (".".into(), json!("reflink"), Value::Null),
            ("a".into(), json!("reflink"), Value::Null),
            ("b".into(), json!("git"), Value::Null),
            (
                "c".into(),
                json!("git"),
                json!("no reflinks on this filesystem")
            ),
        ]
    );
    // asked once for the whole command; each add gets git's own arguments
    let calls = calls(&log);
    assert_eq!(calls.iter().filter(|c| c.ends_with(" version")).count(), 1);
    assert_eq!(
        calls.get(1).map(String::as_str),
        Some(format!("--json add -b t {}", set.display()).as_str())
    );
    assert!(set.join("c/README").exists(), "git made c's worktree");

    // add goes through it too; its preview runs nothing
    worktree_json(&sandbox, &ws, &["create", "u", "--repo", "a"]);
    let before = self::calls(&log).len();
    let out = worktree(
        &sandbox,
        &ws,
        &["add", "u", "--repo", "b"],
        json!({"json_output": true, "dry_run": true}),
    );
    let preview: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(preview["repos"][0].get("backend").is_none(), "{preview}");
    assert_eq!(self::calls(&log).len(), before, "a preview runs nothing");
    let out = worktree(&sandbox, &ws, &["add", "u", "--repo", "c"], json!({}));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("[git: git-cowtree declined: no reflinks"),
        "{text}"
    );
    behave(&sandbox, "b", "reflink");
    let out = worktree(&sandbox, &ws, &["add", "u", "--repo", "b"], json!({}));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("[reflink]"), "{text}");

    // meta's remove takes them all, whatever made them
    for name in ["t", "u"] {
        worktree_json(&sandbox, &ws, &["remove", name]);
        assert!(!ws.join(".worktrees").join(name).exists());
    }
}

#[test]
fn any_other_git_cowtree_failure_rolls_the_create_back_and_never_retries_git() {
    for mode in ["git_failed", "hook", "io_error_3", "unsupported_5"] {
        let sandbox = Sandbox::new();
        let ws = workspace(&sandbox);
        let log = fake_cowtree(&sandbox, "git-cowtree.v1", 0);
        behave(&sandbox, "b", mode);

        let args = ["create", "t", "--repo", "a", "--repo", "b"];
        let out = worktree(&sandbox, &ws, &args, json!({}));
        let stderr = String::from_utf8_lossy(&out.stderr);
        // git would have made b's worktree, so the create would succeed
        assert!(!out.status.success(), "{mode}: {stderr}");
        assert!(stderr.contains("git-cowtree add"), "{mode}: {stderr}");
        assert!(!ws.join(".worktrees/t").exists(), "{mode}: {stderr}");
        for repo in [ws.clone(), ws.join("a"), ws.join("b")] {
            assert_eq!(worktree_count(&sandbox, &repo), 1, "{mode}: {stderr}");
        }
        // the branches it made in the worktrees made before b's are gone
        assert!(!has_branch(&sandbox, &ws, "t"), "{mode}");
        assert!(!has_branch(&sandbox, &ws.join("a"), "t"), "{mode}");
        assert_eq!(
            calls(&log).iter().filter(|c| c.contains(" add ")).count(),
            3,
            "{mode}: no add is tried twice"
        );
    }
}

#[test]
fn a_worktree_git_cowtree_could_not_undo_is_named_and_never_reused() {
    let sandbox = Sandbox::new();
    let ws = workspace(&sandbox);
    let log = fake_cowtree(&sandbox, "git-cowtree.v1", 0);
    behave(&sandbox, "b", "rollback_failed");

    let args = ["create", "t", "--repo", "a", "--repo", "b"];
    let out = worktree(&sandbox, &ws, &args, json!({}));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    let left = ws.join(".worktrees/t/b");
    assert!(stderr.contains("rollback_failed"), "{stderr}");
    assert!(stderr.contains(&left.display().to_string()), "{stderr}");
    // git still has it, locked: meta deletes nothing of the set and says so
    assert!(left.join("README").exists(), "{stderr}");
    assert!(stderr.contains("left in place"), "{stderr}");
    assert_eq!(
        calls(&log).iter().filter(|c| c.contains(" add ")).count(),
        3,
        "git never retried b's add"
    );
    // the path isn't reused until someone has looked at it
    let out = worktree(&sandbox, &ws, &args, json!({}));
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("already exists"));
}
