//! The loop binary: which directories it runs in, its config and its aliases.
#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

#[test]
fn an_exclusion_holds_for_directories_named_outright() {
    let tmp = tempfile::tempdir().unwrap();
    for dir in ["a", "b"] {
        std::fs::create_dir(tmp.path().join(dir)).unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_loop"))
        .args(["-i", "a", "-i", "b", "-e", "a", "--", "pwd"])
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let shown = String::from_utf8_lossy(&out.stdout);
    // pwd's lines, among loop's own
    let root = tmp.path().canonicalize().unwrap();
    let ran: Vec<_> = shown
        .lines()
        .filter_map(|line| std::path::Path::new(line.trim()).strip_prefix(&root).ok())
        .collect();
    assert_eq!(ran, [std::path::Path::new("b")], "{shown}");

    // nothing left is said, not run silently
    let out = Command::new(env!("CARGO_BIN_EXE_loop"))
        .args(["-i", "a", "-e", "a", "--", "pwd"])
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("No directories left after exclusions"));
    // nor when loop finds the directories itself
    let empty = tmp.path().join("b");
    let out = Command::new(env!("CARGO_BIN_EXE_loop"))
        .args(["-e", ".", "--", "pwd"])
        .current_dir(&empty)
        .env("HOME", tmp.path())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("No directories left after exclusions"));
}

/// A sandbox: a working directory `ws` and a separate `home`.
struct Sandbox {
    _tmp: tempfile::TempDir,
    ws: PathBuf,
    home: PathBuf,
}

fn sandbox(dirs: &[&str]) -> Sandbox {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (ws, home) = (root.join("ws"), root.join("home"));
    for dir in dirs {
        fs::create_dir_all(ws.join(dir)).unwrap();
    }
    fs::create_dir_all(&home).unwrap();
    Sandbox {
        _tmp: tmp,
        ws,
        home,
    }
}

impl Sandbox {
    fn run(&self, args: &[&str], stdin: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_loop"))
            .args(args)
            .current_dir(&self.ws)
            .env("HOME", &self.home)
            .env("SHELL", self.home.join("shell"))
            .env_remove("XDG_CONFIG_HOME")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // loop may exit before it reads its input
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        child.wait_with_output().unwrap()
    }

    /// The directories `pwd` ran in, from the working directory
    fn ran_in(&self, args: &[&str]) -> Vec<PathBuf> {
        let out = self.run(&[args, &["--", "pwd"]].concat(), "");
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| Path::new(line.trim()).strip_prefix(&self.ws).ok())
            .map(Path::to_path_buf)
            .collect()
    }
}

#[test]
fn an_empty_looprc_selects_what_no_looprc_does() {
    let ws = sandbox(&["a", ".git"]);
    let without = ws.ran_in(&[]);
    assert!(!without.contains(&PathBuf::from(".git")), "{without:?}");
    fs::write(ws.ws.join(".looprc"), "{}").unwrap();
    assert_eq!(ws.ran_in(&[]), without);
}

#[test]
fn a_config_file_loop_cannot_use_is_an_error() {
    let ws = sandbox(&["a"]);
    let out = ws.run(&["-c", "missing.json", "--", "pwd"], "");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing.json"));

    fs::write(ws.ws.join(".looprc"), r#"{"concurrency": 4}"#).unwrap();
    let out = ws.run(&["--", "pwd"], "");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("concurrency") && stderr.contains(".looprc"),
        "{stderr}"
    );
}

#[test]
fn a_setting_in_the_file_holds_when_no_flag_is_given() {
    let ws = sandbox(&["a"]);
    fs::write(ws.ws.join(".looprc"), r#"{"silent": true}"#).unwrap();
    let out = ws.run(&["--", "echo", "SAID"], "");
    assert!(out.status.success(), "{out:?}");
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("SAID"),
        "{out:?}"
    );
}

