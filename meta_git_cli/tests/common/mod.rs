//! The git plugin called as meta would call it: the request meta would send,
//! the way meta starts the plugin, and what meta would make of its answer.

use meta_plugin::testing::{call, Called};
use meta_plugin::{Answer, Meta, Request, Scope};
use serde_json::{json, Value};
use std::path::Path;

/// meta's options, as a test gives them: `json!({"json": true, "strict":
/// true, "dry_run": true, "recursive": true, "all": true, "depth": 2})`.
/// `recursive`/`all`/`depth` are how meta resolves the workspace and how
/// deep (meta sends those as `request.depth` and the workspace it found);
/// the rest are `request.meta`.
///
/// A config meta refuses fails the command as meta fails it, except for a
/// command that finds its repos itself (`git worktree list`),
/// which meta then sends no workspace.
fn request(
    cwd: &Path,
    command: &str,
    args: &[&str],
    options: &Value,
    worktrees: Option<&Path>,
) -> Result<Request, String> {
    let flag = |name: &str| options[name].as_bool().unwrap_or(false);
    let (recursive, all) = (flag("recursive"), flag("all"));
    let mut request = Request::new(command, cwd);
    request.args = args.iter().map(|a| a.to_string()).collect();
    let verb = command.split_whitespace().nth(2).or(args.first().copied());
    let named = args.first().is_some_and(|a| !a.starts_with('-'));
    request.scope = match command.split_whitespace().nth(1) {
        Some("clone") => Scope::None,
        Some("worktree")
            if named && matches!(verb, Some("exec" | "remove" | "status" | "diff" | "add")) =>
        {
            Scope::NamedSet
        }
        Some("worktree") => Scope::Workspace,
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
    let found = meta_core::config::Workspace::find(cwd, None).and_then(|found| match all {
        true => found.map(|w| w.root()).transpose(),
        false => Ok(found),
    });
    let found = match (found, request.scope) {
        (Ok(found), _) => found,
        (Err(_), Scope::Workspace) => None,
        (Err(e), _) => return Err(format!("{e:#}")),
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
    Ok(request)
}

/// The workspace `found` as meta sends it: its whole tree (from the
/// outermost config) as members.
pub fn as_meta_sends(found: meta_core::config::Workspace) -> meta_plugin::Workspace {
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
pub fn as_meta_starts(cmd: &mut std::process::Command) -> Option<std::path::PathBuf> {
    let meta = meta_plugin::testing::built_meta();
    let bin = meta.parent().unwrap();
    cmd.env(meta_plugin::client::EXE_VAR, &meta);
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
pub struct Status(Option<i32>);

impl Status {
    pub fn success(&self) -> bool {
        self.code() == Some(0)
    }
    pub fn code(&self) -> Option<i32> {
        self.0
    }
}

/// What meta would make of a plugin's answer: the plugin's own output, then
/// its warnings and error on stderr as meta says them (`warning: …`,
/// `Error: …`), a message on stdout; under `--json`, stdout is the
/// Envelope's parts (`results`, `warnings`, `message`, `error`); the code
/// meta would end with (a failed record fails the command).
pub struct Ran {
    pub status: Status,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
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
pub fn meta_git_with(
    cmd: &mut std::process::Command,
    cwd: &Path,
    command: &str,
    args: &[&str],
    options: Value,
) -> Ran {
    let worktrees = as_meta_starts(cmd);
    match request(cwd, command, args, &options, worktrees.as_deref()) {
        Ok(request) => ran(call(cmd, &request), request.meta.json),
        // meta fails before it calls the plugin
        Err(error) => Ran {
            status: Status(Some(1)),
            stdout: Vec::new(),
            stderr: format!("Error: {error}\n").into_bytes(),
        },
    }
}
