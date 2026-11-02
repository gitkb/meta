//! Plugin scope and metadata probing must not break unrelated commands.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]
// Unix only: the plugins here are shell scripts
#![cfg(unix)]

use meta_test_support::Sandbox;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use rustix::process::{geteuid, kill_process, test_kill_process, Pid, Signal};

struct Fixture {
    sandbox: Sandbox,
    home: PathBuf,
    cwd: PathBuf,
    data: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new(workspace: bool) -> Self {
        let sandbox = Sandbox::new();
        let home = sandbox.home();
        let cwd = sandbox.work().join("workspace");
        let data = sandbox.meta_data();
        let bin = sandbox.work().join("bin");
        for dir in [&cwd, &data, &bin] {
            fs::create_dir_all(dir).unwrap();
        }
        if workspace {
            fs::write(cwd.join(".meta.yaml"), "projects: {}\n").unwrap();
        }
        Self {
            sandbox,
            home,
            cwd,
            data,
            bin,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = self.sandbox.command(env!("CARGO_BIN_EXE_meta"));
        cmd.args(args)
            .current_dir(&self.cwd)
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()));
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
}

fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn plugin(path: &Path, name: &str, version: &str) {
    script(
        path,
        &format!(
            "echo '{{\"name\":\"{name}\",\"version\":\"{version}\",\"commands\":[\"{name}\"]}}'"
        ),
    );
}

#[test]
fn bare_home_meta_directory_is_neither_workspace_nor_project_plugin_source() {
    let mut f = Fixture::new(false);
    f.cwd = f.home.clone();
    plugin(&f.home.join(".meta/plugins/meta-fake"), "fake", "1.0");
    let help = f.run(&["--help"]);
    assert!(help.status.success());
    assert!(!String::from_utf8_lossy(&help.stdout).contains("fake"));
    for args in [
        vec!["plugin", "list", "--local"],
        vec!["plugin", "list", "--local", "--json"],
        vec!["plugin", "install", "--local", "missing/repo"],
        vec!["plugin", "uninstall", "--local", "missing"],
        vec!["plugin", "update", "--local", "missing"],
    ] {
        let output = f.run(&args);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.status.success(), "{args:?}");
        assert!(text.contains("Not in a meta workspace"), "{args:?}: {text}");
    }
}

