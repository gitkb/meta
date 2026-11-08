//! What meta suggests after a mistake is a command that works as printed.

#![expect(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A workspace repo with a cloned project `api` (one commit on `main`), and
/// a home, config and data dir of its own.
struct Workspace {
    _temp: tempfile::TempDir,
    ws: PathBuf,
    base: PathBuf,
}

impl Workspace {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        for dir in ["home", "config", "data", "meta", "tmp"] {
            fs::create_dir_all(base.join(dir)).unwrap();
        }
        fs::write(
            base.join("gitconfig"),
            "[user]\n\tname = T\n\temail = t@example.invalid\n[init]\n\tdefaultBranch = main\n",
        )
        .unwrap();
        let workspace = Self {
            ws: base.join("ws"),
            base,
            _temp: temp,
        };
        let api = workspace.ws.join("api");
        fs::create_dir_all(&api).unwrap();
        workspace.git(&workspace.ws, &["init", "-q"]);
        workspace.git(&api, &["init", "-q"]);
        fs::write(api.join("README.md"), "# api\n").unwrap();
        workspace.git(&api, &["add", "."]);
        workspace.git(&api, &["commit", "-qm", "init"]);
        fs::write(
            workspace.ws.join(".meta.yaml"),
            "projects:\n  api: https://example.invalid/api.git\n",
        )
        .unwrap();
        fs::write(workspace.ws.join(".gitignore"), "/api/\n.worktrees/\n").unwrap();
        workspace.git(&workspace.ws, &["add", "."]);
        workspace.git(&workspace.ws, &["commit", "-qm", "workspace"]);
        workspace
    }

    fn command(&self, program: impl AsRef<std::ffi::OsStr>, dir: &Path) -> Command {
        let bin = Path::new(env!("CARGO_BIN_EXE_meta")).parent().unwrap();
        let mut command = Command::new(program);
        command
            .current_dir(dir)
            .env_clear()
            .env("HOME", self.base.join("home"))
            .env("XDG_CONFIG_HOME", self.base.join("config"))
            .env("XDG_DATA_HOME", self.base.join("data"))
            .env("META_DATA_DIR", self.base.join("meta"))
            .env("TMPDIR", self.base.join("tmp"))
            .env("GIT_CONFIG_GLOBAL", self.base.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "PATH",
                format!("{}:/usr/local/bin:/usr/bin:/bin", bin.display()),
            )
            .stdin(Stdio::null());
        command
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        assert!(self
            .command("git", dir)
            .args(args)
            .status()
            .unwrap()
            .success());
    }

    fn meta(&self, args: &[&str]) -> Output {
        let bin = Path::new(env!("CARGO_BIN_EXE_meta"));
        self.command(bin, &self.ws).args(args).output().unwrap()
    }
}

fn text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
}

/// The command a message suggests: its line that starts with `meta `.
fn suggested(out: &Output) -> Vec<String> {
    let said = text(out);
    let line = said
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("meta "))
        .unwrap_or_default();
    shlex::split(line)
        .unwrap_or_default()
        .into_iter()
        .skip(1)
        .collect()
}

