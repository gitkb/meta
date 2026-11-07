//! The test kit for plugins, and for meta's own tests (feature `testing`;
//! the git fixtures need `testing-git`).
//!
//! A [`Sandbox`] is a folder with a home, XDG dirs, a git config and a meta
//! data dir of its own. Every program a test starts through
//! [`Sandbox::command`] gets those instead of the user's, so a test never
//! reads or writes the user's settings (their git identity, hooks, signing,
//! plugins). Git a fixture runs fails the test unless it succeeds.
//!
//! [`Sandbox::workspace`] builds a meta workspace of repos, nested meta
//! repos and members not cloned yet; [`call`] sends a plugin a
//! [`Request`] (made with [`Request::new`]) the way meta does.
//!
//! On Windows the user's home is found without `HOME`, so there meta's own
//! files stay out of it through `META_DATA_DIR` alone.
//!
//! This is test code: a panic here is a failed test.

#![expect(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "test support: a panic is a failed test, and tests start the programs they test"
)]

use crate::Request;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The identity commits in a sandbox are made as.
const NAME: &str = "t";
const EMAIL: &str = "t@example.com";

/// A home, config and meta data dir of a test's own, removed when dropped.
pub struct Sandbox {
    root: PathBuf,
    /// None in the copy of a test that [`Sandbox::isolated`] started: the
    /// test that started it removes the folder
    _dir: Option<tempfile::TempDir>,
}

/// Names the sandbox of the copy of a test that [`Sandbox::isolated`] runs.
const ISOLATED: &str = "META_TEST_SANDBOX";