#[test]
fn discovery_stops_at_nearest_workspace_and_uses_global_data_dir() {
    let f = Fixture::new(true);
    plugin(
        &f.sandbox.work().join(".meta/plugins/meta-ancestor"),
        "ancestor",
        "1",
    );
    plugin(&f.cwd.join(".meta/plugins/meta-local"), "local", "2");
    plugin(&f.data.join("plugins/meta-global"), "global", "3");
    plugin(&f.bin.join("meta-system"), "system", "4");
    // All three sources provide the same name: the local one wins.
    plugin(&f.data.join("plugins/meta-local"), "local", "wrong-global");
    plugin(&f.bin.join("meta-local"), "local", "wrong-path");
    plugin(&f.bin.join("meta-global"), "global", "wrong-path");
    let output = f.run(&["plugin", "list", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(!text.contains("ancestor"), "{text}");
    assert!(!text.contains("wrong-"), "{text}");
    for name in ["local", "global", "system"] {
        assert!(text.contains(name), "{text}");
    }
}

#[test]
fn nested_meta_dir_reads_its_workspace_plugins_but_not_an_unlisting_parent() {
    let f = Fixture::new(true);
    plugin(
        &f.sandbox.work().join(".meta/plugins/meta-parent"),
        "parent",
        "1",
    );
    let listed = |yaml: &str| {
        fs::write(f.sandbox.work().join(".meta.yaml"), yaml).unwrap();
        let output = f.run(&["plugin", "list", "--json"]);
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).contains("parent")
    };
    // The parent lists this meta dir: one workspace, its plugins load here
    assert!(listed(
        "projects:\n  workspace:\n    repo: x\n    meta: true\n"
    ));
    // A parent that doesn't list it is another workspace
    assert!(!listed("projects: {}\n"));
}

#[test]
fn a_worktree_set_checkout_reads_its_own_and_its_origins_plugins() {
    let f = Fixture::new(true);
    // A set's root checkout, laid out as git and meta leave it
    let set = f.cwd.join(".worktrees/s1");
    for dir in ["objects", "refs"] {
        fs::create_dir_all(f.cwd.join(".git").join(dir)).unwrap();
    }
    fs::write(f.cwd.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let admin = f.cwd.join(".git/worktrees/s1");
    fs::create_dir_all(&admin).unwrap();
    fs::write(admin.join("commondir"), "../..\n").unwrap();
    fs::write(admin.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    let back = set.join(".git").display().to_string();
    fs::write(admin.join("gitdir"), format!("{back}\n")).unwrap();
    let record = format!(
        "{{\"project\": {:?}, \"set\": {:?}, \"created_at\": \"2025-01-01T00:00:00Z\"}}",
        f.cwd.display().to_string(),
        set.display().to_string()
    );
    fs::write(admin.join("meta-worktree.json"), record).unwrap();
    fs::create_dir_all(&set).unwrap();
    fs::write(set.join(".meta.yaml"), "projects: {}\n").unwrap();
    fs::write(set.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
    plugin(&f.cwd.join(".meta/plugins/meta-main"), "main", "1");
    plugin(&set.join(".meta/plugins/meta-setlocal"), "setlocal", "1");
    let output = f
        .command(&["plugin", "list", "--json"])
        .current_dir(&set)
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.contains("setlocal") && text.contains("\"main\""),
        "{text}"
    );
}

#[test]
fn explicit_config_does_not_redirect_plugin_scope() {
    let f = Fixture::new(true);
    let other = f.sandbox.work().join("other");
    fs::create_dir(&other).unwrap();
    fs::write(other.join(".meta.yaml"), "projects: {}").unwrap();
    plugin(&other.join(".meta/plugins/meta-foreign"), "foreign", "1");
    plugin(&f.cwd.join(".meta/plugins/meta-local"), "local", "2");
    let output = f.run(&[
        "--config",
        other.join(".meta.yaml").to_str().unwrap(),
        "plugin",
        "list",
        "--json",
    ]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("local"));
    assert!(!text.contains("foreign"));
}

#[test]
fn local_list_from_subdirectory_uses_real_path_and_binary_version() {
    let mut f = Fixture::new(true);
    let plugin_path = f.cwd.join(".meta/plugins/meta-real");
    plugin(&plugin_path, "real", "2.5.0");
    fs::write(f.cwd.join(".meta/plugins/.manifest.json"), r#"{"plugins":{"meta-real":{"source":"example/repo","version":"stale","installed":"2024-01-01","platform":"test"}}}"#).unwrap();
    f.cwd = f.cwd.join("nested/deep");
    fs::create_dir_all(&f.cwd).unwrap();
    let output = f.run(&["plugin", "list", "--local"]);
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains(plugin_path.to_str().unwrap()), "{text}");
    assert!(text.contains("2.5.0"), "{text}");
    assert!(!text.contains("stale"), "{text}");
}

#[test]
fn missing_config_errors_are_escaped() {
    let f = Fixture::new(true);
    let output = f.run(&["--config", "absent\u{1b}\nFORGED", "exec", "--", "true"]);
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("Could not find meta config file 'absent\\u{1b}\\nFORGED'"),
        "{err}"
    );
    assert!(!err.contains('\u{1b}'), "{err}");
    fs::remove_file(f.cwd.join(".meta.yaml")).unwrap();
    let output = f.run(&["exec", "--", "true"]);
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("'.meta.yaml'"), "{err}");
}

#[test]
fn local_install_gitignores_untracked_plugin_files() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let f = Fixture::new(true);
    let body =
        b"#!/bin/sh\necho '{\"name\":\"installed\",\"version\":\"1.2.3\",\"commands\":[]}'\n";
    let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    let mut header = tar::Header::new_gnu();
    header.set_path("meta-installed").unwrap();
    header.set_size(body.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    archive.append(&header, body.as_slice()).unwrap();
    let bytes = archive.into_inner().unwrap().finish().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/plugin.tar.gz", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    // macOS hands out the listener's non-blocking mode
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut request = [0; 4096];
                    assert!(stream.read(&mut request).unwrap() > 0);
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        bytes.len()
                    )
                    .unwrap();
                    stream.write_all(&bytes).unwrap();
                    return Ok(());
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => return Err(e),
            }
        }
    });
    let output = f.run(&["plugin", "install", "--local", &url]);
    server
        .join()
        .unwrap()
        .expect("download request did not arrive");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(f.cwd.join(".meta/plugins/meta-installed").is_file());
    assert_eq!(
        fs::read_to_string(f.cwd.join(".meta/plugins/.gitignore")).unwrap(),
        "*\n"
    );
}