#[test]
fn a_typo_of_a_command_suggests_the_command_it_is_near() {
    let workspace = Workspace::new();
    for (typo, near) in [
        (&["gti", "status"][..], &["git", "status"][..]),
        (&["exce", "--", "ls"], &["exec", "--", "ls"]),
        (&["project", "lsit"], &["project", "list"]),
        // asking a typo for its help gets the command it is near, not help
        (
            &["project", "lsit", "--help"],
            &["project", "list", "--help"],
        ),
    ] {
        let out = workspace.meta(typo);
        assert_eq!(out.status.code(), Some(2), "{}", text(&out));
        assert_eq!(suggested(&out), near, "{}", text(&out));
        // and that one runs
        assert!(workspace.meta(near).status.success(), "meta {near:?}");
    }
    // meta's options typed after the typo go before the command, and the
    // ones typed before it stay
    for (typo, near) in [
        (
            &["project", "lsit", "--json"][..],
            &["--json", "project", "list"][..],
        ),
        (
            &["--json", "project", "lsit"],
            &["--json", "project", "list"],
        ),
        (
            &["-r", "project", "lsit", "--json"],
            &["-r", "--json", "project", "list"],
        ),
    ] {
        let out = workspace.meta(typo);
        assert_eq!(out.status.code(), Some(2), "{}", text(&out));
        // under --json the message is the Envelope's error
        let said = match serde_json::from_slice::<serde_json::Value>(&out.stdout) {
            Ok(envelope) => envelope["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            Err(_) => text(&out),
        };
        let line = said
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with("meta "))
            .unwrap_or_default();
        assert_eq!(
            shlex::split(line)
                .unwrap_or_default()
                .get(1..)
                .unwrap_or_default(),
            near,
            "{said}"
        );
        assert!(workspace.meta(near).status.success(), "meta {near:?}");
    }
    // meta's options before the command stay: a preview stays a preview
    let out = workspace.meta(&["--dry-run", "--include", "api", "exce", "--", "rm", "x"]);
    assert_eq!(
        suggested(&out),
        ["--dry-run", "--include", "api", "exec", "--", "rm", "x"],
        "{}",
        text(&out)
    );
    let out = workspace.meta(&["--include", "api", "query", "x"]);
    assert_eq!(
        suggested(&out),
        ["--include", "api", "exec", "--", "query", "x"]
    );
    // the line is changed at the command word alone: an option's value that
    // looks like a command word stays (a project named `help`)
    let out = workspace.meta(&["--exclude", "help", "printf", "x"]);
    assert_eq!(
        suggested(&out),
        ["--exclude", "help", "exec", "--", "printf", "x"]
    );
    // through help, a typo is fixed; a program isn't run
    let out = workspace.meta(&["help", "gti", "status"]);
    assert_eq!(suggested(&out), ["help", "git", "status"]);
    let out = workspace.meta(&["--include", "api", "help", "printf", "x"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(suggested(&out).is_empty(), "{}", text(&out));
    // nothing near, a program on PATH (`cp` is near the `meta-mcp` file
    // beside meta), or a word whose file isn't a plugin (`meta-mcp`): run it
    // in each repo
    for words in [&["query", "x"][..], &["cp", "a", "b"], &["mcpq", "x"]] {
        let out = workspace.meta(words);
        let mut exec = vec!["exec", "--"];
        exec.extend(words);
        assert_eq!(suggested(&out), exec, "{}", text(&out));
    }
}

/// The line a tip gives to run instead: after its `as in meta `.
fn tip_line(out: &Output) -> Vec<String> {
    let said = text(out);
    let line = said
        .lines()
        .find_map(|line| line.split_once("as in meta ").map(|(_, line)| line))
        .unwrap_or_default();
    shlex::split(line).unwrap_or_default()
}

#[test]
fn a_meta_option_after_any_command_gets_the_line_with_it_before() {
    let workspace = Workspace::new();
    let url = "https://example.invalid/m.git";
    // as typed, and the line the tip gives: meta runs it (project check
    // then finds api has no origin)
    for (typed, line) in [
        (
            &["exec", "--include", "api", "--", "pwd"][..],
            &["--include", "api", "exec", "--", "pwd"][..],
        ),
        (&["context", "--dry-run"], &["--dry-run", "context"]),
        // two of them: both move, so the line runs
        (
            &["context", "--verbose", "--dry-run"],
            &["--verbose", "--dry-run", "context"],
        ),
        (
            &["project", "list", "--json", "-r"],
            &["--json", "-r", "project", "list"],
        ),
        // typed before it too: said once, in a cluster too
        (
            &["--verbose", "project", "list", "--verbose"],
            &["--verbose", "project", "list"],
        ),
        (
            &["-rs", "project", "list", "--recursive"],
            &["-rs", "project", "list"],
        ),
        (
            &["project", "check", "--include", "api"],
            &["--include", "api", "project", "check"],
        ),
        (
            &["git", "commit", "-m", "x", "--dry-run"],
            &["--dry-run", "git", "commit", "-m", "x"],
        ),
        (
            &["git", "update", "--dry-run"],
            &["--dry-run", "git", "update"],
        ),
        (
            &[
                "git",
                "worktree",
                "create",
                "t",
                "--repo",
                "api",
                "--verbose",
            ],
            &[
                "--verbose",
                "git",
                "worktree",
                "create",
                "t",
                "--repo",
                "api",
            ],
        ),
        // meta's options typed before the command stay
        (
            &["--silent", "exec", "--include=api", "--", "pwd"],
            &["--silent", "--include=api", "exec", "--", "pwd"],
        ),
    ] {
        let out = workspace.meta(typed);
        assert_eq!(out.status.code(), Some(2), "{typed:?}: {}", text(&out));
        assert!(
            text(&out).contains("is an option of meta itself"),
            "{}",
            text(&out)
        );
        assert_eq!(tip_line(&out), line, "{}", text(&out));
        let again = workspace.meta(line);
        assert_ne!(again.status.code(), Some(2), "{line:?}: {}", text(&again));
        assert!(!text(&again).contains("tip:"), "{line:?}: {}", text(&again));
    }
    // no line that fails again: a command that never takes the option, a
    // value moved away from it, or a word the command takes (clone's
    // --depth is git's)
    for typed in [
        &["project", "list", "--include", "api"][..],
        &["git", "update", "--include", "api"],
        &["git", "worktree", "create", "t", "--tag", "backend"],
        &["git", "clone", url, "d", "--exclude", "api"],
        &["exec", "--parallel", "8", "--", "pwd"],
        // two values for one that takes one: which was meant can't be told
        &["--depth", "1", "project", "list", "--depth", "2"],
        // a value meta wouldn't take: not a number, blank, an option
        &["project", "list", "--depth", "nope"],
        &["exec", "--tag", "", "--", "pwd"],
        &["git", "commit", "-m", "x", "--include", "--foo"],
        &["git", "clone", url, "d", "--depth", "5", "--typo"],
        // meta's -A before create selects no repos: create still needs --repo
        &["git", "worktree", "create", "x1", "--all"],
        // an option no command has
        &["git", "clone", url, "o1", "--meta-depth", "2"],
    ] {
        let out = workspace.meta(typed);
        assert_eq!(out.status.code(), Some(2), "{typed:?}: {}", text(&out));
        assert!(tip_line(&out).is_empty(), "{typed:?}: {}", text(&out));
        // nor clap's "use '-- --include'", which hands it to the command
        let said = text(&out);
        assert!(!said.contains("-- --"), "{said}");
    }
    // a cluster of meta's short options before the command is read too:
    // the --json among them makes the usage error JSON
    let out = workspace.meta(&["-rv", "--json", "context", "--json"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    let error: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(error["success"], false, "{}", text(&out));
    let out = workspace.meta(&["git", "update", "--include", "api"]);
    assert!(
        text(&out)
            .contains("--include is an option of meta itself, and this command doesn't take it"),
        "{}",
        text(&out)
    );
    let out = workspace.meta(&["git", "clone", url, "d", "--depth", "5", "--typo"]);
    assert!(!text(&out).contains("option of meta"), "{}", text(&out));
    // clap's "use '-- --x'" only where the words after `--` are taken
    let out = workspace.meta(&["git", "worktree", "exec", "t", "--foo"]);
    assert!(text(&out).contains("use '-- --foo'"), "{}", text(&out));
}

#[test]
fn meta_owned_commands_take_no_copy_of_meta_options() {
    let workspace = Workspace::new();
    for (typed, line) in [
        (
            &["project", "list", "-r"][..],
            &["-r", "project", "list"][..],
        ),
        (
            &["project", "list", "--json"],
            &["--json", "project", "list"],
        ),
        (
            &["project", "check", "--json"],
            &["--json", "project", "check"],
        ),
        (
            &["project", "dependents", "api", "-r"],
            &["-r", "project", "dependents", "api"],
        ),
        (&["git", "update", "-r"], &["-r", "git", "update"]),
        (
            &["git", "clone-missing", "-r"],
            &["-r", "git", "clone-missing"],
        ),
        (&["context", "--json"], &["--json", "context"]),
        (&["plugin", "list", "--json"], &["--json", "plugin", "list"]),
        (
            &["git", "worktree", "create", "t", "--repo", "api", "-r"],
            &["-r", "git", "worktree", "create", "t", "--repo", "api"],
        ),
        (
            &[
                "git", "worktree", "create", "u", "--repo", "api", "--strict",
            ],
            &[
                "--strict", "git", "worktree", "create", "u", "--repo", "api",
            ],
        ),
        (
            &["git", "worktree", "list", "--json"],
            &["--json", "git", "worktree", "list"],
        ),
        (
            &["git", "worktree", "remove", "t", "--dry-run"],
            &["--dry-run", "git", "worktree", "remove", "t"],
        ),
        (
            &[
                "git",
                "worktree",
                "exec",
                "t",
                "--include",
                "api",
                "--",
                "pwd",
            ],
            &[
                "--include",
                "api",
                "git",
                "worktree",
                "exec",
                "t",
                "--",
                "pwd",
            ],
        ),
    ] {
        let out = workspace.meta(typed);
        assert_eq!(out.status.code(), Some(2), "{typed:?}: {}", text(&out));
        assert_eq!(tip_line(&out), line, "{}", text(&out));
        let again = workspace.meta(line);
        assert_ne!(again.status.code(), Some(2), "{line:?}: {}", text(&again));
    }
    // -r on list and status is refused: the strict scan is --strict's, and
    // nested repos are listed either way. Typed after the command, -r gets
    // the tip that it goes before, and the line with it there says so.
    for typed in [
        &["git", "worktree", "list", "-r"][..],
        &["-r", "git", "worktree", "list"],
        &["--recursive", "git", "worktree", "status", "t"],
        &["git", "worktree", "list", "--all"],
        &["-A", "git", "worktree", "list"],
        &["git", "worktree", "list", "-rv"],
    ] {
        let mut out = workspace.meta(typed);
        assert_eq!(out.status.code(), Some(2), "{typed:?}: {}", text(&out));
        if typed[0] == "git" {
            assert!(
                text(&out).contains("goes before the command"),
                "{}",
                text(&out)
            );
            let line = tip_line(&out);
            if line.is_empty() {
                continue;
            }
            let words: Vec<&str> = line.iter().map(String::as_str).collect();
            out = workspace.meta(&words);
            assert_eq!(out.status.code(), Some(2), "{typed:?}: {}", text(&out));
        }
        assert!(
            text(&out).contains("meta --strict git worktree"),
            "{typed:?}: {}",
            text(&out)
        );
    }
    let strict = workspace.meta(&["--strict", "git", "worktree", "status", "t"]);
    assert!(strict.status.success(), "{}", text(&strict));
}

#[test]
fn an_unknown_command_names_no_arguments_and_dumps_no_help() {
    let workspace = Workspace::new();
    let out = workspace.meta(&["hlep", "git", "status"]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert_eq!(suggested(&out), ["help"], "{}", text(&out));
    for words in [&["hlep", "git"][..], &["query", "x"]] {
        let said = text(&workspace.meta(words));
        assert!(!said.contains("Usage:"), "{said}");
        assert!(!said.contains("Options:"), "{said}");
        assert!(said.contains("meta --help"), "{said}");
    }
}

#[test]
fn one_argument_with_a_space_is_told_to_be_two_words() {
    let workspace = Workspace::new();
    let out = workspace.meta(&["--include", "api", "exec", "git status"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out).contains("separate words: git status, not 'git status'"),
        "{}",
        text(&out)
    );
}

#[test]
fn the_hint_for_a_worktree_set_that_exists_removes_it() {
    let workspace = Workspace::new();
    let create = ["git", "worktree", "create", "fix", "--repo", "api"];
    let made = workspace.meta(&create);
    assert!(made.status.success(), "{}", text(&made));
    let again = workspace.meta(&create);
    assert_eq!(again.status.code(), Some(1));
    let said = text(&again);
    let hint = said
        .split('\'')
        .find(|span| span.starts_with("meta "))
        .unwrap_or_default();
    let words = shlex::split(hint).unwrap_or_default();
    assert_eq!(
        words,
        ["meta", "git", "worktree", "remove", "fix"],
        "{said}"
    );
    let words: Vec<&str> = words.iter().skip(1).map(String::as_str).collect();
    assert!(workspace.meta(&words).status.success());
    assert!(!workspace.ws.join(".worktrees/fix").exists());
}

#[test]
fn worktree_diff_shows_the_patch_unless_only_the_summary_is_asked_for() {
    let workspace = Workspace::new();
    let create = ["git", "worktree", "create", "d", "--repo", "api"];
    assert!(workspace.meta(&create).status.success());
    let api = workspace.ws.join(".worktrees/d/api");
    fs::write(api.join("README.md"), "# api\nchanged\n").unwrap();
    workspace.git(&api, &["commit", "-qam", "change"]);

    let full = text(&workspace.meta(&["git", "worktree", "diff", "d"]));
    let stat = text(&workspace.meta(&["git", "worktree", "diff", "d", "--stat"]));
    assert!(full.contains("+changed"), "{full}");
    assert!(!stat.contains("+changed"), "{stat}");
    // both start with the same summary
    assert!(full.starts_with(stat.trim_end()), "{full}\n---\n{stat}");
}
