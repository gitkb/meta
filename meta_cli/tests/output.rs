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

/// What a run printed from its summary on.
fn summary(text: &str) -> &str {
    text.split_once("Summary:").map_or("", |(_, rest)| rest)
}

/// A captured failure's summary says what went wrong, not only the last
/// line git wrote; `--full-errors` cuts nothing. A live one's output is the
/// user's, already on screen.
#[test]
fn the_summary_says_why_a_captured_command_failed() {
    let sandbox = Sandbox::new();
    let root = workspace(&sandbox);
    let run = |options: &[&str], script: &str| {
        let mut args = options.to_vec();
        args.extend(["-i", "a", "exec", "--", "sh", "-c", script]);
        combined(&sandbox, &root, &args)
    };

    let hints = "echo 'fatal: the real reason' >&2; echo >&2; echo 'and the repository exists.' >&2; echo 'CONFLICT (content)'; exit 1";
    let text = run(&["--parallel"], hints);
    let said = summary(&text);
    for line in [
        "\n    fatal: the real reason\n    and the repository exists.\n",
        "\n  stdout:\n    CONFLICT (content)\n",
    ] {
        assert!(said.contains(line), "{line:?} in {said}");
    }

    let long = r#"i=1; while [ $i -le 30 ]; do echo "line-$i." >&2; i=$((i+1)); done; printf '\033[31mred\033[0m\n' >&2; printf '%0400d\n' 7 >&2; exit 1"#;
    let text = run(&["--parallel"], long);
    let said = summary(&text);
    assert!(
        said.contains("line-5.") && !said.contains("line-6."),
        "{said}"
    );
    assert!(said.contains("… 23 more lines"), "{said}");
    assert!(said.contains("\n  red\n"), "{said}");
    assert!(said.contains("… 100 more characters …"), "{said}");
    let text = run(&["--parallel", "--full-errors"], long);
    let said = summary(&text);
    assert!(
        (1..=30).all(|i| said.contains(&format!("line-{i}."))),
        "{said}"
    );
    assert!(
        !said.contains("more lines") && !said.contains("more characters"),
        "{said}"
    );
    assert!(said.contains(&format!("{}7", "0".repeat(399))), "{said}");

    let text = run(&["--sequential"], hints);
    let said = summary(&text);
    assert!(said.contains("(exit code 1): output is above"), "{said}");
    assert!(!said.contains("\n    fatal"), "{said}");
}

/// Past five failures, one line names every repo that failed.
#[test]
fn many_failures_end_with_one_line_naming_them() {
    let sandbox = Sandbox::new();
    let root = sandbox.work().join("ws");
    let names = ["a", "b", "c", "d", "e", "f"];
    names
        .iter()
        .fold(sandbox.workspace(&root), |ws, name| ws.repo(name))
        .build();
    let script = r#"echo "why-$(basename "$PWD")" >&2; exit 1"#;
    let args = [
        "--parallel",
        "-i",
        "a,b,c,d,e,f",
        "exec",
        "--",
        "sh",
        "-c",
        script,
    ];
    let text = combined(&sandbox, &root, &args);
    let line = "✗ Failed (6): ./a, ./b, ./c, ./d, ./e, ./f";
    assert!(summary(&text).contains(line), "{text}");
    let five = [
        "--parallel",
        "-i",
        "a,b,c,d,e",
        "exec",
        "--",
        "sh",
        "-c",
        script,
    ];
    assert!(!combined(&sandbox, &root, &five).contains("Failed ("));
}

