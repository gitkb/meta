//! What `meta git worktree create` and `add` do about a branch this clone
//! hasn't fetched, and about dependencies only `-r` follows: through real
//! meta, real meta-git and local bare origins.

#![expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]
#![cfg(unix)]

use meta_plugin::testing::Sandbox;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

struct Fixture {
    sandbox: Sandbox,
    ws: PathBuf,
}

/// The workspace: `a` and `b`, and the nested meta repo `platform` with
/// `api` (depends on `core`) and `core`.
fn fixture() -> Fixture {
    let sandbox = Sandbox::new();
    let ws = sandbox
        .workspace(&sandbox.work().join("ws"))
        .repo("a")
        .repo("b")
        .nested("platform", |w| w.repo("api").repo("core"))
        .build();
    let fixture = Fixture { sandbox, ws };
    fixture.config("platform", "api", json!({"depends_on": ["core"]}));
    fixture
}

impl Fixture {
    /// Merge `fields` into the entry of `project` in the config in `dir`.
    fn config(&self, dir: &str, project: &str, fields: Value) {
        let path = self.ws.join(dir).join(".meta.yaml");
        let mut config: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let entry = &mut config["projects"][project];
        if entry.is_string() {
            *entry = json!({"repo": entry.clone()});
        }
        for (key, value) in fields.as_object().unwrap() {
            entry[key] = value.clone();
        }
        fs::write(path, config.to_string()).unwrap();
    }

    fn meta(&self, words: &[&str]) -> Output {
        let built = Path::new(env!("CARGO_BIN_EXE_meta")).with_file_name("");
        self.sandbox
            .command(env!("CARGO_BIN_EXE_meta"))
            .args(words)
            .current_dir(&self.ws)
            .env("PATH", format!("{}:/usr/bin:/bin", built.display()))
            .output()
            .unwrap()
    }

    fn origin(&self, project: &str) -> PathBuf {
        self.sandbox
            .work()
            .join("up")
            .join(format!("{project}.git"))
    }

    /// Push branch `branch` with one commit of `file` to `project`'s origin,
    /// after this clone was made.
    fn push_branch(&self, project: &str, branch: &str, file: &str) {
        let side = self.sandbox.work().join(format!("side-{project}-{branch}"));
        let url = self.origin(project).display().to_string();
        let work = self.sandbox.work();
        self.sandbox
            .git(&work, &["clone", "-q", &url, side.to_str().unwrap()]);
        self.sandbox.git(&side, &["switch", "-qc", branch]);
        fs::write(side.join(file), "x").unwrap();
        self.sandbox.git(&side, &["add", file]);
        self.sandbox.git(&side, &["commit", "-qm", "on the branch"]);
        self.sandbox.git(&side, &["push", "-q", "origin", branch]);
    }

    fn set(&self, name: &str) -> PathBuf {
        self.ws.join(".worktrees").join(name)
    }

    /// `git status -sb`'s first line in the checkout `dir`.
    fn branch_line(&self, dir: &Path) -> String {
        self.sandbox.git(dir, &["status", "-sb"])
    }

