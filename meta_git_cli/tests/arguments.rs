//! What the git plugin takes from a protocol request: the arguments of
//! `meta git clone` and `meta git commit`, and the request's directory.

#![expect(
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]

use meta_plugin::testing::{call, Called, Sandbox};
use meta_plugin::{Answer, Meta, Request, Scope};
use serde_json::{json, Value};
use std::path::Path;
use std::process::Stdio;

/// meta's options, as a test gives them: `json!({"json": true, "strict":
/// true, "dry_run": true, "recursive": true, "all": true, "depth": 2})`.
/// `recursive`/`all`/`depth` are how meta resolves the workspace and how
/// deep (meta sends those as `request.depth` and the workspace it found);
/// the rest are `request.meta`.
fn request(
    cwd: &Path,
    command: &str,
    args: &[&str],
    options: &Value,
    worktrees: Option<&Path>,
) -> Request {
    let flag = |name: &str| options[name].as_bool().unwrap_or(false);
    let (recursive, all) = (flag("recursive"), flag("all"));
    let mut request = Request::new(command, cwd);
    request.args = args.iter().map(|a| a.to_string()).collect();
    request.scope = match command.split_whitespace().nth(1) {
        Some("clone") => Scope::None,
        Some("update" | "worktree") => Scope::Workspace,
        _ => Scope::Selection,
    };
    // with -r or -A, how deep (none: no limit); without, none of the nested
    request.recursive = recursive || all;
    request.depth = match recursive || all {
        true => options["depth"]
            .as_u64()
            .map(|d| usize::try_from(d).unwrap()),
        false => Some(0),
    };
    // the workspace meta finds from cwd: -A from its outermost config, -r
    // naming its projects from the outermost that lists it
    let found = meta_core::config::Workspace::find(cwd, None).unwrap();
    let found = match all {
        true => found.map(|w| w.root().unwrap()),
        false => found,
    };
    let set = set_of(cwd);
    // where sets go: `worktrees` when given (as `META_WORKTREES` gives it)
    request.worktree = meta_core::worktree::plugin_worktree(
        found.as_ref(),
        cwd,
        set.as_ref().map(|s| s.dir.as_path()),
    );
    if let Some(dir) = worktrees {
        (request.worktree.dir, request.worktree.refused) = (Some(dir.to_path_buf()), None);
    }
    request.workspace = found.map(as_meta_sends);
    request.set = set;
    request.meta = Meta {
        json: flag("json"),
        dry_run: flag("dry_run"),
        strict: flag("strict"),
        verbose: flag("verbose"),
        ..Meta::default()
    };
    request
}

/// The workspace `found` as meta sends it: its whole tree (from the
/// outermost config) as members.
fn as_meta_sends(found: meta_core::config::Workspace) -> meta_plugin::Workspace {
    use meta_core::{config, repo_filter};
    let (root, repos) = match repo_filter::Reach::of(found.clone(), true, true, None) {
        Ok(reach) => (reach.root.clone(), reach.scope().repos),
        Err(_) => (found.dir.clone(), Vec::new()),
    };
    let members = repos
        .iter()
        .map(|repo| {
            let info = repo.info.as_ref();
            meta_plugin::Project {
                name: repo.name().unwrap_or(".").to_string(),
                path: repo_filter::repo_path(&repo.dir, &root),
                abs: repo.dir.clone(),
                repo: info
                    .and_then(|i| i.repo.as_ref())
                    .map(|r| r.raw().to_string()),
                tags: repo.tags().to_vec(),
                depends_on: info.map(|i| i.depends_on.clone()).unwrap_or_default(),
                meta: info.is_some_and(|i| i.meta),
                present: repo.dir.exists(),
                ..meta_plugin::Project::default()
            }
        })
        .collect();
    meta_plugin::Workspace {
        root,
        config: found.config_path.clone(),
        listed_root: config::listed_root(&found.dir),
        members,
    }
}