#[test]
fn include_and_exclude_take_comma_lists() {
    let ws = sandbox(&["a", "b", "c"]);
    assert_eq!(
        ws.ran_in(&["-i", "a,b,c", "-e", "c,x"]),
        ["a", "b"].map(PathBuf::from)
    );
    let help = ws.run(&["--help"], "");
    assert!(
        String::from_utf8_lossy(&help.stdout).contains("Usage: loop "),
        "{help:?}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn a_directory_name_that_is_not_utf8_is_where_the_command_runs() {
    use std::os::unix::ffi::OsStrExt;
    let ws = sandbox(&[]);
    // a byte 0xff name, and the one it reads as when made into text
    let raw = ws.ws.join(std::ffi::OsStr::from_bytes(b"r\xff"));
    let lossy = ws.ws.join("r\u{fffd}");
    fs::create_dir_all(&raw).unwrap();
    fs::create_dir_all(&lossy).unwrap();
    let out = ws.run(&["-e", ".", "--", "touch", "here"], "");
    assert!(out.status.success(), "{out:?}");
    assert!(
        raw.join("here").exists(),
        "nothing ran in the 0xff directory"
    );
}

#[test]
fn local_aliases_win_over_global_ones() {
    let ws = sandbox(&["a"]);
    fs::write(
        ws.home.join(".looprc"),
        r#"{"aliases": {"meta-say": "echo GLOBAL", "only-global": "echo G2"}}"#,
    )
    .unwrap();
    fs::write(
        ws.ws.join(".looprc"),
        r#"{"aliases": {"meta-say": "echo LOCAL"}}"#,
    )
    .unwrap();
    fs::write(
        ws.ws.join("other.json"),
        r#"{"aliases": {"meta-say": "echo OTHER"}}"#,
    )
    .unwrap();
    let said = |args: &[&str]| {
        let out = ws.run(&[&["-i", "a"], args].concat(), "");
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert!(said(&["meta-say"]).contains("LOCAL"));
    assert!(said(&["only-global"]).contains("G2"));
    // -c names the local file
    assert!(said(&["-c", "other.json", "meta-say"]).contains("OTHER"));
}

/// A shell whose `alias` lists `q` with quotes inside its value, and `new`
fn fake_shell(ws: &Sandbox) {
    use std::os::unix::fs::PermissionsExt;
    let shell = ws.home.join("shell");
    fs::write(
        &shell,
        "#!/bin/sh\necho \"alias q='printf \\\"%s\\\" \\\"two words\\\"'\"\necho \"alias new=ls\"\n",
    )
    .unwrap();
    fs::set_permissions(&shell, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn importing_aliases_changes_only_the_aliases() {
    let ws = sandbox(&["a"]);
    fake_shell(&ws);
    let looprc = ws.home.join(".looprc");
    // a dotfile manager's symlink
    let target = ws.home.join("dotfiles-looprc");
    fs::write(
        &target,
        r#"{"parallel": true, "ignore": ["node_modules"], "aliases": {"old": "echo old"}}"#,
    )
    .unwrap();
    std::os::unix::fs::symlink("dotfiles-looprc", &looprc).unwrap();

    // cancelled: unchanged, and not a success
    let before = fs::read(&target).unwrap();
    let out = ws.run(&["--add-aliases-to-global-looprc"], "y\nn\n");
    assert!(!out.status.success(), "{out:?}");
    assert_eq!(fs::read(&target).unwrap(), before);
    // with a command: an error, and the command doesn't run
    let out = ws.run(
        &["--add-aliases-to-global-looprc", "touch", "ran"],
        "y\ny\n",
    );
    assert!(!out.status.success());
    assert!(!ws.ws.join("a/ran").exists() && !ws.ws.join("ran").exists());
    assert_eq!(fs::read(&target).unwrap(), before, "it imported anyway");

    let out = ws.run(&["--add-aliases-to-global-looprc"], "y\ny\n");
    assert!(out.status.success(), "{out:?}");
    assert!(
        !String::from_utf8_lossy(&out.stdout).contains("Starting"),
        "{out:?}"
    );
    assert!(fs::symlink_metadata(&looprc)
        .unwrap()
        .file_type()
        .is_symlink());
    let config: serde_json::Value = serde_json::from_slice(&fs::read(&target).unwrap()).unwrap();
    assert_eq!(
        config,
        serde_json::json!({
            "parallel": true,
            "ignore": ["node_modules"],
            "aliases": {"old": "echo old", "q": "printf \"%s\" \"two words\"", "new": "ls"},
        })
    );

    // a file loop can't read is reported, not replaced
    fs::write(&target, r#"{"aliases": {"x": 1}}"#).unwrap();
    let out = ws.run(&["--add-aliases-to-global-looprc"], "y\ny\n");
    assert!(!out.status.success());
    assert_eq!(fs::read(&target).unwrap(), br#"{"aliases": {"x": 1}}"#);
}
