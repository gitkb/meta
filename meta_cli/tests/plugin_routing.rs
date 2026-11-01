//! `meta exec` runs commands as typed; a plugin file `meta-<word>` gets only
//! `meta <word> ...`, and only it is started for that command.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tempfile::tempdir;

/// A workspace with repos `a` and `b`, a plugin `meta-touchy` on PATH that
/// claims `touch` and logs each request it receives instead of running it,
/// and a plugin `meta-spy` that logs every time it is started.
fn workspace(root: &Path) {
    fs::write(
        root.join(".meta.yaml"),
        r#"{"projects":{"a":"https://example.invalid/a.git","b":"https://example.invalid/b.git"}}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("a")).unwrap();
    fs::create_dir_all(root.join("b")).unwrap();
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let plugin = bin.join("meta-touchy");
    fs::write(
        &plugin,
        format!(
            r#"#!/bin/sh
case "$1" in
  --meta-plugin-info) echo '{{"name":"touchy","version":"1.0.0","commands":["touch"]}}' ;;
  --meta-plugin-exec) cat >> "{}" ;;
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

fn meta(root: &Path, args: &[&str]) {
    let output = run_meta(root, args);
    assert!(
        output.status.success(),
        "meta {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_meta(root: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_meta"))
        .args(args)
        .current_dir(root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("xdg"))
        .env("XDG_DATA_HOME", root.join("xdg/data"))
        .env("GIT_CONFIG_GLOBAL", root.join("gitconfig"))
        .env("META_DATA_DIR", root.join("data"))
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
    let temp = tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    workspace(&root);

    meta(&root, &["exec", "--", "touch", "ran"]);

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
    let temp = tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    workspace(&root);

    meta(&root, &["touchy", "ran"]);

    let request = fs::read_to_string(root.join("plugin.log")).unwrap();
    assert!(request.contains(r#""command":"touchy""#), "{request}");
    assert!(request.contains(r#""args":["ran"]"#), "{request}");
    assert!(
        !root.join("ran").exists(),
        "meta ran the plugin's command raw"
    );

    // `touch` is claimed by meta-touchy, but no file is named meta-touch
    fs::remove_file(root.join("plugin.log")).unwrap();
    let output = run_meta(&root, &["touch", "ran"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unrecognized command 'touch'"), "{stderr}");
    assert!(!root.join("plugin.log").exists(), "meta-touchy got `touch`");
}

#[test]
fn only_the_plugin_a_command_names_is_started() {
    let temp = tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    workspace(&root);

    meta(&root, &["touchy", "ran"]);
    // meta-touchy prints nothing for --help: its info stands in
    let help = run_meta(&root, &["touchy", "--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("Commands:\n  touch"));
    meta(&root, &["exec", "--", "true"]);
    meta(&root, &["context"]);
    let typo = run_meta(&root, &["touhcy"]);
    assert!(!typo.status.success());
    let stderr = String::from_utf8_lossy(&typo.stderr);
    assert!(stderr.contains("meta --help lists them"), "{stderr}");
    assert_eq!(spy_starts(&root), "", "an unrelated plugin was started");

    // Help and the plugin list ask every plugin, and name each by its file
    let help = run_meta(&root, &["--help"]);
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("plugin: spy v1.0.0"), "{help}");
    assert!(help.contains("plugin: touchy v1.0.0"), "{help}");
    let list = run_meta(&root, &["plugin", "list", "--json"]);
    let list: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    let names: Vec<_> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|plugin| plugin["name"].as_str().unwrap())
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
    let temp = tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    workspace(&root);
    let local = root.join(".meta/plugins/meta-touchy");
    fs::create_dir_all(local.parent().unwrap()).unwrap();
    script(
        &local,
        &format!(
            r#"[ "$1" = --meta-plugin-info ] && echo '{{"name":"touchy","version":"2.0.0","commands":[]}}'
[ "$1" = --meta-plugin-exec ] && cat > "{}"
exit 0"#,
            root.join("local.log").display()
        ),
    );

    meta(&root, &["touchy", "ran"]);

    assert!(
        root.join("local.log").exists(),
        "the workspace plugin didn't run"
    );
    assert!(!root.join("plugin.log").exists(), "the one on PATH ran");
}

#[test]
fn a_called_plugin_that_does_not_answer_fails_naming_it() {
    let temp = tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    workspace(&root);
    script(&root.join("bin/meta-slow"), "exec sleep 60");

    let started = std::time::Instant::now();
    let output = run_meta(&root, &["slow", "x"]);

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
    let temp = tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    workspace(&root);
    // An older plugin that still plans `cmd` strings instead of `argv`
    let plugin = root.join("bin").join("meta-oldplan");
    fs::write(
        &plugin,
        r#"#!/bin/sh
case "$1" in
  --meta-plugin-info) echo '{"name":"oldplan","version":"1.0.0","commands":["oldplan"]}' ;;
  --meta-plugin-exec) cat >/dev/null; echo '{"plan":{"commands":[{"dir":".","cmd":"touch ran"}]}}' ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&plugin, fs::Permissions::from_mode(0o755)).unwrap();

    let output = run_meta(&root, &["oldplan", "run"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "an unreadable plan passed as success"
    );
    assert!(stderr.contains("upgrade the plugin"), "{stderr}");
    assert!(stderr.contains(&plugin.display().to_string()), "{stderr}");
}

#[test]
fn a_plan_with_max_parallel_zero_is_refused_before_any_of_it_runs() {
    let temp = tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    workspace(&root);
    let plugin = root.join("bin").join("meta-zero");
    fs::write(
        &plugin,
        r#"#!/bin/sh
case "$1" in
  --meta-plugin-info) echo '{"name":"zero","version":"1.0.0","commands":["zero"]}' ;;
  --meta-plugin-exec) cat >/dev/null; echo '{"plan":{"pre_commands":[{"dir":".","argv":["touch","setup-ran"]}],"commands":[{"dir":".","argv":["touch","ran"]}],"max_parallel":0}}' ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&plugin, fs::Permissions::from_mode(0o755)).unwrap();

    let output = run_meta(&root, &["zero", "run"]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "max_parallel 0 ran unlimited");
    assert!(stderr.contains("max_parallel"), "{stderr}");
    assert!(!root.join("setup-ran").exists() && !root.join("ran").exists());
}

#[test]
fn a_broken_looprc_stops_only_what_runs_through_loop() {
    let temp = tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    workspace(&root);
    // HOME is root too: ~/.looprc and ./.looprc are this file
    fs::write(root.join(".looprc"), r#"{"bogus": 1}"#).unwrap();

    // the plugin answers with output, not a plan: loop never runs
    meta(&root, &["touchy", "ran"]);
    assert!(fs::read_to_string(root.join("plugin.log"))
        .unwrap()
        .contains("touchy"));

    let output = run_meta(&root, &["exec", "--", "true"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(
        stderr.contains(".looprc") && stderr.contains("bogus"),
        "{stderr}"
    );
}