#[test]
fn orphan_hint_names_actual_parent_config() {
    let f = Fixture::new(true);
    fs::write(f.sandbox.work().join(".meta.yaml"), "projects: {}").unwrap();
    let output = f.run(&["--help"]);
    assert!(output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    // meta names the parent by its resolved path (/private/var on macOS)
    let parent_config = f.sandbox.work().join(".meta.yaml");
    assert!(
        err.contains(&format!(
            "To include it, add to {}:",
            parent_config.display()
        )),
        "{err}"
    );
}

#[test]
fn a_meta_file_where_the_folder_goes_is_not_taken_for_no_plugins() {
    let f = Fixture::new(true);
    fs::write(f.cwd.join(".meta"), "{}\n").unwrap();
    let out = f.run(&["--help"]);
    assert!(out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("Cannot read plugins directory"), "{err}");
    let out = f.run(&["plugin", "list", "--local"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("Failed to read manifest"), "{err}");
}

#[test]
fn unreadable_plugin_directory_warns_and_help_still_runs() {
    // Root can read mode-000 directories, so this case cannot be exercised there.
    if geteuid().is_root() {
        return;
    }
    let f = Fixture::new(true);
    let global_data = f.home.join("global\nFORGED\u{1b}");
    let global_plugins = global_data.join("plugins");
    fs::create_dir_all(&global_plugins).unwrap();
    fs::set_permissions(&global_plugins, fs::Permissions::from_mode(0o000)).unwrap();
    let output = f
        .command(&["--help"])
        .env("META_DATA_DIR", &global_data)
        .output()
        .unwrap();
    fs::set_permissions(&global_plugins, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("global\\nFORGED\\u{1b}/plugins"), "{err}");
    assert!(err.contains("Cannot read plugins directory"), "{err}");
    assert!(!err.contains('\u{1b}'));
    // Also exercise the actual workspace plugins directory.
    let path = f.cwd.join(".meta/plugins");
    fs::create_dir_all(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let output_local = f.run(&["--help"]);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(output.status.success());
    assert!(output_local.status.success());
    let err = String::from_utf8_lossy(&output_local.stderr);
    assert!(
        err.contains("warning: Cannot read plugins directory"),
        "{err}"
    );
    assert!(err.contains(path.to_str().unwrap()), "{err}");
}

fn running(pid: Pid) -> bool {
    // On Linux an orphan may briefly be a zombie until init reaps it.
    if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) {
        return !stat.rsplit_once(") ").unwrap().1.starts_with('Z');
    }
    test_kill_process(pid).is_ok()
}

fn timeout_case(background: bool) {
    let f = Fixture::new(true);
    let pidfile = f.cwd.join("pids");
    let path = f.cwd.join(".meta/plugins/meta-hung\nFORGED\u{1b}");
    let body = if background {
        "echo $$ > pids\nsleep 60 &\necho $! >> pids\nexit 0"
    } else {
        "echo $$ > pids\nexec sleep 60"
    };
    script(&path, body);
    // Redirect to files so a regression cannot block this test on inherited pipes.
    let stdout = fs::File::create(f.cwd.join("stdout")).unwrap();
    let stderr = fs::File::create(f.cwd.join("stderr")).unwrap();
    let started = Instant::now();
    let mut child = f
        .command(&["--help"])
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .unwrap();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if started.elapsed() > Duration::from_secs(8) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let pids: Vec<Pid> = fs::read_to_string(&pidfile)
        .unwrap()
        .lines()
        .map(|s| Pid::from_raw(s.parse().unwrap()).unwrap())
        .collect();
    let mut survivors = Vec::new();
    for pid in pids {
        for _ in 0..50 {
            if !running(pid) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if running(pid) {
            survivors.push(pid);
            let _ = kill_process(pid, Signal::KILL);
        }
    }
    assert!(
        status.is_some_and(|s| s.success()),
        "command did not finish within deadline"
    );
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(
        survivors.is_empty(),
        "plugin processes left running: {survivors:?}"
    );
    let err = fs::read_to_string(f.cwd.join("stderr")).unwrap();
    assert!(err.contains("meta-hung\\nFORGED\\u{1b}"), "{err}");
    assert!(
        err.contains("did not answer --meta-plugin-info within 5 seconds; skipping"),
        "{err}"
    );
    assert!(!err.contains('\u{1b}'));
}

#[test]
fn sleeping_probe_times_out_and_reaps_plugin() {
    timeout_case(false);
}

#[test]
fn exited_wrapper_with_background_stdout_times_out_and_kills_descendant() {
    timeout_case(true);
}

#[test]
fn generated_help_keeps_a_plugins_values_on_their_lines() {
    let f = Fixture::new(true);
    script(
        &f.data.join("plugins/meta-odd"),
        r#"case "$1" in
--meta-plugin-info) printf '%s\n' '{"name":"odd","version":"1\u001b[2J","description":"d\nFORGED","commands":["odd x\u001b[2J\nFORGED"]}' ;;
*) exit 1 ;;
esac"#,
    );
    for args in [&["--help"][..], &["odd", "--help"][..]] {
        let out = f.run(args);
        let text = String::from_utf8_lossy(&out.stdout).to_string()
            + &String::from_utf8_lossy(&out.stderr);
        assert!(!text.contains('\u{1b}'), "{args:?}: {text}");
        assert!(
            !text.lines().any(|line| line.starts_with("FORGED")),
            "{args:?}: {text}"
        );
        assert!(text.contains("odd"), "{args:?}: {text}");
    }
}

#[test]
fn verbose_dispatch_keeps_a_plugins_values_on_their_lines() {
    let f = Fixture::new(true);
    script(
        &f.data.join("plugins/meta-demo"),
        r#"case "$1" in
--meta-plugin-info) printf '%s\n' '{"name":"demo","version":"1\u001b[2J\nFORGED","commands":["demo run"]}' ;;
*) cat >/dev/null ;;
esac"#,
    );
    let out = f.run(&["--verbose", "demo", "run"]);
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("Found plugin: demo v1"), "{text}");
    assert!(text.contains("Executing plugin demo"), "{text}");
    assert!(!text.contains('\u{1b}'), "{text}");
    assert!(
        !text.lines().any(|line| line.starts_with("FORGED")),
        "{text}"
    );
}

