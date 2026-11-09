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
        said.contains("depends on 'platform/core' (written 'core-cap'), which -r doesn't follow")
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
        said.contains("depends on 'b'") && said.contains("--repo b"),
        "{said}"
    );
}

/// Under `-r`, a dependency written as a project's name, where that project
/// has another `path:`, isn't followed: it is said (on the PR's third head it
/// wasn't), and once the set holds the repo nothing is.
#[test]
fn r_names_a_dependency_written_as_a_name_whose_path_differs() {
    let f = fixture();
    let rename = |from: &str, to: &str| {
        fs::create_dir_all(f.ws.join(to).parent().unwrap()).unwrap();
        fs::rename(f.ws.join(from), f.ws.join(to)).unwrap();
    };
    rename("b", "libs/b");
    f.config(".", "b", json!({"path": "libs/b"}));
    f.config(".", "a", json!({"depends_on": ["b"]}));
    rename("platform/core", "platform/libs/core");
    f.config("platform", "core", json!({"path": "libs/core"}));
    for (asked, needed) in [("a", "libs/b"), ("platform/api", "platform/libs/core")] {
        let line = [
            "--strict", "-r", "git", "worktree", "create", "s", "--repo", asked,
        ];
        let out = f.meta(&line);
        let said = text(&out);
        assert!(!out.status.success(), "{said}");
        assert!(said.contains(&format!("--repo {needed}")), "{said}");
        assert!(!f.set("s").exists(), "{said}");
        let out = ok(f.meta(&[&line[..], &["--repo", needed]].concat()));
        assert!(f.set("s").join(needed).is_dir(), "{}", text(&out));
        ok(f.meta(&["git", "worktree", "remove", "s"]));
    }
}

/// `meta --json <words>` in `dir`, with the binaries built beside the tests.
fn meta_in(f: &Fixture, dir: &Path, words: &[&str]) -> Value {
    let built = Path::new(env!("CARGO_BIN_EXE_meta")).with_file_name("");
    let out = f
        .sandbox
        .command(env!("CARGO_BIN_EXE_meta"))
        .arg("--json")
        .args(words)
        .current_dir(dir)
        .env("PATH", format!("{}:/usr/bin:/bin", built.display()))
        .output()
        .unwrap();
    envelope(&out)
}

/// What a warning's advice adds, taken literally: meta's options (`add -r`,
/// `add -A`) and the create's (the `--repo` it ends with). For "only -r
/// follows: add -r, or name it with --repo …", `second` takes the second
/// choice.
fn advice(warning: &str, second: bool) -> (Vec<String>, Vec<String>) {
    if warning.contains("only -r follows: add -r") && !second {
        return (vec!["-r".to_string()], Vec::new());
    }
    let options = match warning.contains("add -A") {
        true => vec!["-A".to_string()],
        false => Vec::new(),
    };
    let repo = warning.rsplit_once("--repo ").unwrap().1.trim().to_string();
    (options, vec!["--repo".to_string(), repo])
}

/// The fixture with a nested meta repo inside `platform`: `platform/inner`,
/// holding `x`.
fn fixture_with_inner() -> Fixture {
    let sandbox = Sandbox::new();
    let ws = sandbox
        .workspace(&sandbox.work().join("ws"))
        .repo("a")
        .repo("b")
        .nested("platform", |w| {
            w.repo("api").repo("core").nested("inner", |w| w.repo("x"))
        })
        .build();
    Fixture { sandbox, ws }
}

