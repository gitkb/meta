//! What a `meta git` line acts on, through real meta, the real meta-git and
//! real sets: the scope and set the host sends, whether the provider check
//! runs, and whether meta's filters are refused. Two spellings of one
//! command act alike.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]
// Unix only: the plugin log is a shell script around meta-git
#![cfg(unix)]

use meta_plugin::testing::Sandbox;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

/// A workspace with projects `a` and `b`, set `s` of both made by meta-git,
/// set `j` made by another provider (jj), and a `meta-git` first on PATH
/// that logs each start and request, then runs the real one.
struct Table {
    sandbox: Sandbox,
    ws: PathBuf,
    log: PathBuf,
    bin: PathBuf,
}

impl Table {
    fn new() -> Self {
        let sandbox = Sandbox::new();
        let ws = sandbox
            .workspace(&sandbox.work().join("ws"))
            .repo("a")
            .repo("b")
            .build();
        let log = sandbox.work().join("log");
        let bin = sandbox.work().join("bin");
        fs::create_dir_all(&log).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let real = Path::new(env!("CARGO_BIN_EXE_meta"))
            .with_file_name("meta-git")
            .display()
            .to_string();
        let wrapper = bin.join("meta-git");
        fs::write(
            &wrapper,
            format!(
                r#"#!/bin/sh
echo "$1" >> "{log}/starts"
[ "$1" = --meta-plugin-exec ] && {{ cat "$META_REQUEST"; echo; }} >> "{log}/requests"
exec "{real}" "$@"
"#,
                log = log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let table = Table {
            sandbox,
            ws,
            log,
            bin,
        };
        let made = table.meta(&table.ws, "git worktree create s --repo a --repo b", &[]);
        assert!(made.status.success(), "{}", text(&made));
        let jj = table.ws.join(".worktrees/j");
        for project in ["a", "b"] {
            fs::create_dir_all(jj.join(project).join(".jj")).unwrap();
        }
        meta_plugin::sets::create(&jj, "jj", &table.ws).unwrap();
        table.forget();
        table
    }

    /// `meta <line>` (words split at spaces) in `dir`, with `env` set
    fn meta(&self, dir: &Path, line: &str, env: &[(&str, &Path)]) -> Output {
        let mut command = self.sandbox.command(env!("CARGO_BIN_EXE_meta"));
        command
            .args(line.split_whitespace())
            .current_dir(dir)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()));
        for (name, value) in env {
            command.env(name, value);
        }
        command.output().unwrap()
    }