#[test]
fn an_unreadable_plan_names_the_plugin_on_one_line() {
    let f = Fixture::new(false);
    // a workspace whose path holds a newline: the plugin's path does too
    let cwd = f.cwd.join("ws\nFORGED");
    fs::create_dir_all(&cwd).unwrap();
    fs::write(cwd.join(".meta.yaml"), "projects: {}\n").unwrap();
    script(
        &cwd.join(".meta/plugins/meta-demo"),
        r#"case "$1" in
--meta-plugin-info) printf '%s\n' '{"name":"demo","version":"1","commands":["demo run"]}' ;;
*) cat >/dev/null; printf '%s\n' '{"plan":{"commands":[{"cmd":"true"}]}}' ;;
esac"#,
    );
    let out = f
        .command(&["demo", "run"])
        .current_dir(&cwd)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(err.contains("ws\\nFORGED"), "{err}");
    assert!(!err.lines().any(|line| line.starts_with("FORGED")), "{err}");
}

#[test]
fn a_plugin_that_answers_without_reading_its_request_still_answers() {
    let f = Fixture::new(true);
    script(
        &f.data.join("plugins/meta-quick"),
        r#"case "$1" in
--meta-plugin-info) printf '%s\n' '{"name":"quick","version":"1","commands":["quick go"]}' ;;
*) echo answered ;;
esac"#,
    );
    // a request bigger than a pipe holds: the plugin always exits before
    // meta has written it all
    let out = f.run(&["--strict", "quick", "go", &"r".repeat(100 * 1024)]);
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("answered"), "{text}");
}