    fn refs_and_files(&self, project: &str) -> (String, bool, bool) {
        let dir = self.ws.join(project);
        (
            self.sandbox.git(&dir, &["for-each-ref"]),
            dir.join(".git/FETCH_HEAD").exists(),
            self.ws.join(".worktrees/p").exists(),
        )
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn ok(out: Output) -> Output {
    assert!(out.status.success(), "{}", text(&out));
    out
}

fn envelope(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("{}", text(out)))
}

/// On main this made a new branch from `origin/main` with the same name.
#[test]
fn a_branch_only_origin_has_is_continued_and_tracked() {
    let f = fixture();
    f.push_branch("a", "x", "from-x");
    f.push_branch("a", "y", "from-y");
    f.push_branch("platform/api", "nx", "from-x");

    let out = ok(f.meta(&[
        "git", "worktree", "create", "c", "--repo", "a", "--branch", "x",
    ]));
    assert!(text(&out).contains("new from origin/x"), "{}", text(&out));
    let a = f.set("c/a");
    assert!(a.join("from-x").exists());
    assert!(f.branch_line(&a).contains("x...origin/x"));

    // per repo, and a branch defaulted from the set's name
    ok(f.meta(&["git", "worktree", "create", "d", "--repo", "a:y"]));
    assert!(f.set("d/a/from-y").exists());
    f.push_branch("b", "named", "from-named");
    ok(f.meta(&["git", "worktree", "create", "named", "--repo", "b"]));
    assert!(f.set("named/b/from-named").exists());

    // with -r on a nested repo
    ok(f.meta(&[
        "-r",
        "git",
        "worktree",
        "create",
        "n",
        "--repo",
        "platform/api",
        "--branch",
        "nx",
    ]));
    assert!(f.set("n/platform/api/from-x").exists());

    // `worktree add`
    f.push_branch("b", "z", "from-z");
    ok(f.meta(&["git", "worktree", "create", "e", "--repo", "a", "--no-deps"]));
    ok(f.meta(&["git", "worktree", "add", "e", "--repo", "b:z"]));
    assert!(f.set("e/b/from-z").exists());
    assert!(f.branch_line(&f.set("e/b")).contains("z...origin/z"));
}

/// `git clone --depth` makes a single-branch clone: `--track` needs the
/// branch in its fetch config.
#[test]
fn a_shallow_clone_continues_the_branch_too() {
    let f = fixture();
    f.push_branch("a", "x", "from-x");
    let a = f.ws.join("a");
    fs::remove_dir_all(&a).unwrap();
    let url = format!("file://{}", f.origin("a").display());
    f.sandbox
        .git(&f.ws, &["clone", "-q", "--depth", "1", &url, "a"]);
    ok(f.meta(&[
        "git",
        "worktree",
        "create",
        "c",
        "--repo",
        "a",
        "--no-deps",
        "--branch",
        "x",
    ]));
    assert!(f.set("c/a/from-x").exists());
    assert!(f.branch_line(&f.set("c/a")).contains("x...origin/x"));
}

#[test]
fn each_repo_starts_from_what_its_origin_has() {
    let f = fixture();
    f.push_branch("a", "x", "from-x");
    let out = ok(f.meta(&[
        "git", "worktree", "create", "c", "--repo", "a", "--repo", "b", "--branch", "x",
    ]));
    let said = text(&out);
    assert!(said.contains("a -> x (new from origin/x)"), "{said}");
    assert!(said.contains("b -> x (new from origin/main)"), "{said}");
    assert!(f.set("c/a/from-x").exists());
    assert!(!f.set("c/b/from-x").exists());
}

/// A preview gives the start the run will take and leaves no trace.
#[test]
fn a_preview_says_the_real_start_and_writes_nothing() {
    let f = fixture();
    f.push_branch("a", "x", "from-x");
    let before = f.refs_and_files("a");
    let line = [
        "git",
        "worktree",
        "create",
        "p",
        "--repo",
        "a",
        "--no-deps",
        "--branch",
        "x",
    ];
    let preview = ok(f.meta(&[&["--dry-run"], &line[..]].concat()));
    let said = text(&preview);
    assert!(
        said.contains("fetch origin") && said.contains("--track -b x"),
        "{said}"
    );
    let plan = envelope(&ok(f.meta(&[&["--json", "--dry-run"], &line[..]].concat())));
    let repo = &plan["results"][0]["data"]["repos"][0];
    assert_eq!(repo["created_branch"], true, "{plan}");
    assert_eq!(repo["started_from"], "origin/x", "{plan}");
    assert_eq!(f.refs_and_files("a"), before);

    let made = envelope(&ok(f.meta(&[&["--json"], &line[..]].concat())));
    let made = &made["results"][0]["data"]["repos"][0];
    assert_eq!(made["started_from"], repo["started_from"]);
}

#[test]
fn an_origin_that_cannot_answer_warns_and_strict_refuses() {
    let f = fixture();
    let a = f.ws.join("a");
    f.sandbox
        .git(&a, &["remote", "set-url", "origin", "/nonexistent/a.git"]);
    let line = ["git", "worktree", "create", "c", "--repo", "a", "--no-deps"];
    let strict = f.meta(&[&["--strict"], &line[..]].concat());
    assert!(!strict.status.success(), "{}", text(&strict));
    assert!(!f.set("c").exists(), "{}", text(&strict));
    let out = envelope(&ok(f.meta(&[&["--json"], &line[..]].concat())));
    let warnings = out["warnings"].to_string();
    assert!(
        warnings.contains("'a'") && warnings.contains("branch 'c'"),
        "{out}"
    );
    assert!(f.set("c/a").is_dir());

    // no origin at all: not asked, nothing said, and --strict goes on
    let b = f.ws.join("b");
    f.sandbox.git(&b, &["remote", "remove", "origin"]);
    let out = ok(f.meta(&[
        "--strict",
        "git",
        "worktree",
        "create",
        "o",
        "--repo",
        "b",
        "--no-deps",
    ]));
    assert!(!text(&out).contains("origin of"), "{}", text(&out));
}

#[test]
fn an_origin_that_hangs_costs_seconds_only() {
    let f = fixture();
    let a = f.ws.join("a");
    f.sandbox
        .git(&a, &["config", "protocol.ext.allow", "always"]);
    f.sandbox
        .git(&a, &["remote", "set-url", "origin", "ext::sleep 60"]);
    let start = Instant::now();
    let out = ok(f.meta(&["git", "worktree", "create", "c", "--repo", "a", "--no-deps"]));
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
    assert!(text(&out).contains("branch 'c'"), "{}", text(&out));
}

/// A local branch keeps its place before anything origin says: not asked.
#[test]
fn a_local_branch_and_a_commit_ish_are_not_asked_about() {
    let f = fixture();
    let a = f.ws.join("a");
    f.sandbox.git(&a, &["branch", "x"]);
    f.sandbox
        .git(&a, &["remote", "set-url", "origin", "/nonexistent/a.git"]);
    let line = [
        "git",
        "worktree",
        "create",
        "c",
        "--repo",
        "a",
        "--no-deps",
        "--branch",
        "x",
    ];
    let out = ok(f.meta(&line));
    assert!(!text(&out).contains("origin of"), "{}", text(&out));
    let out = ok(f.meta(&[
        "git",
        "worktree",
        "create",
        "d",
        "HEAD",
        "--repo",
        "a",
        "--no-deps",
        "--branch",
        "other",
    ]));
    assert!(!text(&out).contains("origin of"), "{}", text(&out));
}

#[test]
fn a_branch_name_like_an_option_never_reaches_git_as_one() {
    let f = fixture();
    let out = f.meta(&[
        "git",
        "worktree",
        "create",
        "c",
        "--repo",
        "a",
        "--no-deps",
        "--branch=--upload-pack=x",
    ]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(!text(&out).contains("origin of"), "{}", text(&out));
}

fn create_api(f: &Fixture, options: &[&str]) -> Output {
    let tail = ["git", "worktree", "create", "s", "--repo", "platform/api"];
    f.meta(&[options, &tail[..]].concat())
}

/// On main the dependency was dropped without a word.
#[test]
fn a_nested_repo_says_which_dependency_only_r_follows() {
    let f = fixture();
    let out = ok(create_api(&f, &["--json"]));
    let out = envelope(&out);
    let warnings = out["warnings"].to_string();
    assert!(
        warnings.contains("'platform/core'") && warnings.contains("--repo platform/core"),
        "{out}"
    );
    // nothing is added for it
    assert!(!f.set("s/platform/core").exists());
    assert!(f.set("s/platform/api").is_dir());
}

#[test]
fn strict_refuses_it_and_r_follows_it() {
    let f = fixture();
    let out = create_api(&f, &["--strict"]);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("only -r follows"), "{}", text(&out));
    assert!(!f.set("s").exists());

    let out = ok(create_api(&f, &["-r"]));
    assert!(!text(&out).contains("only -r follows"), "{}", text(&out));
    assert!(f.set("s/platform/core").is_dir());
}

/// On the PR's first head this warned, and `--strict` failed, though the
/// set held every dependency.
#[test]
fn a_dependency_the_set_holds_is_not_warned_about() {
    let f = fixture();
    let out = ok(f.meta(&[
        "--strict",
        "git",
        "worktree",
        "create",
        "s",
        "--repo",
        "platform/api",
        "--repo",
        "platform/core",
    ]));
    assert!(!text(&out).contains("only -r"), "{}", text(&out));
}

/// `-r` doesn't follow a dependency across nesting levels: it is named, with
/// the `--repo` that adds it, and once named nothing is said.
#[test]
fn r_names_a_dependency_across_levels_and_the_repo_to_add() {
    let f = fixture();
    f.config("platform", "core", json!({"provides": ["core-cap"]}));
    f.config(".", "a", json!({"depends_on": ["core-cap"]}));
    let out = ok(f.meta(&["-r", "git", "worktree", "create", "t", "--repo", "a"]));
    let said = text(&out);
    assert!(!f.set("t/platform/core").exists(), "{said}");
    assert!(
        said.contains("'platform/core', at another nesting level")
            && said.trim_end().ends_with("--repo platform/core"),
        "{said}"
    );
    let line = [
        "--strict",
        "-r",
        "git",
        "worktree",
        "create",
        "u",
        "--repo",
        "a",
        "--repo",
        "platform/core",
    ];
    let out = ok(f.meta(&line));
    assert!(!text(&out).contains("warning"), "{}", text(&out));
}

/// On the PR's second head a nested provider took a top-level dependency
/// from the provider of its own config.
#[test]
fn a_capability_two_levels_provide_resolves_in_its_own_config() {
    let f = fixture();
    f.config(".", "b", json!({"provides": ["cap"]}));
    f.config("platform", "core", json!({"provides": ["cap"]}));
    f.config(".", "a", json!({"depends_on": ["cap"]}));
    for (name, options) in [("s", vec!["--strict"]), ("t", vec!["--strict", "-r"])] {
        let tail = ["git", "worktree", "create", name, "--repo", "a"];
        let out = ok(f.meta(&[&options[..], &tail[..]].concat()));
        assert!(f.set(name).join("b").is_dir(), "{}", text(&out));
        assert!(
            !f.set(name).join("platform/core").exists(),
            "{}",
            text(&out)
        );
    }
}

/// On the PR's second head, created from inside a nested meta repo, a
/// dependency at the parent level was left out without a word.
#[test]
fn from_a_nested_meta_repo_a_parent_level_dependency_is_named() {
    let f = fixture();
    f.config(".", "b", json!({"provides": ["b-cap"]}));
    f.config("platform", "core", json!({"depends_on": ["b-cap"]}));
    let out = f
        .sandbox
        .command(env!("CARGO_BIN_EXE_meta"))
        .args(["git", "worktree", "create", "s", "--repo", "core"])
        .current_dir(f.ws.join("platform"))
        .env(
            "PATH",
            format!(
                "{}:/usr/bin:/bin",
                Path::new(env!("CARGO_BIN_EXE_meta"))
                    .with_file_name("")
                    .display()
            ),
        )
        .output()
        .unwrap();
    let said = text(&ok(out));
    assert!(
        said.contains("depends on 'b', at another nesting level"),
        "{said}"
    );
}

#[test]
fn only_a_dependency_across_nesting_is_warned_about() {
    let f = fixture();
    let warned = |f: &Fixture, words: &[&str]| {
        let out = ok(f.meta(words));
        text(&out).contains("--repo")
    };
    // --no-deps is silent
    assert!(!warned(
        &f,
        &[
            "git",
            "worktree",
            "create",
            "n",
            "--repo",
            "platform/api",
            "--no-deps"
        ]
    ));
    // no dependencies
    assert!(!warned(
        &f,
        &["git", "worktree", "create", "o", "--repo", "platform/core"]
    ));
    // a top-level repo depending on a top-level repo
    f.config(".", "a", json!({"depends_on": ["b"]}));
    assert!(!warned(
        &f,
        &["git", "worktree", "create", "t", "--repo", "a"]
    ));
    // a top-level repo depending on a nested one
    f.config(".", "a", json!({"depends_on": ["core"]}));
    assert!(warned(
        &f,
        &["git", "worktree", "create", "u", "--repo", "a"]
    ));
    // a nested repo depending on a top-level one
    f.config("platform", "core", json!({"depends_on": ["b"]}));
    assert!(warned(
        &f,
        &["git", "worktree", "create", "v", "--repo", "platform/core"]
    ));
}
