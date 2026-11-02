//! One sandbox for every test in this workspace.
//!
//! A [`Sandbox`] is a folder with a home, XDG dirs, a git config and a meta
//! data dir of its own. Every program a test starts through
//! [`Sandbox::command`] gets those instead of the user's, so a test never
//! reads or writes the user's settings (their git identity, hooks, signing,
//! plugins). Git a fixture runs fails the test unless it succeeds.
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

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The identity commits in a sandbox are made as.
pub const NAME: &str = "t";
pub const EMAIL: &str = "t@example.com";

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
        std::fs::write(root.join("gitconfig"), "").unwrap();
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
    /// ```ignore
    /// let Some(sandbox) = Sandbox::isolated() else { return };
    /// ```
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
            output.contains("1 passed"),
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
    pub fn env(&self) -> Vec<(&'static str, PathBuf)> {
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
        for name in repo_env().iter().map(String::as_str).chain(UNSET) {
            command.env_remove(name);
        }
        command.envs(self.env());
        command
    }

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

    /// A bare repo at `dir` whose `main` has one commit, to clone from.
    pub fn bare(&self, dir: &Path) -> PathBuf {
        let seed = self.root.join("seeds").join(dir.file_name().unwrap());
        self.repo(&seed);
        let parent = dir.parent().unwrap();
        std::fs::create_dir_all(parent).unwrap();
        self.git(parent, &["clone", "-q", "--bare", &path(&seed), &path(dir)]);
        dir.to_path_buf()
    }
}

/// Settings of the user's shell a test would otherwise pick up.
const UNSET: [&str; 6] = [
    "META_WORKTREES",
    "RUST_LOG",
    "NO_COLOR",
    "CLICOLOR",
    "CLICOLOR_FORCE",
    "FORCE_COLOR",
];

/// The variables that point git at a repo (`GIT_DIR`, `GIT_WORK_TREE`, …),
/// as git names them.
fn repo_env() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let out = Command::new("git")
            .args(["rev-parse", "--local-env-vars"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git rev-parse --local-env-vars: {out:?}"
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(String::from)
            .collect()
    })
}

/// Whether git can make reftable repos (2.45 on); says why not when not.
pub fn git_has_reftable() -> bool {
    let out = Command::new("git").arg("version").output().unwrap();
    let version = String::from_utf8_lossy(&out.stdout);
    // "git version 2.45.1", "git version 2.45.1.windows.1"
    let mut numbers = version
        .split_whitespace()
        .nth(2)
        .unwrap_or_default()
        .split('.')
        .map(|n| n.parse::<u32>().unwrap_or(0));
    let (major, minor) = (numbers.next().unwrap_or(0), numbers.next().unwrap_or(0));
    let has = (major, minor) >= (2, 45);
    if !has {
        eprintln!("skipped: {} has no reftable (2.45 on)", version.trim());
    }
    has
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
        assert!(global.is_empty(), "{global}");
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
        let Some(sandbox) = Sandbox::isolated() else {
            return;
        };
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
