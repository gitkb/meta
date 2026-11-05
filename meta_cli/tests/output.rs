//! What meta shows of the programs it runs: a failed one's stderr in every
//! output mode, and captured output that stays bounded.

#![expect(clippy::unwrap_used, reason = "test helpers: a panic is a failed test")]
// Unix only: the plugin here is a shell script
#![cfg(unix)]

use meta_plugin::testing::Sandbox;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A workspace with repos `a` and `b`, and a plugin `die` that writes to
/// stderr and exits 3 without an answer.
fn workspace(sandbox: &Sandbox) -> PathBuf {
    let root = sandbox.work().join("ws");
    sandbox.workspace(&root).repo("a").repo("b").build();
    let plugins = root.join(".meta/plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let plugin = plugins.join("meta-die");
    std::fs::write(
        &plugin,
        "#!/bin/sh\nif [ \"$1\" = --meta-plugin-info ]; then\n    echo '{\"name\":\"die\",\"version\":\"1\",\"commands\":[\"die\"]}'\nelse\n    echo plugin-boom >&2; exit 3\nfi\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&plugin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    root
}

/// meta with `args` in `root`: its stdout and stderr through one pipe, in
/// the order it wrote them.
fn combined(sandbox: &Sandbox, root: &Path, args: &[&str]) -> String {
    let (mut reader, writer) = std::io::pipe().unwrap();
    let mut command = meta(sandbox, root, args);
    command.stdout(writer.try_clone().unwrap()).stderr(writer);
    let mut child = command.spawn().unwrap();
    drop(command);
    let mut text = String::new();
    reader.read_to_string(&mut text).unwrap();
    assert!(!child.wait().unwrap().success(), "{text}");
    text
}

fn meta(sandbox: &Sandbox, root: &Path, args: &[&str]) -> Command {
    let mut command = sandbox.command(env!("CARGO_BIN_EXE_meta"));
    command.args(args).current_dir(root).stdin(Stdio::null());
    command
}

/// Each line of `json` (one document, or a stream of lines) parsed.
fn documents(out: &Output) -> Vec<serde_json::Value> {
    serde_json::Deserializer::from_slice(&out.stdout)
        .into_iter()
        .collect::<Result<_, _>>()
        .unwrap()
}

/// Whether every line of `text` that has `why` in it is about `repo`: it
/// names it, or the closest line above it that names a repo names it.
fn tied(text: &str, why: &str, repo: &str) -> bool {
    let named = |line: &str| ["./a", "./b"].into_iter().find(|r| line.contains(r));
    let mut last = None;
    let mut seen = false;
    for line in text.lines() {
        if line.contains(why) {
            seen = true;
            if named(line).or(last) != Some(repo) {
                return false;
            }
        }
        last = named(line).or(last);
    }
    seen
}

#[test]
fn a_failed_commands_stderr_is_shown_with_its_repo_in_every_output_mode() {
    let sandbox = Sandbox::new();
    let root = workspace(&sandbox);
    let failing = r#"echo "why-$(basename "$PWD")" >&2; exit 1"#;
    for order in ["--parallel", "--sequential"] {
        for mode in ["", "--silent", "--json", "--json-stream"] {
            let mut args: Vec<&str> = [mode, order]
                .into_iter()
                .filter(|a| !a.is_empty())
                .collect();
            let say = format!("{mode} {order}");
            let mut exec = args.clone();
            exec.extend(["-i", "a,b", "exec", "--", "sh", "-c", failing]);
            args.extend(["die", "now"]);
            if mode.starts_with("--json") {
                let out = meta(&sandbox, &root, &exec).output().unwrap();
                let rows: Vec<serde_json::Value> = documents(&out)
                    .into_iter()
                    .flat_map(|doc| match doc.get("envelope") {
                        Some(envelope) => envelope["results"].as_array().cloned().unwrap(),
                        None => match doc.get("event") {
                            Some(event) => vec![event["step"].clone()],
                            None => doc["results"].as_array().cloned().unwrap(),
                        },
                    })
                    .collect();
                for repo in ["a", "b"] {
                    assert!(
                        rows.iter().any(|row| row["project"] == repo
                            && row["stderr"] == format!("why-{repo}\n")
                            && row["error"]["code"] == "tool-failed"),
                        "{say}: {rows:#?}"
                    );
                }
                let out = meta(&sandbox, &root, &args).output().unwrap();
                let last = documents(&out).pop().unwrap();
                let error = last.get("envelope").unwrap_or(&last)["error"].clone();
                assert_eq!(error["code"], "plugin-failed", "{say}: {last}");
                let message = error["message"].as_str().unwrap();
                assert!(
                    message.contains("meta-die") && message.contains("plugin-boom"),
                    "{say}: {message}"
                );
            } else {
                let text = combined(&sandbox, &root, &exec);
                for repo in ["a", "b"] {
                    let why = format!("why-{repo}");
                    assert!(tied(&text, &why, &format!("./{repo}")), "{say}:\n{text}");
                }
                assert!(
                    combined(&sandbox, &root, &args).contains("plugin-boom"),
                    "{say}"
                );
            }
        }
    }
}

/// A command that never stops writing, with meta's stdout closed (`| head
/// -1`): meta ends promptly, as a closed stdout ends it, and holds no more
/// of the command's output than its limit meanwhile (a data limit makes
/// meta fail rather than fill the machine if it did).
#[cfg(target_os = "linux")]
#[test]
fn a_closed_stdout_ends_meta_promptly_with_an_endless_command() {
    use std::time::{Duration, Instant};
    let sandbox = Sandbox::new();
    let root = workspace(&sandbox);
    for mode in [
        &["--parallel"][..],
        &["--sequential"],
        &["--json"],
        &["--json-stream"],
        &["--json", "--sequential"],
    ] {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let mut command = sandbox.command("sh");
        command
            .args(["-c", r#"ulimit -d 1048576; exec "$0" "$@""#])
            .arg(env!("CARGO_BIN_EXE_meta"))
            .args(mode)
            .args(["-i", "a,b", "exec", "--", "yes"])
            .current_dir(&root)
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(Stdio::piped());
        let mut child = command.spawn().unwrap();
        drop(command);
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                child.kill().unwrap();
            }
            assert!(
                Instant::now() <= deadline,
                "{mode:?}: meta still runs with its stdout closed"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert_eq!(status.code(), Some(141), "{mode:?}: {stderr}");
    }
}

/// A silenced command (`--silent`, one at a time) is never stopped for what
/// it writes, as before meta kept its stderr: it runs to its end, and only
/// the end of its stderr is kept to say why it failed.
#[test]
fn a_silenced_command_runs_to_its_end_however_much_it_writes() {
    let sandbox = Sandbox::new();
    let root = workspace(&sandbox);
    let chatty = r#"head -c 17000000 /dev/zero | tr '\0' x >&2; echo; echo "last-$(basename "$PWD")" >&2; touch finished; exit 1"#;
    let out = meta(
        &sandbox,
        &root,
        &[
            "--silent",
            "--sequential",
            "-i",
            "a,b",
            "exec",
            "--",
            "sh",
            "-c",
            chatty,
        ],
    )
    .output()
    .unwrap();
    assert!(!out.status.success());
    let text = String::from_utf8_lossy(&out.stderr);
    for repo in ["a", "b"] {
        assert!(
            root.join(repo).join("finished").exists(),
            "{repo} was stopped"
        );
        assert!(
            text.contains(&format!("last-{repo}")),
            "{repo}'s end is shown"
        );
    }
    assert!(!text.contains("16 MiB"), "{}", text.len());
}
