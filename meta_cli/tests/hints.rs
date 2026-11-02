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
    ] {
        let out = workspace.meta(typo);
        assert_eq!(out.status.code(), Some(2), "{}", text(&out));
        assert_eq!(suggested(&out), near, "{}", text(&out));
        // and that one runs
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
    // through help too, where only a command near the word keeps `help`
    let out = workspace.meta(&["help", "gti", "status"]);
    assert_eq!(suggested(&out), ["help", "git", "status"]);
    let out = workspace.meta(&["--include", "api", "help", "printf", "x"]);
    assert_eq!(
        suggested(&out),
        ["--include", "api", "exec", "--", "printf", "x"]
    );
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