/// The set `cwd` is in, as meta finds it.
fn set_of(cwd: &Path) -> Option<meta_plugin::Set> {
    let (name, dir) = meta_core::worktree::detect_worktree_location(cwd).ok()??;
    Some(meta_plugin::Set {
        name,
        dir,
        members: Vec::new(),
        provider: "git".to_string(),
        leased: false,
    })
}

/// `cmd` as meta starts a plugin: it can call meta back (the `meta` built
/// beside it: `cargo build --workspace --bins` first); and the
/// `META_WORKTREES` set on it, which meta reads for the request.
fn as_meta_starts(cmd: &mut std::process::Command) -> Option<std::path::PathBuf> {
    let bin = Path::new(env!("CARGO_BIN_EXE_meta-git")).parent().unwrap();
    let meta = bin.join(format!("meta{}", std::env::consts::EXE_SUFFIX));
    assert!(
        meta.is_file(),
        "{} is not built: cargo build --workspace --bins first",
        meta.display()
    );
    cmd.env(meta_plugin::client::EXE_VAR, meta);
    // and meta finds this meta-git, as the plugin of `meta git`
    let path = cmd
        .get_envs()
        .find(|(key, _)| *key == "PATH")
        .and_then(|(_, value)| value.map(std::ffi::OsStr::to_os_string))
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    let paths = std::iter::once(bin.to_path_buf()).chain(std::env::split_paths(&path));
    cmd.env("PATH", std::env::join_paths(paths).unwrap());
    cmd.get_envs()
        .find(|(key, _)| *key == "META_WORKTREES")
        .and_then(|(_, value)| value)
        .map(std::path::PathBuf::from)
}

/// How meta would end: its exit code.
struct Status(Option<i32>);

impl Status {
    fn success(&self) -> bool {
        self.0 == Some(0)
    }
}

/// What meta would make of a plugin's answer: the plugin's own output, then
/// its warnings and error on stderr as meta says them (`warning: …`,
/// `Error: …`), a message on stdout; under `--json`, stdout is the
/// Envelope's parts (`results`, `warnings`, `message`, `error`); the code
/// meta would end with (a failed record fails the command).
struct Ran {
    status: Status,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn ran(called: Called, json: bool) -> Ran {
    let Called { output, response } = called;
    let (mut stdout, mut stderr) = (output.stdout, output.stderr);
    let mut code = output.status.code();
    let Some(response) = response else {
        return Ran {
            status: Status(code),
            stdout,
            stderr,
        };
    };
    let mut say = |text: String| stderr.extend(text.into_bytes());
    for warning in &response.warnings {
        say(format!("warning: {}\n", warning.message));
    }
    let (records, message, error) = match response.answer {
        Answer::Results(results) => {
            let error = results.records.iter().find_map(|r| r.error.clone());
            (results.records, None, error)
        }
        Answer::Error(error) => (Vec::new(), None, Some(error)),
        Answer::Message(message) => (Vec::new(), Some(message), None),
        Answer::Plan(_) => (Vec::new(), None, None),
    };
    if let Some(error) = &error {
        say(format!("Error: {}\n", error.message));
        code = Some(error.code.exit_code());
    }
    match json {
        true => {
            let envelope = json!({
                "success": error.is_none(),
                "results": records,
                "warnings": response.warnings,
                "message": message,
                "error": error,
            });
            stdout = envelope.to_string().into_bytes();
        }
        false => {
            if let Some(message) = message.filter(|m| !m.is_empty()) {
                stdout.extend(format!("{}\n", message.trim_end()).into_bytes());
            }
        }
    }
    Ran {
        status: Status(code),
        stdout,
        stderr,
    }
}

/// `meta <command> <args>` in `cwd`, with meta's `options`, the plugin
/// started from `cmd` (a `sandbox.command(env!("CARGO_BIN_EXE_meta-git"))`
/// with whatever else the test sets on it).
fn meta_git_with(
    cmd: &mut std::process::Command,
    cwd: &Path,
    command: &str,
    args: &[&str],
    options: Value,
) -> Ran {
    let worktrees = as_meta_starts(cmd);
    let request = request(cwd, command, args, &options, worktrees.as_deref());
    ran(call(cmd, &request), request.meta.json)
}

fn meta_git(sandbox: &Sandbox, cwd: &Path, command: &str, args: &[&str], options: Value) -> Ran {
    meta_git_from(sandbox, cwd, cwd, command, args, options)
}

/// [`meta_git`], with the plugin process started in `process_dir`.
fn meta_git_from(
    sandbox: &Sandbox,
    process_dir: &Path,
    cwd: &Path,
    command: &str,
    args: &[&str],
    options: Value,
) -> Ran {
    meta_git_with(
        sandbox
            .command(env!("CARGO_BIN_EXE_meta-git"))
            .current_dir(process_dir),
        cwd,
        command,
        args,
        options,
    )
}

fn text(out: &Ran) -> String {
    String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr)
}

