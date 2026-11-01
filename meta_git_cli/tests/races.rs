//! Commands that make and delete one worktree set at the same time: whatever
//! order they run in, a set being made or used is never deleted, and a set
//! its creator left half-made stays removable. Each test holds one command
//! at a chosen step (a fake `gh`, a checkout hook, a record read through a
//! FIFO) while another runs.

#![expect(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]
#![cfg(unix)]

use serde_json::json;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Sandbox {
    root: PathBuf,
    ws: PathBuf,
}

impl Sandbox {
    /// A workspace (not itself a repository) with repos `backend` and
    /// `frontend`, each with one commit.
    fn new(root: &Path) -> Sandbox {
        let ws = root.join("ws");
        for repo in ["backend", "frontend"] {
            let dir = ws.join(repo);
            fs::create_dir_all(&dir).unwrap();
            git(&dir, &["init", "-q", "-b", "main"]);
            git(&dir, &["commit", "-q", "--allow-empty", "-m", "initial"]);
        }
        fs::write(
            ws.join(".meta.yaml"),
            "projects:\n  backend:\n    path: backend\n  frontend:\n    path: frontend\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        Sandbox {
            root: root.to_path_buf(),
            ws,
        }
    }

    /// `meta git worktree <args>`, started.
    fn start(&self, args: &[&str]) -> Child {
        let (sub, rest) = args.split_first().unwrap();
        let request = json!({"command": format!("git worktree {sub}"), "args": rest,
            "cwd": self.ws, "options": {}});
        let path = std::env::var_os("PATH").unwrap_or_default();
        let paths = std::iter::once(self.root.join("bin")).chain(std::env::split_paths(&path));
        let mut child = Command::new(env!("CARGO_BIN_EXE_meta-git"))
            .arg("--meta-plugin-exec")
            .current_dir(&self.ws)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("HOME", &self.root)
            .env("XDG_CONFIG_HOME", self.root.join("xdg"))
            .env("GIT_CONFIG_GLOBAL", self.root.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("META_DATA_DIR", self.root.join("data"))
            .env_remove("META_WORKTREES")
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
        child
    }

    fn run(&self, args: &[&str]) -> Output {
        self.start(args).wait_with_output().unwrap()
    }

    /// A program `name` on the commands' PATH that says it started (touches
    /// `<name>-started`), waits for `release-<name>`, then prints `prints`.
    fn holding(&self, dir: &Path, name: &str, prints: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\ntouch '{root}/{name}-started'\n\
                 while [ ! -e '{root}/release-{name}' ] && [ -d '{root}' ]; do sleep 0.02; done\n\
                 echo {prints}\n",
                root = self.root.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn wait_for(&self, file: &str) {
        let path = self.root.join(file);
        let deadline = Instant::now() + Duration::from_secs(20);
        while !path.exists() {
            assert!(Instant::now() < deadline, "{file} never appeared");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn release(&self, name: &str) {
        fs::write(self.root.join(format!("release-{name}")), "").unwrap();
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Create A checks that the set doesn't exist, then waits (on `gh`). Create
/// B makes the set and is stopped in a checkout hook, leaving it half-made.
/// A then must not take B's lease over: it refuses, and `remove` takes the
/// half set back.
#[test]
fn a_late_create_leaves_a_half_set_removable() {
    let temp = tempfile::tempdir().unwrap();
    let sb = Sandbox::new(&temp.path().canonicalize().unwrap());
    // backend's origin names org/repo, for --from-pr
    let origin = sb.root.join("origin/org/repo.git");
    fs::create_dir_all(origin.parent().unwrap()).unwrap();
    let backend = sb.ws.join("backend");
    git(
        &backend,
        &["clone", "-q", "--bare", ".", origin.to_str().unwrap()],
    );
    git(
        &backend,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    sb.holding(&sb.root.join("bin"), "gh", "main");
    let hooks = sb.root.join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    sb.holding(&hooks, "post-checkout", "");
    git(
        &sb.ws.join("frontend"),
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );

    let a = sb.start(&[
        "create",
        "same",
        "--repo",
        "backend",
        "--no-deps",
        "--from-pr",
        "org/repo#1",
    ]);
    sb.wait_for("gh-started");
    let mut b = sb.start(&["create", "same", "--repo", "frontend", "--no-deps"]);
    sb.wait_for("post-checkout-started");
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&b),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    b.wait().unwrap();
    assert!(sb.ws.join(".worktrees/same/frontend/.git").exists());

    sb.release("gh");
    let a = a.wait_with_output().unwrap();
    assert!(!a.status.success());
    assert!(text(&a).contains("left half-made"), "{}", text(&a));

    sb.release("post-checkout");
    let removed = sb.run(&["remove", "same"]);
    assert!(removed.status.success(), "{}", text(&removed));
    assert!(!sb.ws.join(".worktrees/same").exists());
}

/// prune reads the leases, then (held on a record read through a FIFO) an
/// ephemeral run makes a set at a path whose old ephemeral set was deleted
/// by hand. prune must leave the running set alone.
#[test]
fn prune_leaves_an_ephemeral_run_that_started_after_it_looked() {
    prune_held_while_an_ephemeral_run_starts(|_| {});
}

/// The same, with a repository that sorts first added while prune waits.
#[test]
fn prune_leaves_an_ephemeral_run_started_after_a_repository_was_added_during_it() {
    prune_held_while_an_ephemeral_run_starts(add_a_first_repository);
}

/// prune reads the leases, then (held on a record read through a FIFO)
/// `meanwhile` runs and an ephemeral run makes a set at a path whose old
/// ephemeral set was deleted by hand. prune must leave the running set alone.
fn prune_held_while_an_ephemeral_run_starts(meanwhile: impl FnOnce(&Sandbox)) {
    let temp = tempfile::tempdir().unwrap();
    let sb = Sandbox::new(&temp.path().canonicalize().unwrap());
    assert!(sb
        .run(&["create", "aaa", "--repo", "backend", "--no-deps"])
        .status
        .success());
    let made = sb.run(&[
        "create",
        "zzz",
        "--repo",
        "frontend",
        "--no-deps",
        "--ephemeral",
    ]);
    assert!(made.status.success(), "{}", text(&made));
    fs::remove_dir_all(sb.ws.join(".worktrees/zzz")).unwrap();

    let admin = PathBuf::from(git(
        &sb.ws.join(".worktrees/aaa/backend"),
        &["rev-parse", "--absolute-git-dir"],
    ));
    let record = admin.join("meta-worktree.json");
    let saved = fs::read(&record).unwrap();
    let fifo = |path: &Path| {
        fs::remove_file(path).unwrap();
        let made = Command::new("mkfifo").arg(path).status().unwrap();
        assert!(made.success());
    };
    // a writer that waits for prune to open the FIFO for reading
    let writer = |path: PathBuf| {
        std::thread::spawn(move || fs::OpenOptions::new().write(true).open(path).unwrap())
    };
    fifo(&record);
    let prune = sb.start(&["prune"]);
    // prune's first read of the record (finding the sets) goes through...
    let mut first = writer(record.clone()).join().unwrap();
    first.write_all(&saved).unwrap();
    // ...and its second read (after it has read the leases) waits
    fifo(&record);
    drop(first);
    let second = writer(record.clone());

    meanwhile(&sb);
    sb.holding(&sb.root, "command", "");
    let run = sb.start(&[
        "exec",
        "--ephemeral",
        "zzz",
        "--repo",
        "backend",
        "--",
        sb.root.join("command").to_str().unwrap(),
    ]);
    sb.wait_for("command-started");

    fs::remove_file(&record).unwrap();
    fs::write(&record, &saved).unwrap();
    let mut second = second.join().unwrap();
    second.write_all(&saved).unwrap();
    drop(second);
    let pruned = prune.wait_with_output().unwrap();
    assert!(
        sb.ws.join(".worktrees/zzz/backend").exists(),
        "{}",
        text(&pruned)
    );

    sb.release("command");
    let run = run.wait_with_output().unwrap();
    assert!(run.status.success(), "{}", text(&run));
}

/// Where the workspace root isn't a repository, the leases' home is its
/// first repository by path, which moves when one that sorts first is
/// added. prune must still see a running ephemeral set's lease.
#[test]
fn prune_leaves_an_ephemeral_run_after_a_repository_is_added() {
    let temp = tempfile::tempdir().unwrap();
    let sb = Sandbox::new(&temp.path().canonicalize().unwrap());
    sb.holding(&sb.root, "command", "");
    let run = sb.start(&[
        "exec",
        "--ephemeral",
        "zzz",
        "--repo",
        "frontend",
        "--",
        sb.root.join("command").to_str().unwrap(),
    ]);
    sb.wait_for("command-started");

    let first = sb.ws.join("aaa");
    fs::create_dir_all(&first).unwrap();
    git(&first, &["init", "-q", "-b", "main"]);
    git(&first, &["commit", "-q", "--allow-empty", "-m", "initial"]);
    fs::write(
        sb.ws.join(".meta.yaml"),
        "projects:\n  aaa:\n    path: aaa\n  backend:\n    path: backend\n  frontend:\n    path: frontend\n",
    )
    .unwrap();
    let pruned = sb.run(&["prune"]);
    assert!(pruned.status.success(), "{}", text(&pruned));
    assert!(
        sb.ws.join(".worktrees/zzz/frontend").exists(),
        "{}",
        text(&pruned)
    );

    sb.release("command");
    let run = run.wait_with_output().unwrap();
    assert!(run.status.success(), "{}", text(&run));
}

/// A repository `aaa` (sorting before the others), listed in the workspace.
fn add_a_first_repository(sb: &Sandbox) {
    let first = sb.ws.join("aaa");
    fs::create_dir_all(&first).unwrap();
    git(&first, &["init", "-q", "-b", "main"]);
    git(&first, &["commit", "-q", "--allow-empty", "-m", "initial"]);
    fs::write(
        sb.ws.join(".meta.yaml"),
        "projects:\n  aaa:\n    path: aaa\n  backend:\n    path: backend\n  frontend:\n    path: frontend\n",
    )
    .unwrap();
}

/// An ephemeral run is stopped, its set deleted by hand, a repository
/// added, and an ordinary set made again at that path: the lease the stopped
/// run left must not make prune remove the new set.
#[test]
fn a_lease_left_by_a_stopped_run_does_not_expire_a_set_made_again() {
    let temp = tempfile::tempdir().unwrap();
    let sb = Sandbox::new(&temp.path().canonicalize().unwrap());
    sb.holding(&sb.root, "command", "");
    let mut run = sb.start(&[
        "exec",
        "--ephemeral",
        "same",
        "--repo",
        "frontend",
        "--",
        sb.root.join("command").to_str().unwrap(),
    ]);
    sb.wait_for("command-started");
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&run),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    run.wait().unwrap();
    sb.release("command");
    fs::remove_dir_all(sb.ws.join(".worktrees/same")).unwrap();
    add_a_first_repository(&sb);
    let made = sb.run(&["create", "same", "--repo", "backend", "--no-deps"]);
    assert!(made.status.success(), "{}", text(&made));

    let pruned = sb.run(&["prune"]);
    assert!(pruned.status.success(), "{}", text(&pruned));
    assert!(
        sb.ws.join(".worktrees/same/backend").exists(),
        "{}",
        text(&pruned)
    );
}

/// `git clean -fdx` in the workspace deletes the loose files in the (ignored)
/// worktrees folder, a running ephemeral set's lease among them. prune must
/// not take a missing lease for a run that ended.
#[test]
fn prune_leaves_an_ephemeral_run_whose_lease_was_deleted() {
    let temp = tempfile::tempdir().unwrap();
    let sb = Sandbox::new(&temp.path().canonicalize().unwrap());
    sb.holding(&sb.root, "command", "");
    let run = sb.start(&[
        "exec",
        "--ephemeral",
        "eph",
        "--repo",
        "backend",
        "--",
        sb.root.join("command").to_str().unwrap(),
    ]);
    sb.wait_for("command-started");
    let worktrees = sb.ws.join(".worktrees");
    let mut deleted = 0;
    for entry in fs::read_dir(&worktrees).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            fs::remove_file(path).unwrap();
            deleted += 1;
        }
    }
    assert_eq!(deleted, 2, "the lease and its lock");

    let pruned = sb.run(&["prune"]);
    assert!(pruned.status.success(), "{}", text(&pruned));
    assert!(worktrees.join("eph/backend").exists(), "{}", text(&pruned));

    sb.release("command");
    let run = run.wait_with_output().unwrap();
    assert!(run.status.success(), "{}", text(&run));
}