impl Default for Sandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Sandbox {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // the path programs see: macOS's temp dir is behind a symlink. Not
        // on Windows, where git refuses the `\\?\` form canonicalize gives
        let root = if cfg!(windows) {
            dir.path().to_path_buf()
        } else {
            dir.path().canonicalize().unwrap()
        };
        for sub in ["home", "config", "data", "runtime", "meta", "tmp", "work"] {
            std::fs::create_dir(root.join(sub)).unwrap();
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let private = std::fs::Permissions::from_mode(0o700);
            std::fs::set_permissions(root.join("runtime"), private).unwrap();
        }
        // whatever the machine's git defaults to, a new repo starts on main
        std::fs::write(root.join("gitconfig"), "[init]\n\tdefaultBranch = main\n").unwrap();
        Sandbox {
            root,
            _dir: Some(dir),
        }
    }

    /// For a test that runs git through the code it tests, in this process:
    /// that git would read the user's settings. Returns the sandbox in a
    /// copy of the test running in the sandbox's environment; in the test
    /// as started, runs that copy, fails unless it passes, and returns None.
    ///
    /// [`isolated!`] is the one line a test needs.
    pub fn isolated() -> Option<Sandbox> {
        if let Some(root) = std::env::var_os(ISOLATED) {
            return Some(Sandbox {
                root: root.into(),
                _dir: None,
            });
        }
        let sandbox = Sandbox::new();
        // libtest runs each test on a thread named after it
        let test = std::thread::current().name().unwrap().to_string();
        let out = sandbox
            .command(std::env::current_exe().unwrap())
            .args([test.as_str(), "--exact", "--include-ignored", "--nocapture"])
            .env(ISOLATED, &sandbox.root)
            .current_dir(std::env::current_dir().unwrap())
            .output()
            .unwrap();
        let output = String::from_utf8_lossy(&out.stdout);
        // the copy's output as this test's, which libtest shows when asked
        print!("{output}{}", String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "{test} failed in its sandbox");
        // libtest exits 0 when the name matches no test
        assert!(
            output.contains("test result: ok. 1 passed;"),
            "{test} didn't run in its sandbox"
        );
        None
    }

    /// An empty folder for the test's workspaces and repos, outside the
    /// sandbox's home.
    pub fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    /// The sandbox's home (`HOME`).
    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// The sandbox's meta data dir (`META_DATA_DIR`).
    pub fn meta_data(&self) -> PathBuf {
        self.root.join("meta")
    }

    /// What every program a test starts sees instead of the user's.
    fn env(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("HOME", self.home()),
            ("XDG_CONFIG_HOME", self.root.join("config")),
            ("XDG_DATA_HOME", self.root.join("data")),
            ("XDG_RUNTIME_DIR", self.root.join("runtime")),
            ("GIT_CONFIG_GLOBAL", self.root.join("gitconfig")),
            ("GIT_CONFIG_NOSYSTEM", "1".into()),
            ("META_DATA_DIR", self.meta_data()),
            ("TMPDIR", self.root.join("tmp")),
            ("TEMP", self.root.join("tmp")),
            ("TMP", self.root.join("tmp")),
            ("GIT_AUTHOR_NAME", NAME.into()),
            ("GIT_AUTHOR_EMAIL", EMAIL.into()),
            ("GIT_COMMITTER_NAME", NAME.into()),
            ("GIT_COMMITTER_EMAIL", EMAIL.into()),
        ]
    }

    /// `program`, to run in the sandbox: its home, config and git are the
    /// sandbox's, and nothing of the repo the tests run in reaches it. A
    /// test that wants a variable set (a poisoned `GIT_DIR`) sets it on the
    /// returned command.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = Command::new(program);
        command.current_dir(self.work());
        for name in crate::process::REPO_ENV_VARS.iter().chain(&UNSET) {
            command.env_remove(name);
        }
        command.envs(self.env());
        command
    }

    #[cfg(feature = "testing-git")]
    /// `git args` in `dir`; its stdout. The test fails unless git does.
    pub fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = self
            .command("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    #[cfg(feature = "testing-git")]
    /// A repo at `dir` on branch `main`, with one commit of a `README`
    /// that names it.
    pub fn repo(&self, dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        self.git(dir, &["init", "-q", "-b", "main"]);
        let name = dir.file_name().unwrap_or(dir.as_os_str()).to_string_lossy();
        std::fs::write(dir.join("README"), name.as_bytes()).unwrap();
        self.git(dir, &["add", "README"]);
        self.git(dir, &["commit", "-qm", "init"]);
        dir.to_path_buf()
    }

    #[cfg(feature = "testing-git")]
    /// A bare repo at `dir` whose `main` has one commit, to clone from.
    pub fn bare(&self, dir: &Path) -> PathBuf {
        // one seed per bare repo, wherever two of them share a name
        let seed = self
            .root
            .join("seeds")
            .join(dir.strip_prefix(&self.root).unwrap_or(dir));
        self.repo(&seed);
        let parent = dir.parent().unwrap();
        std::fs::create_dir_all(parent).unwrap();
        self.git(parent, &["clone", "-q", "--bare", &path(&seed), &path(dir)]);
        dir.to_path_buf()
    }
}

/// How a plugin answered a [`call`]: what it printed, how it ended, and the
/// response it wrote (none when it wrote none, or one meta couldn't read).
pub struct Called {
    pub output: std::process::Output,
    pub response: Option<crate::Response>,
}

/// `plugin --meta-plugin-exec` (a [`Sandbox::command`], with whatever else
/// the test sets on it: its folder, its variables), sent `request` the way
/// meta sends it: in a file, with the response read from another.
pub fn call(plugin: &mut Command, request: &Request) -> Called {
    let dir = tempfile::tempdir().unwrap();
    let (request_file, response_file) = (dir.path().join("request"), dir.path().join("response"));
    std::fs::write(&request_file, serde_json::to_vec(request).unwrap()).unwrap();
    let output = plugin
        .arg("--meta-plugin-exec")
        .env(crate::REQUEST_VAR, &request_file)
        .env(crate::RESPONSE_VAR, &response_file)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let response = std::fs::read(&response_file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    Called { output, response }
}

#[cfg(feature = "testing-git")]
impl Sandbox {
    /// A meta workspace at `dir`, itself a repo, to fill with projects:
    /// see [`Workspace`].
    pub fn workspace(&self, dir: &Path) -> Workspace<'_> {
        Workspace {
            sandbox: self,
            dir: dir.to_path_buf(),
            projects: Vec::new(),
        }
    }
}