#[test]
fn a_failed_commands_stderr_is_shown_with_its_repo_unless_silenced() {
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
                    match mode {
                        "--silent" => assert!(!text.contains(&why), "{say}:\n{text}"),
                        _ => assert!(tied(&text, &why, &format!("./{repo}")), "{say}:\n{text}"),
                    }
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

/// A silenced command is never stopped for what it writes: it goes nowhere,
/// nothing of it is held, and the command runs to its end.
#[test]
fn a_silenced_command_runs_to_its_end_however_much_it_writes() {
    let sandbox = Sandbox::new();
    let root = workspace(&sandbox);
    let chatty = r#"head -c 17000000 /dev/zero | tr '\0' x >&2; touch finished; exit 1"#;
    // in meta's default order, at once, and one at a time
    for order in [
        None,
        Some("--parallel"),
        Some("--sequential"),
        Some("--json"),
    ] {
        let mut args = vec!["--silent"];
        args.extend(order);
        args.extend(["-i", "a,b", "exec", "--", "sh", "-c", chatty]);
        let out = meta(&sandbox, &root, &args).output().unwrap();
        assert!(!out.status.success());
        let text = String::from_utf8_lossy(&out.stderr);
        for repo in ["a", "b"] {
            let finished = root.join(repo).join("finished");
            assert!(finished.exists(), "{order:?}: {repo} was stopped");
            std::fs::remove_file(finished).unwrap();
        }
        assert!(!text.contains("16 MiB"), "{order:?}: {}", text.len());
        if order == Some("--json") {
            let rows = documents(&out).pop().unwrap()["results"].clone();
            for row in rows.as_array().unwrap() {
                assert_eq!(row["exit_code"], 1, "{row}");
                assert_eq!(row["stderr"], "", "{row}");
            }
        }
    }
}

/// Silenced, in every order, nothing of a command is shown: the one line is
/// the closing error, naming the repos that failed and nothing they wrote.
#[test]
fn a_silenced_run_ends_with_the_repos_that_failed_and_nothing_they_wrote() {
    let sandbox = Sandbox::new();
    let root = workspace(&sandbox);
    let failing = "echo out-text; echo err-text >&2; exit 1";
    for order in ["--parallel", "--sequential"] {
        for (repos, closing) in [
            ("a", "Error: 1 command failed: a"),
            ("a,b", "Error: 2 commands failed: a, b"),
        ] {
            let args = [
                "--silent", order, "-i", repos, "exec", "--", "sh", "-c", failing,
            ];
            let out = meta(&sandbox, &root, &args).output().unwrap();
            assert!(!out.status.success());
            assert_eq!(String::from_utf8_lossy(&out.stdout), "", "{order}");
            let text = String::from_utf8_lossy(&out.stderr);
            assert_eq!(text.trim_end(), closing, "{order}");
        }
    }
}

/// A silenced command that leaves a program running in the background,
/// holding its stderr, doesn't keep meta waiting for that program.
#[test]
fn a_silenced_command_that_leaves_a_background_program_ends_meta_promptly() {
    let sandbox = Sandbox::new();
    let root = workspace(&sandbox);
    let pids = sandbox.work().join("background.pid");
    let script = format!(r#"sleep 60 & echo $! >> "{}"; exit 1"#, pids.display());
    let started = std::time::Instant::now();
    let out = meta(
        &sandbox,
        &root,
        &["--silent", "-i", "a", "exec", "--", "sh", "-c", &script],
    )
    .output()
    .unwrap();
    let took = started.elapsed();
    for pid in std::fs::read_to_string(&pids)
        .unwrap_or_default()
        .split_whitespace()
    {
        if let Some(pid) = pid.parse().ok().and_then(rustix::process::Pid::from_raw) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
        }
    }
    assert!(!out.status.success());
    assert!(took < std::time::Duration::from_secs(20), "took {took:?}");
}

/// `--version --json` is the document atc and git-kb print, which their
/// callers read field by field, and `--version` stays one line of text.
#[test]
fn version_json_has_exactly_the_six_build_fields() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    for args in [
        ["--version", "--json"],
        ["--json", "--version"],
        ["-V", "--json"],
    ] {
        let out = meta(&sandbox, &root, &args).output().unwrap();
        assert!(out.status.success(), "{args:?}");
        let documents = documents(&out);
        assert_eq!(documents.len(), 1, "{args:?}");
        let build = documents.first().unwrap();
        let mut fields: Vec<&str> = build
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            ["branch", "built", "commit", "dirty", "source", "version"]
        );
        assert_eq!(build["version"], include_str!("../../VERSION").trim());
        // null in a build from outside a git checkout
        assert!(build["commit"].is_null() || build["commit"].as_str().unwrap().len() == 40);
        assert!(build["dirty"].is_null() || build["dirty"].is_boolean());
        chrono::DateTime::parse_from_rfc3339(build["built"].as_str().unwrap()).unwrap();
    }

    let out = meta(&sandbox, &root, &["--version"]).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("meta "), "{text}");
    assert_eq!(text.lines().count(), 1, "{text}");
}