#[test]
fn a_plugin_that_writes_before_reading_a_large_request_gets_all_of_it() {
    let f = Fixture::new(true);
    let received = f.home.join("received");
    // 256 KiB out before reading: more than a pipe holds either way
    script(
        &f.data.join("plugins/meta-chatty"),
        &format!(
            r#"case "$1" in
--meta-plugin-info) printf '%s\n' '{{"name":"chatty","version":"1","commands":["chatty"]}}' ;;
*) head -c 262144 /dev/zero | tr '\0' x; wc -c > "{}" ;;
esac"#,
            received.display()
        ),
    );
    let stdout = f.home.join("stdout");
    let mut meta = f
        .command(&["chatty", &"r".repeat(100 * 1024)])
        .stdout(fs::File::create(&stdout).unwrap())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let status = loop {
        if let Some(status) = meta.try_wait().unwrap() {
            break Some(status);
        }
        if started.elapsed() > Duration::from_secs(30) {
            let _ = meta.kill();
            let _ = meta.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let status = status.expect("meta and the plugin blocked each other");
    assert!(status.success());
    let request: usize = fs::read_to_string(&received)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(request > 100 * 1024, "{request}");
    assert_eq!(fs::read(&stdout).unwrap(), vec![b'x'; 262144]);
}

#[test]
fn unusable_files_in_plugin_folders_are_warned_about_but_not_on_path() {
    let f = Fixture::new(true);
    // named apart per folder, as the first file with a name hides the rest
    let unusable = |dir: &Path, tag: &str| {
        let bad_json = dir.join(format!("meta-garbled-{tag}"));
        script(&bad_json, "echo not-json");
        let cannot_start = dir.join(format!("meta-unstartable-{tag}"));
        fs::write(&cannot_start, "#!/nonexistent/interpreter\n").unwrap();
        fs::set_permissions(&cannot_start, fs::Permissions::from_mode(0o755)).unwrap();
        let not_executable = dir.join(format!("meta-plain-{tag}"));
        fs::write(&not_executable, "#!/bin/sh\n").unwrap();
        [bad_json, cannot_start, not_executable]
    };
    let warned: Vec<PathBuf> = [
        (f.cwd.join(".meta/plugins"), "local"),
        (f.data.join("plugins"), "global"),
    ]
    .iter()
    .flat_map(|(dir, tag)| {
        fs::create_dir_all(dir).unwrap();
        unusable(dir, tag)
    })
    .collect();
    // anything may be named meta-* on PATH
    let silent = unusable(&f.bin, "path");
    let out = f.run(&["--help"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    for path in &warned {
        assert!(
            err.lines()
                .any(|line| line.starts_with("warning:") && line.contains(path.to_str().unwrap())),
            "{}: {err}",
            path.display()
        );
    }
    for path in &silent {
        assert!(!err.contains(path.to_str().unwrap()), "{err}");
    }
}

#[test]
fn plugin_update_names_each_plugin_it_cannot_check_and_fails() {
    let f = Fixture::new(true);
    // installed from a URL, and from GitHub with no version recorded:
    // neither can be checked, and checking them needs no network. A plugin
    // whose file was deleted is no longer installed.
    for name in ["fromurl", "noversion"] {
        plugin(&f.data.join(format!("plugins/meta-{name}")), name, "1");
    }
    fs::write(
        f.data.join("plugins/.manifest.json"),
        r#"{"plugins": {
  "meta-fromurl": {"source": "http://127.0.0.1:9/p.tar.gz", "installed": "2026-01-01T00:00:00Z", "platform": "x"},
  "meta-noversion": {"source": "someone/meta-noversion", "installed": "2026-01-01T00:00:00Z", "platform": "x"},
  "meta-deleted": {"source": "http://127.0.0.1:9/d.tar.gz", "installed": "2026-01-01T00:00:00Z", "platform": "x"}
}}"#,
    )
    .unwrap();
    for args in [
        &["plugin", "update"][..],
        &["plugin", "update", "--check"][..],
    ] {
        let out = f.run(args);
        let text = String::from_utf8_lossy(&out.stdout).to_string()
            + &String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{args:?}: {text}");
        assert!(!text.contains("up to date"), "{args:?}: {text}");
        assert!(!text.contains("deleted"), "{args:?}: {text}");
        // a source may hold a credential
        assert!(!text.contains("127.0.0.1"), "{args:?}: {text}");
        for name in ["meta-fromurl", "meta-noversion"] {
            assert!(
                text.contains(&format!("Cannot check {name} for updates")),
                "{args:?}: {text}"
            );
        }
    }
    let out = f.run(&["plugin", "update", "fromurl"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(
        err.contains("Cannot check meta-fromurl for updates"),
        "{err}"
    );
}

#[test]
fn plugin_help_comes_from_its_info_never_from_running_it_with_help() {
    let f = Fixture::new(true);
    let log = f.home.join("starts.log");
    // its own --help would forge a line and clear the screen
    script(
        &f.data.join("plugins/meta-odd"),
        &format!(
            r#"echo "$1" >> "{}"
case "$1" in
--meta-plugin-info) printf '%s\n' '{{"name":"odd","version":"1","commands":["odd x"],"help":{{"usage":"meta odd <x>\u001b[2J","commands":{{"x":"does x"}}}}}}' ;;
--help) printf 'own\033[2J\nFORGED\n' ;;
esac"#,
            log.display()
        ),
    );
    for args in [&["odd"][..], &["odd", "--help"][..], &["help", "odd"][..]] {
        let out = f.run(args);
        assert!(out.status.success(), "{args:?}");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("meta odd <x>"), "{args:?}: {text}");
        assert!(text.contains("does x"), "{args:?}: {text}");
        assert!(
            !text.contains('\u{1b}') && !text.contains("FORGED"),
            "{args:?}: {text}"
        );
    }
    let starts = fs::read_to_string(&log).unwrap();
    assert!(!starts.lines().any(|line| line == "--help"), "{starts}");
}

#[test]
fn plugin_words_that_escape_alike_are_listed_once() {
    let f = Fixture::new(true);
    // a word with DEL, and one spelled as DEL's escape
    plugin(&f.data.join("plugins/meta-odd\u{7f}"), "odd", "1");
    plugin(&f.data.join("plugins/meta-odd\\u{7f}"), "odd", "1");
    let out = f.run(&["--help"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let help = String::from_utf8_lossy(&out.stdout);
    assert_eq!(help.matches("odd\\u{7f}").count(), 2, "{help}");
    assert!(!help.contains('\u{7f}'), "{help}");
}
