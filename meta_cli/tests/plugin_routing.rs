//! `meta exec` runs commands as typed; a plugin file `meta-<word>` gets only
//! `meta <word> ...`, and only it is started for that command.

#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]
// Unix only: the plugins here are shell scripts
#![cfg(unix)]

use meta_plugin::testing::Sandbox;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// A workspace with repos `a` and `b`, a plugin `meta-touchy` on PATH that
/// claims `touch` and logs each request it receives instead of running it,
/// and a plugin `meta-spy` that logs every time it is started.
fn workspace(sandbox: &Sandbox, root: &Path) {
    fs::write(
        root.join(".meta.yaml"),
        r#"{"projects":{"a":"https://example.invalid/a.git","b":"https://example.invalid/b.git"}}"#,
    )
    .unwrap();
    // projects are repositories: meta runs nothing in a plain folder
    for project in ["a", "b"] {
        fs::create_dir_all(root.join(project)).unwrap();
        sandbox.git(&root.join(project), &["init", "-q"]);
    }
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let plugin = bin.join("meta-touchy");
    fs::write(
        &plugin,
        format!(
            r#"#!/bin/sh
case "$1" in
  --meta-plugin-info) echo '{{"name":"touchy","version":"1.0.0","commands":["touch"]}}' ;;
  --meta-plugin-exec) cat "$META_REQUEST" >> "{}"; echo '{{"results":{{"records":[]}}}}' > "$META_RESPONSE" ;;
esac
"#,
            root.join("plugin.log").display()
        ),
    )
    .unwrap();
    fs::set_permissions(&plugin, fs::Permissions::from_mode(0o755)).unwrap();
    script(
        &bin.join("meta-spy"),
        &format!(
            r#"echo "$1" >> "{}"
[ "$1" = --meta-plugin-info ] && echo '{{"name":"spy","version":"1.0.0","commands":["spy"]}}'
exit 0"#,
            root.join("spy.log").display()
        ),
    );
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// What meta-spy was started for, one line each
fn spy_starts(root: &Path) -> String {
    fs::read_to_string(root.join("spy.log")).unwrap_or_default()
}

fn meta(sandbox: &Sandbox, root: &Path, args: &[&str]) {
    let output = run_meta(sandbox, root, args);
    assert!(
        output.status.success(),
        "meta {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_meta(sandbox: &Sandbox, root: &Path, args: &[&str]) -> std::process::Output {
    sandbox
        .command(env!("CARGO_BIN_EXE_meta"))
        .args(args)
        .current_dir(root)
        // Only this workspace's plugins: none installed on the caller's PATH
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .output()
        .unwrap()
}

#[test]
fn exec_runs_a_plugin_claimed_command_verbatim() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);

    meta(&sandbox, &root, &["exec", "--", "touch", "ran"]);

    for repo in [&root, &root.join("a"), &root.join("b")] {
        assert!(repo.join("ran").exists(), "not run in {}", repo.display());
    }
    assert!(
        !root.join("plugin.log").exists(),
        "meta exec handed the command to the plugin"
    );
}