/// A meta workspace being built ([`Sandbox::workspace`]). Each project is
/// a clone of a bare repo of its own under the sandbox's work folder
/// (`up/`), so `meta git update` can clone one that isn't cloned yet. The
/// workspace's `.meta.yaml`, and its `.gitignore` of the projects and
/// `.worktrees/`, are committed.
#[cfg(feature = "testing-git")]
pub struct Workspace<'a> {
    sandbox: &'a Sandbox,
    dir: PathBuf,
    projects: Vec<(String, Member)>,
}

#[cfg(feature = "testing-git")]
enum Member {
    Cloned,
    Uncloned,
    /// A meta repo of its own, with these projects
    Nested(Vec<(String, Member)>),
}

#[cfg(feature = "testing-git")]
impl<'a> Workspace<'a> {
    /// A project `name`, cloned at `name`.
    pub fn repo(mut self, name: &str) -> Self {
        self.projects.push((name.to_string(), Member::Cloned));
        self
    }

    /// A project `name` the config lists but that isn't cloned.
    pub fn uncloned(mut self, name: &str) -> Self {
        self.projects.push((name.to_string(), Member::Uncloned));
        self
    }

    /// A nested meta repo `name` (`meta: true`), cloned, with the projects
    /// `fill` gives it.
    pub fn nested(mut self, name: &str, fill: impl FnOnce(Self) -> Self) -> Self {
        let inner = fill(self.sandbox.workspace(&self.dir.join(name)));
        self.projects
            .push((name.to_string(), Member::Nested(inner.projects)));
        self
    }

    /// Make it all; the workspace's folder.
    pub fn build(self) -> PathBuf {
        let up = self.sandbox.work().join("up");
        build(self.sandbox, &self.dir, &up, &self.projects);
        self.dir
    }
}

/// The meta repo at `dir` with `projects`, their upstreams under `up`.
#[cfg(feature = "testing-git")]
fn build(sandbox: &Sandbox, dir: &Path, up: &Path, projects: &[(String, Member)]) {
    std::fs::create_dir_all(dir).unwrap();
    sandbox.git(dir, &["init", "-q", "-b", "main"]);
    let mut config = serde_json::Map::new();
    for (name, member) in projects {
        let url = up.join(format!("{name}.git"));
        sandbox.bare(&url);
        let entry = match member {
            Member::Nested(_) => serde_json::json!({ "repo": path(&url), "meta": true }),
            _ => serde_json::json!(path(&url)),
        };
        config.insert(name.clone(), entry);
        match member {
            Member::Cloned => {
                sandbox.git(dir, &["clone", "-q", &path(&url), name]);
            }
            Member::Uncloned => {}
            Member::Nested(inner) => {
                // its own projects, pushed upstream so a clone of it has them
                let clone = dir.join(name);
                sandbox.git(dir, &["clone", "-q", &path(&url), name]);
                build(sandbox, &clone, &up.join(name), inner);
                sandbox.git(&clone, &["push", "-q", "origin", "main"]);
            }
        }
    }
    let config = serde_json::json!({ "projects": config }).to_string();
    std::fs::write(dir.join(".meta.yaml"), config).unwrap();
    let ignored: Vec<&str> = projects.iter().map(|(name, _)| name.as_str()).collect();
    std::fs::write(
        dir.join(".gitignore"),
        format!("{}\n.worktrees/\n", ignored.join("\n")),
    )
    .unwrap();
    sandbox.git(dir, &["add", ".meta.yaml", ".gitignore"]);
    sandbox.git(dir, &["commit", "-qm", "meta"]);
}

/// A git config file with nothing in it, for the git the sandbox itself runs
#[cfg(feature = "testing-git")]
const NO_CONFIG: &str = if cfg!(windows) { "NUL" } else { "/dev/null" };

/// Settings of the user's shell a test would otherwise pick up.
const UNSET: [&str; 13] = [
    // the `-c` settings a git around the tests was given
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_PARAMETERS",
    crate::sets::WORKTREES_VAR,
    "META_GITHUB_URL",
    "META_PLATFORM",
    "GIT_TEMPLATE_DIR",
    "GIT_SSH_COMMAND",
    "GIT_EDITOR",
    "RUST_LOG",
    "NO_COLOR",
    "CLICOLOR",
    "CLICOLOR_FORCE",
    "FORCE_COLOR",
];