#[test]
fn clone_never_passes_a_url_to_git_as_an_option() {
    let sandbox = Sandbox::new();
    let tmp = sandbox.work();
    let marker = tmp.join("marker");
    let upload_pack = format!("--upload-pack=touch {}; false", marker.display());
    let out = meta_git(
        &sandbox,
        &tmp,
        "git clone",
        &["--", &upload_pack, "file:///nonexistent/source.git"],
        json!({}),
    );
    assert!(!out.status.success(), "{}", text(&out));
    assert!(!marker.exists(), "git ran the URL as an option");
}

#[test]
fn clone_and_commit_refuse_what_they_do_not_take() {
    let sandbox = Sandbox::new();
    let tmp = sandbox.work();
    for (command, args, problem) in [
        ("git clone", &["--parallel", "0", "u"][..], "invalid value"),
        // meta's nesting depth is meta's own: meta --depth N, before
        (
            "git clone",
            &["--meta-depth", "1", "u"],
            "as in meta -r --depth 1 git clone u",
        ),
        ("git clone", &["u", "--branch", "b"], "unexpected argument"),
        ("git commit", &["--amend"], "unexpected argument"),
    ] {
        let out = meta_git(&sandbox, &tmp, command, args, json!({}));
        assert!(!out.status.success(), "{args:?}: {}", text(&out));
        assert!(text(&out).contains(problem), "{args:?}: {}", text(&out));
    }
    // nothing was cloned
    assert!(!tmp.join("u").exists());
}

#[test]
fn clone_into_an_existing_folder_says_so_before_git_runs() {
    let sandbox = Sandbox::new();
    let tmp = sandbox.work();
    std::fs::create_dir(tmp.join("ws")).unwrap();
    std::fs::write(tmp.join("ws/.meta.yaml"), "projects: {}\n").unwrap();
    std::fs::create_dir(tmp.join("other")).unwrap();
    std::fs::write(tmp.join("other/notes"), "").unwrap();
    std::fs::create_dir(tmp.join("empty")).unwrap();
    for (dir, said) in [
        ("ws", "meta git update"),
        ("other", "not an empty directory"),
    ] {
        for options in [json!({}), json!({"dry_run": true})] {
            let out = meta_git(
                &sandbox,
                &tmp,
                "git clone",
                &["file:///gone.git", dir],
                options,
            );
            assert!(!out.status.success(), "{}", text(&out));
            assert!(text(&out).contains(said), "{}", text(&out));
            // before git, and before saying it clones
            assert!(!text(&out).contains("Cloning"), "{}", text(&out));
            assert!(!text(&out).contains("fatal"), "{}", text(&out));
        }
    }
    // git takes an empty directory
    let out = meta_git(
        &sandbox,
        &tmp,
        "git clone",
        &["file:///gone.git", "empty"],
        json!({}),
    );
    assert!(text(&out).contains("fatal"), "{}", text(&out));
}