#[test]
fn a_plugin_serves_the_word_in_its_file_name_only() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);

    meta(&sandbox, &root, &["touchy", "ran"]);

    let request = fs::read_to_string(root.join("plugin.log")).unwrap();
    assert!(request.contains(r#""command":"touchy""#), "{request}");
    assert!(request.contains(r#""args":["ran"]"#), "{request}");
    assert!(
        !root.join("ran").exists(),
        "meta ran the plugin's command raw"
    );

    // `touch` is claimed by meta-touchy, but no file is named meta-touch
    fs::remove_file(root.join("plugin.log")).unwrap();
    let output = run_meta(&sandbox, &root, &["touch", "ran"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unrecognized command 'touch'"), "{stderr}");
    assert!(!root.join("plugin.log").exists(), "meta-touchy got `touch`");
}

#[test]
fn only_the_plugin_a_command_names_is_started() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);

    meta(&sandbox, &root, &["touchy", "ran"]);
    // meta-touchy prints nothing for --help: its info stands in
    let help = run_meta(&sandbox, &root, &["touchy", "--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("Commands:\n  touch"));
    meta(&sandbox, &root, &["exec", "--", "true"]);
    meta(&sandbox, &root, &["context"]);
    let typo = run_meta(&sandbox, &root, &["touhcy"]);
    assert!(!typo.status.success());
    let stderr = String::from_utf8_lossy(&typo.stderr);
    assert!(
        stderr.contains("the ones plugins add too: meta --help"),
        "{stderr}"
    );
    assert_eq!(spy_starts(&root), "", "an unrelated plugin was started");

    // Help and the plugin list ask every plugin, and name each by its file
    let help = run_meta(&sandbox, &root, &["--help"]);
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("plugin: spy v1.0.0"), "{help}");
    assert!(help.contains("plugin: touchy v1.0.0"), "{help}");
    let list = run_meta(&sandbox, &root, &["--json", "plugin", "list"]);
    let list: meta_plugin::Envelope = serde_json::from_slice(&list.stdout).unwrap();
    let names: Vec<_> = list
        .results
        .iter()
        .map(|row| row.data.as_ref().unwrap()["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"touchy") && names.contains(&"spy"),
        "{names:?}"
    );
    assert_eq!(
        spy_starts(&root),
        "--meta-plugin-info\n--meta-plugin-info\n"
    );
}

#[test]
fn a_workspace_plugin_comes_before_one_on_path() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);
    let local = root.join(".meta/plugins/meta-touchy");
    fs::create_dir_all(local.parent().unwrap()).unwrap();
    script(
        &local,
        &format!(
            r#"[ "$1" = --meta-plugin-info ] && echo '{{"name":"touchy","version":"2.0.0","commands":[]}}'
[ "$1" = --meta-plugin-exec ] && cat "$META_REQUEST" > "{}" && echo '{{"message":""}}' > "$META_RESPONSE"
exit 0"#,
            root.join("local.log").display()
        ),
    );

    meta(&sandbox, &root, &["touchy", "ran"]);

    assert!(
        root.join("local.log").exists(),
        "the workspace plugin didn't run"
    );
    assert!(!root.join("plugin.log").exists(), "the one on PATH ran");
}

#[test]
fn a_called_plugin_that_does_not_answer_fails_naming_it() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);
    script(&root.join("bin/meta-slow"), "exec sleep 60");

    let started = std::time::Instant::now();
    let output = run_meta(&sandbox, &root, &["slow", "x"]);

    assert!(started.elapsed() < std::time::Duration::from_secs(8));
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("meta-slow did not answer --meta-plugin-info within 5 seconds"),
        "{stderr}"
    );
    assert!(!stderr.contains("unrecognized command"), "{stderr}");
    assert_eq!(spy_starts(&root), "", "an unrelated plugin was started");
}

#[test]
fn a_plan_meta_cannot_read_fails_and_names_the_plugin() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);
    // A plugin that plans `cmd` strings instead of steps
    let plugin = root.join("bin").join("meta-oldplan");
    fs::write(
        &plugin,
        r#"#!/bin/sh
case "$1" in
  --meta-plugin-info) echo '{"name":"oldplan","version":"1.0.0","commands":["oldplan"]}' ;;
  --meta-plugin-exec) echo '{"plan":{"commands":[{"dir":".","cmd":"touch ran"}]}}' > "$META_RESPONSE" ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&plugin, fs::Permissions::from_mode(0o755)).unwrap();

    let output = run_meta(&sandbox, &root, &["oldplan", "run"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "an unreadable plan passed as success"
    );
    assert!(stderr.contains("an answer that is no response"), "{stderr}");
    assert!(stderr.contains(&plugin.display().to_string()), "{stderr}");
    assert!(!root.join("ran").exists());
}

#[test]
fn a_plan_with_max_parallel_zero_is_refused_before_any_of_it_runs() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);
    let plugin = root.join("bin").join("meta-zero");
    fs::write(
        &plugin,
        r#"#!/bin/sh
case "$1" in
  --meta-plugin-info) echo '{"name":"zero","version":"1.0.0","commands":["zero"]}' ;;
  --meta-plugin-exec) echo '{"plan":{"steps":[{"dir":".","argv":["touch","first"]},{"dir":".","argv":["touch","ran"]}],"max_parallel":0}}' > "$META_RESPONSE" ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&plugin, fs::Permissions::from_mode(0o755)).unwrap();

    let output = run_meta(&sandbox, &root, &["zero", "run"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "max_parallel 0 ran unlimited");
    assert!(stderr.contains("max_parallel"), "{stderr}");
    assert!(!root.join("first").exists() && !root.join("ran").exists());
}