/// `let sandbox = isolated!();`: the sandbox of [`Sandbox::isolated`]; in
/// the test as started, which only runs the copy, it returns.
#[macro_export]
macro_rules! isolated {
    () => {
        match $crate::testing::Sandbox::isolated() {
            Some(sandbox) => sandbox,
            None => return,
        }
    };
}
pub use crate::isolated;

#[cfg(feature = "testing-git")]
#[expect(clippy::panic, reason = "a git this can't read fails the test")]
/// Whether git can make reftable repos (2.45 on); says why not when not.
pub fn git_has_reftable() -> bool {
    let out = Command::new("git")
        .arg("version")
        .env("GIT_CONFIG_GLOBAL", NO_CONFIG)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "git version: {out:?}");
    let version = String::from_utf8_lossy(&out.stdout);
    // "git version 2.45.1", "git version 2.45.1.windows.1", "git version
    // 2.39.5 (Apple Git-154)": a version this can't read fails the test
    let numbers: Vec<u32> = version
        .split_whitespace()
        .nth(2)
        .unwrap_or_default()
        .split('.')
        .take(2)
        .map(|n| n.parse().unwrap())
        .collect();
    let [major, minor] = numbers[..] else {
        panic!("can't read {version:?}")
    };
    let has = (major, minor) >= (2, 45);
    if !has {
        eprintln!("skipped: {} has no reftable (2.45 on)", version.trim());
    }
    has
}

/// Waits until `done` holds, failing the test after 20 seconds.
#[cfg(unix)]
pub fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "waited too long for {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Whether the process `pid` is gone: not running, and reaped.
#[cfg(unix)]
pub fn gone(pid: &str) -> bool {
    !Command::new("kill")
        .args(["-0", pid])
        .output()
        .unwrap()
        .status
        .success()
}

/// Makes a named pipe at `path`; false, saying why, where there's no
/// `mkfifo` to make one. Fails the test if `mkfifo` is there and fails.
#[cfg(unix)]
pub fn mkfifo(path: &Path) -> bool {
    match Command::new("mkfifo").arg(path).status() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipped: no mkfifo to make {}", path.display());
            false
        }
        status => {
            assert!(status.unwrap().success(), "mkfifo {}", path.display());
            true
        }
    }
}

/// `path` as an argument
pub fn path(path: &Path) -> String {
    path.to_str().unwrap().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_see_the_sandbox_not_the_user() {
        let sandbox = Sandbox::new();
        let repo = sandbox.repo(&sandbox.work().join("r"));
        let global = sandbox.git(&repo, &["config", "--global", "--show-origin", "--list"]);
        assert_eq!(global.lines().count(), 1, "{global}");
        assert!(global.contains("init.defaultbranch=main"), "{global}");
        sandbox.git(&repo, &["config", "--global", "x.y", "z"]);
        let written = std::fs::read_to_string(sandbox.root.join("gitconfig")).unwrap();
        assert!(written.contains("y = z"), "{written}");
        let author = sandbox.git(&repo, &["log", "-1", "--format=%an <%ae>"]);
        assert_eq!(author.trim(), format!("{NAME} <{EMAIL}>"));
        assert_eq!(
            sandbox.git(&repo, &["branch", "--show-current"]).trim(),
            "main"
        );
    }

    #[test]
    fn an_isolated_test_runs_with_the_sandbox_as_its_environment() {
        let sandbox = isolated!();
        assert_eq!(std::env::var_os("HOME"), Some(sandbox.home().into()));
        let global = std::env::var_os("GIT_CONFIG_GLOBAL").unwrap();
        assert_eq!(Path::new(&global), sandbox.root.join("gitconfig"));
        assert!(std::env::var_os("GIT_DIR").is_none());
    }

    #[test]
    fn a_failed_fixture_step_fails_the_test() {
        let sandbox = Sandbox::new();
        let failed = std::panic::catch_unwind(|| sandbox.git(&sandbox.work(), &["log"]));
        assert!(failed.is_err());
    }
}