/// In the fixture with `platform/inner`, after `set_up`: `meta <options> git
/// worktree create s --repo <asked>` from `from` gives one warning;
/// following its advice literally (`second`: its second choice) gives a set
/// holding `wanted` (its place in the set) and no warning.
fn follow(
    set_up: impl Fn(&Fixture),
    from: &str,
    options: &[&str],
    asked: &str,
    second: bool,
    wanted: &str,
) {
    let f = fixture_with_inner();
    set_up(&f);
    let dir = f.ws.join(from);
    let create = |(more_options, more): (Vec<String>, Vec<String>)| {
        let line = ["git", "worktree", "create", "s", "--repo", asked];
        let words: Vec<&str> = options
            .iter()
            .copied()
            .chain(more_options.iter().map(String::as_str))
            .chain(line)
            .chain(more.iter().map(String::as_str))
            .collect();
        meta_in(&f, &dir, &words)
    };
    let first = create((Vec::new(), Vec::new()));
    let warnings = first["warnings"].as_array().unwrap();
    let [warning] = warnings.as_slice() else {
        panic!("{wanted}: {first}");
    };
    let warning = warning["message"].as_str().unwrap();
    assert!(warning.contains("--repo "), "{wanted}: {warning}");
    meta_in(&f, &dir, &["git", "worktree", "remove", "s"]);
    let followed = create(advice(warning, second));
    assert_eq!(followed["warnings"], json!([]), "{warning}: {followed}");
    let data = &followed["results"][0]["data"];
    let root = data["root"]
        .as_str()
        .unwrap_or_else(|| panic!("{warning}: {followed}"));
    let held = data["repos"].as_array().unwrap().iter().any(|r| {
        r["path"]
            .as_str()
            .is_some_and(|p| Path::new(p).ends_with(wanted) && p.starts_with(root))
    });
    assert!(held, "{warning}: {followed}");
}

/// From inside a nested meta repo, a parent-level dependency's advice is
/// `-A` (on the PR's fourth head it said `add -r`, which doesn't reach it);
/// from the root, `add -r`.
#[test]
fn following_a_warning_literally_adds_the_repo() {
    let core_on = |dep: &'static str| {
        move |f: &Fixture| f.config("platform", "core", json!({"depends_on": [dep]}))
    };
    follow(core_on("b"), "platform", &[], "core", false, "b");
    let by_capability = |f: &Fixture| {
        f.config(".", "b", json!({"provides": ["b-cap"]}));
        core_on("b-cap")(f);
    };
    follow(by_capability, "platform", &[], "core", false, "b");
    let api_on_core = |f: &Fixture| f.config("platform", "api", json!({"depends_on": ["core"]}));
    follow(
        api_on_core,
        ".",
        &[],
        "platform/api",
        false,
        "platform/core",
    );
}

/// The advice names a repo of the folder's tree by its path from there (on
/// the PR's fifth head, by its path from the root, which `platform/` reads
/// as unknown): a capability of a nested meta repo, with `-r`.
#[test]
fn from_a_nested_meta_repo_the_advice_names_a_capability_s_repo_from_there() {
    let set_up = |f: &Fixture| {
        f.config("platform/inner", "x", json!({"provides": ["x-cap"]}));
        f.config("platform", "api", json!({"depends_on": ["x-cap"]}));
    };
    follow(set_up, "platform", &["-r"], "api", false, "inner/x");
}

/// The same for the second choice of "only -r follows": a path into a nested
/// meta repo, without `-r`.
#[test]
fn from_a_nested_meta_repo_the_second_choice_names_the_repo_from_there() {
    let set_up = |f: &Fixture| f.config("platform", "api", json!({"depends_on": ["inner/x"]}));
    follow(set_up, "platform", &[], "api", true, "inner/x");
}

/// The same for a name whose project has another `path:`, under `-r`.
#[test]
fn from_a_nested_meta_repo_a_name_with_another_path_is_named_from_there() {
    let set_up = |f: &Fixture| {
        std::fs::create_dir_all(f.ws.join("platform/libs")).unwrap();
        std::fs::rename(f.ws.join("platform/core"), f.ws.join("platform/libs/core")).unwrap();
        f.config("platform", "core", json!({"path": "libs/core"}));
        f.config("platform", "api", json!({"depends_on": ["core"]}));
    };
    follow(set_up, "platform", &["-r"], "api", false, "libs/core");
}

/// What the guidance says `-r` does follow: a path from the workspace root,
/// across nesting levels too.
#[test]
fn r_follows_a_path_from_the_root_across_levels() {
    let f = fixture();
    f.config(".", "a", json!({"depends_on": ["platform/core"]}));
    let out = ok(f.meta(&[
        "--strict", "-r", "git", "worktree", "create", "s", "--repo", "a",
    ]));
    assert!(f.set("s/platform/core").is_dir(), "{}", text(&out));
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