#[test]
fn a_planned_commands_env_reaches_the_command_meta_runs() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);
    let plan = serde_json::json!({"plan": {"steps": [{
        "dir": root.join("a"),
        "argv": ["sh", "-c", "printf '%s' \"$X\" > from-plan.txt"],
        "env": {"X": "from-plan"}
    }]}});
    script(
        &root.join("bin/meta-envplan"),
        &format!(
            r#"case "$1" in
  --meta-plugin-info) echo '{{"name":"envplan","version":"1","commands":["envplan"]}}' ;;
  --meta-plugin-exec) cat > "$META_RESPONSE" <<'PLAN'
{plan}
PLAN
    ;;
esac"#
        ),
    );

    meta(&sandbox, &root, &["envplan", "run"]);

    assert_eq!(
        fs::read_to_string(root.join("a/from-plan.txt")).unwrap(),
        "from-plan"
    );
    assert!(!root.join("from-plan.txt").exists());
}

#[test]
fn a_broken_looprc_stops_only_what_runs_through_loop() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);
    // A malformed workspace .looprc must stop commands using loop.
    fs::write(root.join(".looprc"), r#"{"bogus": 1}"#).unwrap();

    // the plugin answers without a plan: loop never runs
    meta(&sandbox, &root, &["touchy", "ran"]);
    assert!(fs::read_to_string(root.join("plugin.log"))
        .unwrap()
        .contains("touchy"));

    let output = run_meta(&sandbox, &root, &["exec", "--", "true"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains(".looprc") && stderr.contains("bogus"),
        "{stderr}"
    );
}

/// The selection stays a path: a directory whose name isn't UTF-8 is never
/// taken for its U+FFFD twin (macOS file systems refuse such names)
#[cfg(target_os = "linux")]
#[test]
fn a_directory_name_that_is_not_utf8_runs_in_that_directory() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    let raw = root.join(OsStr::from_bytes(b"ws\xff"));
    let twin = root.join("ws\u{fffd}");
    for ws in [&raw, &twin] {
        fs::create_dir_all(ws.join("a")).unwrap();
        sandbox.git(&ws.join("a"), &["init", "-q"]);
        fs::write(ws.join(".meta.yaml"), "projects:\n  a: git@x:o/a.git\n").unwrap();
    }
    let out = sandbox
        .command(env!("CARGO_BIN_EXE_meta"))
        .args(["exec", "--", "touch", "here"])
        .current_dir(&raw)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(raw.join("here").is_file() && raw.join("a/here").is_file());
    assert!(!twin.join("here").exists() && !twin.join("a/here").exists());
    // the envelope names the folder by its bytes, which lead back to it
    let out = sandbox
        .command(env!("CARGO_BIN_EXE_meta"))
        .args(["--json", "exec", "--", "true"])
        .current_dir(&raw)
        .output()
        .unwrap();
    let json: meta_plugin::Envelope = serde_json::from_slice(&out.stdout).unwrap();
    let dirs: Vec<std::path::PathBuf> = json
        .results
        .iter()
        .map(|row| meta_plugin::wire_path::from_bytes(row.path_bytes.as_deref().unwrap()).unwrap())
        .collect();
    assert_eq!(dirs, [raw.clone(), raw.join("a")]);
}

/// A plugin `meta-<word>` that answers every call with `response` (JSON)
/// and keeps the request it got in `<word>.request.json`.
fn answering(root: &Path, word: &str, response: &serde_json::Value) {
    script(
        &root.join(format!("bin/meta-{word}")),
        &format!(
            r#"case "$1" in
  --meta-plugin-info) echo '{{"name":"{word}","version":"1","commands":["{word}"]}}' ;;
  --meta-plugin-exec) cp "$META_REQUEST" "{}"; cat > "$META_RESPONSE" <<'ANSWER'
{response}
ANSWER
  ;;
esac"#,
            root.join(format!("{word}.request.json")).display()
        ),
    );
}