#[test]
fn clone_and_commit_have_their_own_help() {
    let sandbox = Sandbox::new();
    let tmp = sandbox.work();
    // meta sends a help request without its repos, so `-m --help` is help
    // too, never a commit of every repo
    for (command, args) in [
        ("git clone", &["--help"][..]),
        ("git commit", &["--help"]),
        ("git commit", &["-m", "--help"]),
    ] {
        let out = meta_git(&sandbox, &tmp, command, args, json!({}));
        assert!(out.status.success(), "{}", text(&out));
        assert!(
            text(&out).contains(&format!("Usage: meta {command}")),
            "{}",
            text(&out)
        );
    }
}

#[test]
fn clone_history_depth_is_the_commands_own() {
    let sandbox = Sandbox::new();
    let tmp = sandbox.work();
    let dry_run = |args: &[&str]| {
        let out = meta_git(
            &sandbox,
            &tmp,
            "git clone",
            args,
            json!({"dry_run": true, "recursive": true, "depth": 3}),
        );
        assert!(out.status.success(), "{}", text(&out));
        text(&out)
    };
    // meta's --depth is how deep into nested meta repos, never git's
    assert!(dry_run(&["u"]).contains("git clone -- u u"));
    assert!(dry_run(&["--depth", "1", "u", "d"]).contains("git clone --depth 1 -- u d"));
    // the directory git would name, for an IPv6 host too
    assert!(dry_run(&["git@[::1]:repo.git"]).contains(":repo.git' repo"));
    // git reads a `:` as a separator too
    assert!(dry_run(&["git@host:a:b.git"]).contains("-- git@host:a:b.git b\n"));
    #[cfg(unix)]
    {
        std::fs::write(tmp.join("h:ws.bundle"), "").unwrap();
        assert!(dry_run(&["h:ws.bundle"]).contains("-- h:ws.bundle ws\n"));
    }
    // a bundle file clones into its name without `.bundle`, as git does,
    // and keeps a `.git` (from another folder: git won't clone onto it)
    std::fs::create_dir(tmp.join("src")).unwrap();
    for (bundle, dir) in [("src/ws.bundle", "ws"), ("src/other.git", "other.git")] {
        std::fs::write(tmp.join(bundle), "").unwrap();
        assert!(
            dry_run(&[bundle]).contains(&format!("-- {bundle} {dir}")),
            "{bundle}"
        );
    }
    // a linked worktree's `.git` file is no bundle: its checkout's name
    std::fs::create_dir(tmp.join("src/wt")).unwrap();
    std::fs::write(tmp.join("src/wt/.git"), "gitdir: elsewhere\n").unwrap();
    assert!(dry_run(&["src/wt/.git"]).contains("-- src/wt/.git wt"));
}

#[test]
fn worktree_list_finds_the_workspace_of_the_requests_directory() {
    let sandbox = Sandbox::new();
    let tmp = sandbox.work();
    let workspace = tmp.join("workspace");
    let repo = workspace.join("a");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        workspace.join(".meta.yaml"),
        "projects:\n  a:\n    path: a\n",
    )
    .unwrap();
    sandbox.repo(&repo);
    let set_member = workspace.join(".worktrees/s/a");
    sandbox.git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "s",
            &set_member.to_string_lossy(),
        ],
    );
    let elsewhere = tmp.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();

    let out = meta_git_from(
        &sandbox,
        &elsewhere,
        &workspace,
        "git worktree list",
        &[],
        json!({"json": true}),
    );
    assert!(out.status.success(), "{}", text(&out));
    let listed: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listed["results"][0]["data"]["name"], "s", "{listed}");
}

/// A plugin whose stdout meta no longer reads (meta was killed) ends
/// quietly, without a panic.
#[test]
fn a_closed_stdout_ends_the_plugin_without_a_panic() {
    let sandbox = Sandbox::new();
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = sandbox
        .command(env!("CARGO_BIN_EXE_meta-git"))
        .arg("--meta-plugin-info")
        .stdout(writer)
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(out.status.code(), Some(141), "{stderr}");
}