    /// The requests meta-git got since the last call, and whether it was
    /// called at all
    fn requests(&self) -> Vec<Value> {
        let requests = fs::read_to_string(self.log.join("requests")).unwrap_or_default();
        let starts = fs::read_to_string(self.log.join("starts")).unwrap_or_default();
        self.forget();
        let got: Vec<Value> = requests
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            starts.matches("--meta-plugin-exec").count(),
            got.len(),
            "{starts}"
        );
        got
    }

    fn forget(&self) {
        for file in ["requests", "starts"] {
            let _ = fs::remove_file(self.log.join(file));
        }
    }
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// What one line should do
enum Want {
    /// Reach meta-git with this scope, set and projects (their paths)
    Sent(
        &'static str,
        Option<&'static str>,
        Option<&'static [&'static str]>,
    ),
    /// Refused before meta-git is called, saying this
    Refused(&'static str),
    /// Fail saying this, wherever it is refused
    Fails(&'static str),
}

/// Each line run with `--dry-run` from the workspace: nothing changes.
#[test]
fn each_spelling_of_a_command_acts_on_what_clap_says_it_names() {
    use Want::*;
    let table = Table::new();
    let set = Some(&[".", "a", "b"][..]);
    let filters = "does not take --include/--exclude/--tag";
    #[rustfmt::skip]
    let rows: &[(&str, Want)] = &[
        ("git worktree remove s --force", Sent("named-set", Some("s"), set)),
        ("git worktree remove --force s", Sent("named-set", Some("s"), set)),
        ("git worktree remove -- s", Sent("named-set", Some("s"), set)),
        ("git worktree status s", Sent("named-set", Some("s"), set)),
        ("git worktree diff --stat s", Sent("named-set", Some("s"), set)),
        ("git worktree add --repo a s", Sent("named-set", Some("s"), set)),
        ("git worktree exec s -- x --ephemeral", Sent("named-set", Some("s"), set)),
        ("--include a git worktree exec s -- pwd", Sent("named-set", Some("s"), Some(&["a"]))),
        ("--include a git worktree remove --force s", Refused(filters)),
        ("--include a git worktree remove s --force", Refused(filters)),
        ("--tag x git worktree add --repo a s", Refused(filters)),
        ("--include a git update", Refused(filters)),
        ("--include a git worktree list", Refused(filters)),
        ("--include a git clone u", Refused(filters)),
        // a set another provider made: refused by name, in either spelling
        ("git worktree remove --force j", Refused("made by meta-jj")),
        ("git worktree remove j --force", Refused("made by meta-jj")),
        // a name that isn't one: refused before any workspace is read
        ("git worktree remove ../x", Refused("cannot start with '.'")),
        ("git worktree remove --force ../x", Refused("cannot start with '.'")),
        // a valid name of no set: the command says so
        ("git worktree remove nope", Fails("not found")),
        // worktree commands of the workspace, and raw git
        ("git worktree list", Sent("workspace", None, Some(&[]))),
        ("git update", Sent("workspace", None, Some(&[]))),
        ("git -c x=y status", Sent("selection", None, None)),
        ("git --no-pager log -1", Sent("selection", None, None)),
        ("git help log", Sent("selection", None, None)),
        ("git -C d worktree remove s", Sent("selection", None, None)),
        ("--include a git -C d worktree remove s", Sent("selection", None, Some(&["a"]))),
        // what clap won't run: its own help or error, with no repos, and
        // filters not refused
        ("--include a git worktree", Sent("none", None, Some(&[]))),
        ("--include a git worktree remov s", Sent("none", None, Some(&[]))),
        ("git worktree exec --repo a s -- pwd", Sent("none", None, Some(&[]))),
    ];
    let mut wrong = Vec::new();
    for (line, want) in rows {
        let line = format!("--dry-run {line}");
        let out = table.meta(&table.ws, &line, &[]);
        let said = text(&out);
        let sent = table.requests();
        let problem = match want {
            Sent(scope, set, projects) => match sent.as_slice() {
                [request] => {
                    let got_set = request["set"]["name"].as_str();
                    let got_projects: Vec<&str> = request["projects"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|p| p["path"].as_str().unwrap())
                        .collect();
                    (request["scope"] != *scope
                        || got_set != *set
                        || projects.is_some_and(|p| got_projects != p))
                    .then(|| {
                        format!(
                            "sent scope {} set {got_set:?} projects {got_projects:?}",
                            request["scope"]
                        )
                    })
                }
                _ => Some(format!("{} requests: {said}", sent.len())),
            },
            Refused(says) => (!sent.is_empty() || out.status.success() || !said.contains(says))
                .then(|| format!("{} requests, {}: {said}", sent.len(), out.status)),
            Fails(says) => (out.status.success() || !said.contains(says))
                .then(|| format!("{}: {said}", out.status)),
        };
        if let Some(problem) = problem {
            wrong.push(format!("meta {line}\n  {problem}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // nothing was removed
    assert!(table.ws.join(".worktrees/s/a").is_dir());
    assert!(table.ws.join(".worktrees/j/a/.jj").is_dir());
}

/// Raw words after `--` belong to the command, and reach it as typed.
#[test]
fn the_words_after_the_double_dash_reach_the_command_unread() {
    let table = Table::new();
    let out = table
        .sandbox
        .command(env!("CARGO_BIN_EXE_meta"))
        .args([
            "git",
            "worktree",
            "exec",
            "s",
            "--",
            "sh",
            "-c",
            "echo --ephemeral > said",
        ])
        .current_dir(&table.ws)
        .env("PATH", format!("{}:/usr/bin:/bin", table.bin.display()))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    for project in ["a", "b"] {
        let said = fs::read_to_string(table.ws.join(".worktrees/s").join(project).join("said"));
        assert_eq!(said.unwrap(), "--ephemeral\n");
    }
}

/// Both spellings of removing set `s`
const REMOVE_S: [&str; 2] = [
    "git worktree remove --force s",
    "git worktree remove s --force",
];

/// `remove --force s` from inside set `s` removes it, as `remove s
/// --force` does: never a refusal about filters nobody typed.
#[test]
fn a_set_is_removed_from_inside_it_in_either_spelling() {
    for line in REMOVE_S {
        let table = Table::new();
        let out = table.meta(&table.ws.join(".worktrees/s/a"), line, &[]);
        assert!(out.status.success(), "{line}: {}", text(&out));
        assert!(!table.ws.join(".worktrees/s").exists(), "{line}");
    }
}

/// A checkout whose record can't be read is never deleted, in either
/// spelling, from inside the set or from another.
#[test]
fn a_checkout_with_an_unreadable_record_is_kept_in_either_spelling() {
    for from in ["s/a", "t/a"] {
        for line in REMOVE_S {
            let table = Table::new();
            let made = table.meta(&table.ws, "git worktree create t --repo a", &[]);
            assert!(made.status.success(), "{}", text(&made));
            let record = table.ws.join("a/.git/worktrees/a/meta-worktree.json");
            fs::write(record, "garbage").unwrap();
            let out = table.meta(&table.ws.join(".worktrees").join(from), line, &[]);
            let said = format!("{from} {line}: {}", text(&out));
            assert!(!out.status.success(), "{said}");
            assert!(table.ws.join(".worktrees/s/a").is_dir(), "{said}");
        }
    }
}

/// A set whose own `.meta.yaml` is broken is removed by no spelling, and
/// the error names that config.
#[test]
fn a_set_with_a_broken_config_is_refused_in_either_spelling() {
    for line in REMOVE_S {
        let table = Table::new();
        let config = table.ws.join(".worktrees/s/.meta.yaml");
        fs::write(&config, "{broken").unwrap();
        let out = table.meta(&table.ws, line, &[]);
        let said = format!("{line}: {}", text(&out));
        assert!(!out.status.success(), "{said}");
        assert!(said.contains(&config.display().to_string()), "{said}");
        assert!(table.ws.join(".worktrees/s/a").is_dir(), "{said}");
        assert!(table.requests().is_empty(), "{said}");
    }
}

/// With `.meta-sets` a symlink, every spelling of a command on the set is
/// refused alike, and nothing is added or run.
#[test]
fn a_symlinked_owner_folder_is_refused_in_every_spelling() {
    let table = Table::new();
    let made = table.meta(&table.ws, "git worktree create g --repo a", &[]);
    assert!(made.status.success(), "{}", text(&made));
    let owners = table.ws.join(".worktrees/.meta-sets");
    let moved = table.sandbox.work().join("owners");
    fs::rename(&owners, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &owners).unwrap();
    // exec has one spelling: its selection is meta's own
    let out = table.meta(
        &table.ws,
        "--include a git worktree exec g -- touch ran",
        &[],
    );
    assert!(!out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("is a symlink"), "{}", text(&out));
    assert!(!table.ws.join(".worktrees/g/a/ran").exists());
    let (a, b) = ("git worktree add g --repo b", "git worktree add --repo b g");
    let (first, second) = (table.meta(&table.ws, a, &[]), table.meta(&table.ws, b, &[]));
    let said = format!("{a}: {}\n{b}: {}", text(&first), text(&second));
    assert!(
        !first.status.success() && !second.status.success(),
        "{said}"
    );
    assert_eq!(first.status.code(), second.status.code(), "{said}");
    assert!(!table.ws.join(".worktrees/g/b").exists(), "{said}");
    let out = table.meta(&table.ws, "git worktree add --repo b g", &[]);
    assert!(text(&out).contains("is a symlink"), "{}", text(&out));
}

/// `--include` chooses among the checkouts of an intact set: the one not
/// selected runs nothing.
#[test]
fn a_selected_exec_runs_only_in_the_selected_checkout() {
    let table = Table::new();
    let out = table.meta(
        &table.ws,
        "--include a git worktree exec s -- touch ran",
        &[],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(table.ws.join(".worktrees/s/a/ran").exists());
    assert!(!table.ws.join(".worktrees/s/b/ran").exists());
}

/// Outside any workspace, `exec` finds the set where sets go
/// (`META_WORKTREES`) and runs in every checkout of it.
#[test]
fn exec_outside_a_workspace_runs_in_every_checkout_of_the_set() {
    let table = Table::new();
    let sets = table.sandbox.work().join("sets");
    let env = [("META_WORKTREES", sets.as_path())];
    let made = table.meta(&table.ws, "git worktree create o --repo a --repo b", &env);
    assert!(made.status.success(), "{}", text(&made));
    let elsewhere = table.sandbox.work().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let out = table.meta(&elsewhere, "git worktree exec o -- touch f", &env);
    assert!(out.status.success(), "{}", text(&out));
    for project in ["a", "b"] {
        let made = sets.join("o").join(project).join("f");
        assert!(made.is_file(), "{project}: {}", text(&out));
    }
}