fn envelope(output: &std::process::Output) -> meta_plugin::Envelope {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "not an envelope ({e}): {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn every_kind_of_answer_is_one_envelope_under_json() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);
    let a = root.join("a").display().to_string();
    let answers = [
        (
            serde_json::json!({"plan": {"steps": [{"project": "a", "dir": a, "argv": ["true"]}]}, "warnings": [{"message": "w"}]}),
            true,
        ),
        (
            serde_json::json!({"results": {"records": [{"project": "a", "data": {"n": 1}}]}, "skipped": [{"project": "b", "reason": "no x"}]}),
            true,
        ),
        (serde_json::json!({"message": "words"}), true),
        (
            serde_json::json!({"error": {"code": "dirty", "message": "a has changes"}}),
            false,
        ),
    ];
    for (answer, success) in answers {
        answering(&root, "kind", &answer);
        let out = run_meta(&sandbox, &root, &["--json", "kind", "go"]);
        let got = envelope(&out);
        assert_eq!(got.success, success, "{answer}: {got:?}");
        assert_eq!(out.status.success(), success, "{answer}");
        assert_eq!(got.command, "kind go", "{got:?}");
        match answer
            .as_object()
            .and_then(|o| o.keys().next())
            .map(String::as_str)
        {
            Some("plan") => {
                assert_eq!(got.results.len(), 1, "{got:?}");
                assert_eq!(got.results[0].exit_code, Some(0));
                assert_eq!(got.warnings[0].message, "w");
            }
            Some("results") => {
                assert_eq!(got.results[0].data, Some(serde_json::json!({"n": 1})));
                assert_eq!(got.skipped[0].project, "b");
            }
            Some("message") => assert_eq!(got.message.as_deref(), Some("words")),
            _ => assert_eq!(got.error.unwrap().code, meta_plugin::ErrorCode::Dirty),
        }
    }
}

#[test]
fn the_config_meta_is_given_is_the_one_a_plugin_runs_in() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);
    fs::create_dir_all(root.join("other")).unwrap();
    sandbox.git(&root.join("other"), &["init", "-q"]);
    fs::write(
        root.join("other.yaml"),
        r#"{"projects":{"other":"https://example.invalid/other.git"}}"#,
    )
    .unwrap();
    answering(&root, "seen", &serde_json::json!({"message": ""}));

    // from inside a project: the config is looked for upwards
    let out = sandbox
        .command(env!("CARGO_BIN_EXE_meta"))
        .args(["--config", "other.yaml", "seen", "go"])
        .current_dir(root.join("a"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let request: meta_plugin::Request =
        serde_json::from_slice(&fs::read(root.join("seen.request.json")).unwrap()).unwrap();
    let workspace = request.workspace.unwrap();
    assert_eq!(workspace.config.file_name().unwrap(), "other.yaml");
    let paths: Vec<&str> = request.projects.iter().map(|p| p.path.as_str()).collect();
    assert_eq!(paths, [".", "other"]);
    assert_eq!(request.cwd, root.join("a"));
}

#[test]
fn fields_keep_only_the_named_fields_of_each_row() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);
    let out = run_meta(
        &sandbox,
        &root,
        &["--fields", "project", "--json", "exec", "--", "true"],
    );
    let shown: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        shown["results"],
        serde_json::json!([{"project": "."}, {"project": "a"}, {"project": "b"}])
    );
    assert_eq!(shown["success"], true);
    // without --json there is nothing to pick from
    let out = run_meta(
        &sandbox,
        &root,
        &["--fields", "project", "exec", "--", "true"],
    );
    assert_eq!(out.status.code(), Some(2));
}

/// A plugin `meta-<word>` running `body` for a call (sh; `$META_EVENTS` is
/// where its events go, `$META_RESPONSE` its answer).
fn streaming(root: &Path, word: &str, body: &str) {
    script(
        &root.join(format!("bin/meta-{word}")),
        &format!(
            r#"case "$1" in
  --meta-plugin-info) echo '{{"name":"{word}","version":"1","commands":["{word}"]}}' ;;
  --meta-plugin-exec)
{body}
  ;;
esac"#
        ),
    );
}

#[test]
fn a_record_a_plugin_streams_is_shown_before_it_ends() {
    use std::io::BufRead;
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);
    let go = root.join("go");
    streaming(
        &root,
        "live",
        &format!(
            r#"printf '%s\n' '{{"type":"record","project":"a","data":"first match"}}' >> "$META_EVENTS"
i=0; while [ ! -e "{}" ] && [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done
echo '{{"message":""}}' > "$META_RESPONSE""#,
            go.display()
        ),
    );
    let mut meta = sandbox
        .command(env!("CARGO_BIN_EXE_meta"))
        .args(["live", "go"])
        .current_dir(&root)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = std::io::BufReader::new(meta.stdout.take().unwrap()).lines();
    let seen = lines.find(|line| line.as_ref().unwrap().contains("first match"));
    // shown while the plugin still waits for the go
    let running = meta.try_wait().unwrap().is_none() && !go.exists();
    fs::write(&go, "").unwrap();
    assert!(seen.is_some() && running);
    assert!(meta.wait().unwrap().success());
}

