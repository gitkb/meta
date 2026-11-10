//! Commands that make and delete one worktree set at the same time: whatever
//! order they run in, a set being made or used is never deleted, and a set
//! its creator left half-made stays removable. Each test holds one command
//! at a chosen step (a fake `gh`, a checkout or post-create hook, a record
//! read through a FIFO) while another runs.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]
// Unix only: the commands held mid-way are shell scripts that block on a pipe
#![cfg(unix)]

use meta_plugin::testing::{mkfifo, Sandbox};
use meta_plugin::{Answer, Request, Response, Scope};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

struct Race {
    sandbox: Sandbox,
    root: PathBuf,
    ws: PathBuf,
}

impl Race {
    /// A workspace (not itself a repository) with repos `backend` and
    /// `frontend`, each with one commit.
    fn new() -> Self {
        let sandbox = Sandbox::new();
        let root = sandbox.work();
        let ws = root.join("ws");
        for repo in ["backend", "frontend"] {
            let dir = ws.join(repo);
            sandbox.repo(&dir);
        }
        fs::write(
            ws.join(".meta.yaml"),
            "projects:\n  backend:\n    path: backend\n  frontend:\n    path: frontend\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        Race { sandbox, root, ws }
    }

    /// `meta git worktree <args>`, started, sent the request meta would send
    /// from the workspace: in a file, its response read from another.
    fn start(&self, args: &[&str]) -> Running {
        let (sub, rest) = args.split_first().unwrap();
        let mut request = Request::new(format!("git worktree {sub}"), &self.ws);
        request.args = rest.iter().map(|a| a.to_string()).collect();
        request.scope = Scope::Workspace;
        let found = meta_core::config::Workspace::find(&self.ws, None).unwrap();
        request.worktree = meta_core::worktree::plugin_worktree(found.as_ref(), &self.ws, None);
        request.workspace = found.map(as_meta_sends);
        // it can call meta back: the `meta` built beside it (`cargo build
        // --workspace --bins` first)
        let bin = Path::new(env!("CARGO_BIN_EXE_meta-git")).parent().unwrap();
        let meta = bin.join(format!("meta{}", std::env::consts::EXE_SUFFIX));
        assert!(meta.is_file(), "{} is not built", meta.display());
        let files = tempfile::tempdir().unwrap();
        let (request_file, response) =
            (files.path().join("request"), files.path().join("response"));
        fs::write(&request_file, serde_json::to_vec(&request).unwrap()).unwrap();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let paths = [self.root.join("bin"), bin.to_path_buf()]
            .into_iter()
            .chain(std::env::split_paths(&path));
        let child = self
            .sandbox
            .command(env!("CARGO_BIN_EXE_meta-git"))
            .arg("--meta-plugin-exec")
            .current_dir(&self.ws)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env(meta_plugin::client::EXE_VAR, &meta)
            .env(meta_plugin::REQUEST_VAR, &request_file)
            .env(meta_plugin::RESPONSE_VAR, &response)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Running {
            child,
            response,
            _files: files,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        self.start(args).wait_with_output()
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

    /// Hold every create from now on in its post-create hook: its set made
    /// and recorded, its lease still held. The hook says it started
    /// (`post-create-started`) and waits for `release-post-create`.
    fn hold_post_create(&self) {
        let hook = self.holding(&self.root, "post-create", "");
        let mut config = fs::read_to_string(self.ws.join(".meta.yaml")).unwrap();
        config.push_str(&format!(
            "worktree:\n  hooks:\n    post-create: '{}'\n",
            hook.display()
        ));
        fs::write(self.ws.join(".meta.yaml"), config).unwrap();
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

/// The workspace `found` as meta sends it: its whole tree (from the
/// outermost config) as members.
fn as_meta_sends(found: meta_core::config::Workspace) -> meta_plugin::Workspace {
    use meta_core::{config, repo_filter};
    let (root, repos) = match repo_filter::Reach::of(found.clone(), true, true, None) {
        Ok(reach) => (reach.root.clone(), reach.scope().repos),
        Err(_) => (found.dir.clone(), Vec::new()),
    };
    let members = repos
        .iter()
        .map(|repo| {
            let info = repo.info.as_ref();
            meta_plugin::Project {
                name: repo.name().unwrap_or(".").to_string(),
                path: repo_filter::repo_path(&repo.dir, &root),
                abs: repo.dir.clone(),
                repo: info
                    .and_then(|i| i.repo.as_ref())
                    .map(|r| r.raw().to_string()),
                tags: repo.tags().to_vec(),
                depends_on: info.map(|i| i.depends_on.clone()).unwrap_or_default(),
                meta: info.is_some_and(|i| i.meta),
                present: repo.dir.exists(),
                ..meta_plugin::Project::default()
            }
        })
        .collect();
    meta_plugin::Workspace {
        root,
        config: found.config_path.clone(),
        listed_root: config::listed_root(&found.dir),
        members,
    }
}

/// A plugin call under way, and where its response will be.
struct Running {
    child: Child,
    response: PathBuf,
    _files: tempfile::TempDir,
}

impl Running {
    /// Its output as meta would show it: the response's warnings and error
    /// on stderr (`warning: …`, `Error: …`), and the code meta would end with
    /// (a failed record fails the command).
    fn wait_with_output(self) -> Output {
        let mut out = self.child.wait_with_output().unwrap();
        let response: Option<Response> = fs::read(&self.response)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let Some(response) = response else {
            return out;
        };
        for warning in &response.warnings {
            out.stderr
                .extend(format!("warning: {}\n", warning.message).into_bytes());
        }
        let error = match response.answer {
            Answer::Error(error) => Some(error),
            Answer::Results(results) => results.records.into_iter().find_map(|r| r.error),
            _ => None,
        };
        if let Some(error) = error {
            out.stderr
                .extend(format!("Error: {}\n", error.message).into_bytes());
            out.status = std::process::ExitStatus::from_raw(error.code.exit_code() << 8);
        }
        out
    }
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
    let sb = Race::new();
    // backend's origin names org/repo, for --from-pr
    let origin = sb.root.join("origin/org/repo.git");
    fs::create_dir_all(origin.parent().unwrap()).unwrap();
    let backend = sb.ws.join("backend");
    sb.sandbox.git(
        &backend,
        &["clone", "-q", "--bare", ".", origin.to_str().unwrap()],
    );
    sb.sandbox.git(
        &backend,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    sb.holding(&sb.root.join("bin"), "gh", "main");
    let hooks = sb.root.join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    sb.holding(&hooks, "post-checkout", "");
    sb.sandbox.git(
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
        rustix::process::Pid::from_child(&b.child),
        rustix::process::Signal::TERM,
    )
    .unwrap();
    b.child.wait().unwrap();
    assert!(sb.ws.join(".worktrees/same/frontend/.git").exists());

    sb.release("gh");
    let a = a.wait_with_output();
    assert!(!a.status.success());
    assert!(text(&a).contains("left half-made"), "{}", text(&a));

    sb.release("post-checkout");
    let removed = sb.run(&["remove", "same"]);
    assert!(removed.status.success(), "{}", text(&removed));
    assert!(!sb.ws.join(".worktrees/same").exists());
}

/// prune finds an ephemeral set its create left (due: its lease is left),
/// then, held on a record read through a FIFO, the set is deleted by hand
/// and made again by an ephemeral create held in its post-create hook.
/// prune must leave the set being made alone.
#[test]
fn prune_leaves_an_ephemeral_create_that_started_after_it_looked() {
    prune_held_while_an_ephemeral_create_starts(|_| {});
}

/// The same, with a repository that sorts first added while prune waits.
#[test]
fn prune_leaves_an_ephemeral_create_started_after_a_repository_was_added_during_it() {
    prune_held_while_an_ephemeral_create_starts(add_a_first_repository);
}

fn prune_held_while_an_ephemeral_create_starts(meanwhile: impl FnOnce(&Race)) {
    let sb = Race::new();
    let made = sb.run(&[
        "create",
        "zzz",
        "--repo",
        "frontend",
        "--no-deps",
        "--ephemeral",
    ]);
    assert!(made.status.success(), "{}", text(&made));
    let mut create = None;
    let pruned = prune_paused(&sb, || {
        meanwhile(&sb);
        fs::remove_dir_all(sb.ws.join(".worktrees/zzz")).unwrap();
        sb.hold_post_create();
        create = Some(sb.start(&[
            "create",
            "zzz",
            "--repo",
            "backend",
            "--no-deps",
            "--ephemeral",
        ]));
        sb.wait_for("post-create-started");
    });
    assert!(
        sb.ws.join(".worktrees/zzz/backend").exists(),
        "{}",
        text(&pruned)
    );

    sb.release("post-create");
    let create = create.unwrap().wait_with_output();
    assert!(create.status.success(), "{}", text(&create));
}

/// `prune`, with set `aaa` made first: it finds the sets and reads their
/// leases, then waits (on a second read of `aaa`'s record, through a FIFO)
/// while `during` runs. Its output.
fn prune_paused(sb: &Race, during: impl FnOnce()) -> Output {
    assert!(sb
        .run(&["create", "aaa", "--repo", "backend", "--no-deps"])
        .status
        .success());
    let admin = PathBuf::from(
        sb.sandbox
            .git(
                &sb.ws.join(".worktrees/aaa/backend"),
                &["rev-parse", "--absolute-git-dir"],
            )
            .trim(),
    );
    let record = admin.join("meta-worktree.json");
    let saved = fs::read(&record).unwrap();
    let fifo = |path: &Path| {
        fs::remove_file(path).unwrap();
        assert!(mkfifo(path), "this race needs a FIFO");
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
    fifo(&record);
    drop(first);
    // ...and its second (checking `aaa`, after it has read the leases)
    // waits: once prune has opened the FIFO, `during` runs
    let mut second = writer(record.clone()).join().unwrap();
    fs::remove_file(&record).unwrap();
    fs::write(&record, &saved).unwrap();

    during();

    second.write_all(&saved).unwrap();
    drop(second);
    prune.wait_with_output()
}

/// Where the workspace root isn't a repository, the leases' home is its
/// first repository by path, which moves when one that sorts first is
/// added. prune must still see the lease of a set being made, whose TTL
/// has run out meanwhile; a set with the same TTL that no one holds goes.
#[test]
fn prune_leaves_a_create_whose_ttl_ran_out_after_a_repository_is_added() {
    let sb = Race::new();
    let control = sb.run(&[
        "create",
        "ctl",
        "--repo",
        "backend",
        "--no-deps",
        "--ttl",
        "1s",
    ]);
    assert!(control.status.success(), "{}", text(&control));
    sb.hold_post_create();
    let create = sb.start(&[
        "create",
        "zzz",
        "--repo",
        "frontend",
        "--no-deps",
        "--ephemeral",
        "--ttl",
        "1s",
    ]);
    sb.wait_for("post-create-started");
    std::thread::sleep(Duration::from_millis(2100));

    add_a_first_repository(&sb);
    let pruned = sb.run(&["prune"]);
    assert!(pruned.status.success(), "{}", text(&pruned));
    assert!(!sb.ws.join(".worktrees/ctl").exists(), "{}", text(&pruned));
    assert!(
        sb.ws.join(".worktrees/zzz/frontend").exists(),
        "{}",
        text(&pruned)
    );

    sb.release("post-create");
    let create = create.wait_with_output();
    assert!(create.status.success(), "{}", text(&create));
}

/// A repository `aaa` (sorting before the others), listed in the workspace
/// beside what its config already says.
fn add_a_first_repository(sb: &Race) {
    let first = sb.ws.join("aaa");
    fs::create_dir_all(&first).unwrap();
    sb.sandbox.git(&first, &["init", "-q", "-b", "main"]);
    sb.sandbox
        .git(&first, &["commit", "-q", "--allow-empty", "-m", "initial"]);
    let config = fs::read_to_string(sb.ws.join(".meta.yaml")).unwrap();
    let config = config.replacen("projects:\n", "projects:\n  aaa:\n    path: aaa\n", 1);
    fs::write(sb.ws.join(".meta.yaml"), config).unwrap();
}

/// An ephemeral create is killed in its post-create hook (its set made and
/// recorded, its lease held), leaving its lease. prune finds the set due
/// through that lease; then, while prune waits, the set is deleted by hand,
/// a repository added, and an ordinary set made again at that path. The
/// lease the killed create left must not make prune remove the new set.
#[test]
fn a_lease_left_by_a_killed_create_does_not_expire_a_set_made_again() {
    let sb = Race::new();
    sb.hold_post_create();
    let mut create = sb.start(&[
        "create",
        "same",
        "--repo",
        "frontend",
        "--no-deps",
        "--ephemeral",
    ]);
    sb.wait_for("post-create-started");
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&create.child),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    create.child.wait().unwrap();
    // released: the hook holds no create from now on
    sb.release("post-create");

    let pruned = prune_paused(&sb, || {
        fs::remove_dir_all(sb.ws.join(".worktrees/same")).unwrap();
        add_a_first_repository(&sb);
        let made = sb.run(&["create", "same", "--repo", "backend", "--no-deps"]);
        assert!(made.status.success(), "{}", text(&made));
    });
    assert!(pruned.status.success(), "{}", text(&pruned));
    assert!(
        sb.ws.join(".worktrees/same/backend").exists(),
        "{}",
        text(&pruned)
    );
}

/// `git clean -fdx` in the workspace deletes the loose files in the (ignored)
/// worktrees folder, an ephemeral set's lease among them. prune must not
/// take a missing lease for a create that ended; an ephemeral set whose
/// lease was left goes.
#[test]
fn prune_leaves_an_ephemeral_set_whose_lease_was_deleted() {
    let sb = Race::new();
    let made = sb.run(&[
        "create",
        "eph",
        "--repo",
        "backend",
        "--no-deps",
        "--ephemeral",
    ]);
    assert!(made.status.success(), "{}", text(&made));
    let worktrees = sb.ws.join(".worktrees");
    let mut deleted = 0;
    for entry in fs::read_dir(&worktrees).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            fs::remove_file(path).unwrap();
            deleted += 1;
        }
    }
    // a create that is done left its lease and let go of the lock
    assert_eq!(deleted, 1, "the lease");
    let control = sb.run(&[
        "create",
        "ctl",
        "--repo",
        "frontend",
        "--no-deps",
        "--ephemeral",
    ]);
    assert!(control.status.success(), "{}", text(&control));

    let pruned = sb.run(&["prune"]);
    assert!(pruned.status.success(), "{}", text(&pruned));
    assert!(worktrees.join("eph/backend").exists(), "{}", text(&pruned));
    assert!(!worktrees.join("ctl").exists(), "{}", text(&pruned));
}