#[test]
fn json_folds_what_a_plugin_streamed_into_its_one_envelope() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);
    streaming(
        &root,
        "folds",
        r#"printf '%s\n' '{"type":"record","project":"a","data":1}' '{"type":"progress","done":1,"total":2}' '{"type":"warning","message":"slow"}' >> "$META_EVENTS"
echo '{"results":{"records":[{"project":"b","data":2}]}}' > "$META_RESPONSE""#,
    );
    let got = envelope(&run_meta(&sandbox, &root, &["--json", "folds", "go"]));
    let data: Vec<_> = got
        .results
        .iter()
        .map(|r| r.data.clone().unwrap())
        .collect();
    assert_eq!(
        data,
        [serde_json::json!(1), serde_json::json!(2)],
        "{got:?}"
    );
    assert_eq!(got.warnings[0].message, "slow");
    // a step is meta's: a plugin that sends one fails
    streaming(
        &root,
        "steps",
        r#"printf '%s\n' '{"type":"step","project":"a"}' >> "$META_EVENTS"; echo '{"message":""}' > "$META_RESPONSE""#,
    );
    let got = envelope(&run_meta(&sandbox, &root, &["--json", "steps", "go"]));
    assert_eq!(
        got.error.unwrap().code,
        meta_plugin::ErrorCode::PluginFailed
    );
}

#[test]
fn json_stream_is_a_line_per_event_then_the_envelope_json_prints() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);
    let line = ["exec", "--", "true"];
    let streamed = run_meta(&sandbox, &root, &[&["--json-stream"][..], &line].concat());
    let lines: Vec<serde_json::Value> = String::from_utf8_lossy(&streamed.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let (last, events) = lines.split_last().unwrap();
    let steps: Vec<&str> = events
        .iter()
        .map(|e| {
            assert_eq!(e["event"]["type"], "step", "{e}");
            e["event"]["project"].as_str().unwrap()
        })
        .collect();
    assert_eq!(steps.len(), 3, "{lines:?}");
    let json = envelope(&run_meta(
        &sandbox,
        &root,
        &[&["--json"][..], &line].concat(),
    ));
    let last: meta_plugin::Envelope = serde_json::from_value(last["envelope"].clone()).unwrap();
    assert_eq!(last, json);
}

/// A set made by another provider than git (a jj set: folders with no
/// `.git`), with its owner file beside it.
fn jj_set(root: &Path) -> std::path::PathBuf {
    let set = root.join(".worktrees/s");
    for project in ["a", "b"] {
        fs::create_dir_all(set.join(project).join(".jj")).unwrap();
    }
    meta_plugin::sets::create(&set, "jj", root).unwrap();
    set
}

#[test]
fn a_set_whose_checkouts_are_no_git_repos_reaches_a_plugin_with_its_members() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);
    let set = jj_set(&root);
    answering(
        &root,
        "jj",
        &serde_json::json!({"results": {"records": []}}),
    );
    // from inside the set, with the workspace's plugins on PATH
    let output = sandbox
        .command(env!("CARGO_BIN_EXE_meta"))
        .args(["jj", "status"])
        .current_dir(set.join("a"))
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let sent: meta_plugin::Request =
        serde_json::from_slice(&fs::read(root.join("jj.request.json")).unwrap()).unwrap();
    let got = sent.set.unwrap();
    assert_eq!((got.name.as_str(), got.provider.as_str()), ("s", "jj"));
    let paths = |projects: &[meta_plugin::Project]| -> Vec<String> {
        projects.iter().map(|p| p.path.clone()).collect()
    };
    // the projects whose folders are in the set, at their places in it
    assert_eq!(paths(&got.members), ["a", "b"]);
    assert_eq!(paths(&sent.projects), ["a", "b"]);
    assert!(sent.projects.iter().all(|p| p.abs.starts_with(&set)));
}

#[test]
fn a_named_set_command_from_a_provider_that_does_not_own_the_set_is_refused() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    workspace(&sandbox, &root);
    jj_set(&root);
    script(
        &root.join("bin/meta-git"),
        &format!(
            r#"echo "$1" >> "{}"
[ "$1" = --meta-plugin-info ] && echo '{{"name":"git","version":"1","commands":["git worktree remove"]}}'
exit 0"#,
            root.join("git.log").display()
        ),
    );
    let output = run_meta(
        &sandbox,
        &root,
        &["--json", "git", "worktree", "remove", "s"],
    );
    assert!(!output.status.success());
    let error = envelope(&output).error.unwrap();
    assert_eq!(error.code, meta_plugin::ErrorCode::Conflict);
    assert!(
        error.message.contains("made by meta-jj"),
        "{}",
        error.message
    );
    // meta-git never got the call, and the set is untouched
    let log = fs::read_to_string(root.join("git.log")).unwrap_or_default();
    assert!(!log.contains("--meta-plugin-exec"), "{log}");
    assert!(root.join(".worktrees/s/a/.jj").is_dir());
}

/// A git command meta-git doesn't adapt reaches git in each selected repo
/// exactly as typed: git's own leading options, `help`, flags of the
/// command's, `--` and the words after it, one word with spaces.
#[test]
fn a_raw_git_line_reaches_git_byte_for_byte_in_each_repo() {
    let sandbox = Sandbox::new();
    let root = sandbox.work();
    fs::write(
        root.join(".meta.yaml"),
        r#"{"projects":{"a":"https://example.invalid/a.git","b":"https://example.invalid/b.git"}}"#,
    )
    .unwrap();
    for project in ["a", "b"] {
        fs::create_dir_all(root.join(project)).unwrap();
        sandbox.git(&root.join(project), &["init", "-q"]);
    }
    // a git that, for a command meta planned (it sets GIT_PAGER), logs
    // where it ran and each word it got, in one write, then succeeds;
    // meta's own git calls reach the real git
    let real = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|dir| dir.join("git"))
        .find(|git| git.is_file())
        .unwrap();
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let log = root.join("git.log");
    script(
        &bin.join("git"),
        &format!(
            r#"[ "$GIT_PAGER" = cat ] || exec "{}" "$@"
line="${{PWD##*/}}"; for word; do line="$line$(printf '\037')$word"; done
printf '%s\n' "$line" >> "{}""#,
            real.display(),
            log.display()
        ),
    );
    // meta-git, built beside meta
    let plugins = Path::new(env!("CARGO_BIN_EXE_meta")).parent().unwrap();
    let workspace = root.file_name().unwrap().to_string_lossy().into_owned();
    let lines: &[&[&str]] = &[
        &["-c", "key=value", "status"],
        &["--no-pager", "log", "-1"],
        &["-C", "d", "status"],
        &["-C", "d", "worktree", "remove", "s"],
        &["help", "log"],
        &["help"],
        &["log", "--oneline", "-3"],
        &["pull", "-r"],
        &["status", "--", "x"],
        &["log", "--format=%h %s", "--", "a b", "--help"],
    ];
    for line in lines {
        if log.exists() {
            fs::remove_file(&log).unwrap();
        }
        let output = sandbox
            .command(env!("CARGO_BIN_EXE_meta"))
            .arg("git")
            .args(*line)
            .current_dir(&root)
            .env(
                "PATH",
                format!("{}:{}:/usr/bin:/bin", bin.display(), plugins.display()),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "meta git {line:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut ran: Vec<String> = fs::read_to_string(&log)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect();
        ran.sort();
        let typed = line.iter().map(|w| format!("\x1f{w}")).collect::<String>();
        // the workspace's own repo, a and b
        let mut repos = [workspace.as_str(), "a", "b"].map(|repo| format!("{repo}{typed}"));
        repos.sort();
        assert_eq!(ran, repos, "{line:?}");
    }
    // meta-git's own commands keep their help subcommand, and run no git
    if log.exists() {
        fs::remove_file(&log).unwrap();
    }
    for (line, shown) in [
        (
            &["git", "worktree", "help"][..],
            "Usage: meta git worktree <COMMAND>",
        ),
        (
            &["git", "worktree", "help", "create"],
            "Usage: meta git worktree create",
        ),
        (
            &["--json", "git", "worktree", "help"],
            "Usage: meta git worktree <COMMAND>",
        ),
    ] {
        let output = sandbox
            .command(env!("CARGO_BIN_EXE_meta"))
            .args(line)
            .current_dir(&root)
            .env(
                "PATH",
                format!("{}:{}:/usr/bin:/bin", bin.display(), plugins.display()),
            )
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "meta {line:?}: {said}");
        assert!(said.contains(shown), "meta {line:?}: {said}");
    }
    assert!(!log.exists());
}
