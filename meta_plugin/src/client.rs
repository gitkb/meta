//! Calling meta back: a plugin (or any program meta started) runs a meta
//! command and reads its one [`Envelope`], as any caller of `meta --json`
//! does.
//!
//! meta sets [`EXE_VAR`] (its own executable) and [`DEPTH_VAR`] (how many
//! meta calls are around this one) for every program it starts. A call
//! starts that executable one level deeper, in a process group of its own
//! ([`crate::process::spawn_group`]), so [`Running::stop`] stops it and all
//! it started. meta refuses to start deeper than [`MAX_DEPTH`]: a plugin
//! that calls itself through meta ends there, with an error.
//!
//! ```no_run
//! # fn main() -> anyhow::Result<()> {
//! use meta_plugin::client::Meta;
//! let envelope = Meta::from_env()?
//!     .cwd("/w/app")
//!     .args(["project", "list"])
//!     .json()?
//!     .wait()?;
//! for row in &envelope.results {
//!     println!("{:?}", row.project);
//! }
//! # Ok(())
//! # }
//! ```

use crate::display::escape_path;
use crate::envelope::Envelope;
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};

/// The variable holding the meta executable that started this program.
pub const EXE_VAR: &str = "META_EXE";
/// The variable holding how many meta calls are around this program's (0
/// for the outermost meta).
pub const DEPTH_VAR: &str = "META_DEPTH";
/// The deepest a meta may run inside meta calls: no loop of calls goes on
/// past it.
pub const MAX_DEPTH: u32 = 8;
/// The most a call's Envelope may hold: a bigger one is refused, not cut.
pub const MAX_ENVELOPE: u64 = 64 << 20;

/// How many meta calls are around this program (0 when none).
pub fn depth() -> Result<u32> {
    match std::env::var(DEPTH_VAR) {
        Err(std::env::VarError::NotPresent) => Ok(0),
        Ok(text) => text
            .parse()
            .with_context(|| format!("{DEPTH_VAR} is {text:?}, not a number")),
        Err(e) => Err(e).with_context(|| format!("{DEPTH_VAR} can't be read")),
    }
}

/// A meta command to run.
#[derive(Debug, Clone)]
pub struct Meta {
    exe: PathBuf,
    depth: u32,
    cwd: Option<PathBuf>,
    args: Vec<OsString>,
    lease: Option<String>,
}

impl Meta {
    /// The meta that started this program. An error when none did (no
    /// [`EXE_VAR`]), and when the call would be deeper than [`MAX_DEPTH`].
    pub fn from_env() -> Result<Self> {
        let exe = std::env::var_os(EXE_VAR).map(PathBuf::from).with_context(|| {
            format!("{EXE_VAR} is not set: only a program meta started can call meta back")
        })?;
        let depth = depth()? + 1;
        if depth > MAX_DEPTH {
            anyhow::bail!(
                "meta calls meta more than {MAX_DEPTH} deep: a command calls itself through meta"
            );
        }
        Ok(Meta {
            exe,
            depth,
            cwd: None,
            args: Vec::new(),
            lease: None,
        })
    }

    /// Run it from `dir` (meta finds the workspace from there).
    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cwd = Some(dir.into());
        self
    }

    /// Its arguments: meta's options first, then the command and its own
    /// (`["--dry-run", "git", "status"]`).
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Pass on the lease this program holds (a [`crate::sets::lease::Held`]'s
    /// token): the command treats the set as its caller's.
    pub fn lease(mut self, token: impl Into<String>) -> Self {
        self.lease = Some(token.into());
        self
    }

    fn command(&self, json: bool) -> Command {
        #[expect(
            clippy::disallowed_methods,
            reason = "meta itself, started with no repository's git env"
        )]
        let mut command = Command::new(&self.exe);
        crate::process::clean_env(&mut command);
        if json {
            command.arg("--json");
        }
        command.args(&self.args).env(DEPTH_VAR, self.depth.to_string());
        match &self.lease {
            Some(token) => command.env(crate::LEASE_VAR, token),
            None => command.env_remove(crate::LEASE_VAR),
        };
        if let Some(dir) = &self.cwd {
            command.current_dir(dir);
        }
        command
    }

    /// Start it with `--json`: its stdout is read whole into its Envelope
    /// (see [`Running::wait`]); its stderr is this program's.
    pub fn json(self) -> Result<Running> {
        let mut command = self.command(true);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = crate::process::spawn_group(&mut command)
            .with_context(|| format!("Cannot start {}", escape_path(&self.exe)))?;
        let stdout = child.stdout();
        let reader = std::thread::Builder::new()
            .name("meta-client".to_string())
            .spawn(move || read_bounded(stdout))
            .context("Cannot start a thread to read meta's output")?;
        Ok(Running {
            child,
            reader: Some(reader),
        })
    }

    /// Run it with this program's stdin, stdout and stderr, as typed (for a
    /// command whose output the user watches), and its exit status.
    pub fn inherit(self) -> Result<ExitStatus> {
        let mut command = self.command(false);
        crate::process::spawn_group(&mut command)
            .and_then(|mut child| child.wait())
            .with_context(|| format!("Cannot run {}", escape_path(&self.exe)))
    }
}

/// What a call printed: its stdout, up to [`MAX_ENVELOPE`] bytes.
fn read_bounded(stdout: Option<std::process::ChildStdout>) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    if let Some(pipe) = stdout {
        pipe.take(MAX_ENVELOPE + 1).read_to_end(&mut bytes)?;
    }
    Ok(bytes)
}

/// A `meta --json` call that is running.
pub struct Running {
    child: crate::process::Owned,
    reader: Option<std::thread::JoinHandle<std::io::Result<Vec<u8>>>>,
}

impl Running {
    /// Wait for it to end, and read its Envelope. A call that prints none
    /// (meta was killed, say) is an error with its exit status; one that
    /// printed more than [`MAX_ENVELOPE`] is stopped and refused.
    pub fn wait(mut self) -> Result<Envelope> {
        let read = self
            .reader
            .take()
            .map(|reader| {
                reader
                    .join()
                    .unwrap_or_else(|_| Err(std::io::Error::other("the reading thread panicked")))
            })
            .unwrap_or_else(|| Ok(Vec::new()));
        let too_big = read
            .as_ref()
            .is_ok_and(|bytes| bytes.len() as u64 > MAX_ENVELOPE);
        if too_big {
            self.child.stop(true);
        }
        let status = self.child.wait().context("Cannot wait for meta")?;
        let bytes = read.context("Cannot read meta's output")?;
        if too_big {
            anyhow::bail!(
                "meta printed more than {} MiB of JSON; refused, not cut",
                MAX_ENVELOPE >> 20
            );
        }
        serde_json::from_slice(&bytes).with_context(|| {
            format!("meta ended ({status}) without an Envelope on its stdout")
        })
    }

    /// Stop it and everything it started (SIGTERM to its process group; on
    /// Windows its job is ended). [`Running::wait`] then says how it ended.
    pub fn stop(&mut self) {
        self.child.stop(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_goes_one_level_deeper_and_stops_past_the_deepest() {
        let call = |depth: Option<&str>| {
            let mut env = vec![(EXE_VAR, Some("/bin/meta"))];
            env.push((DEPTH_VAR, depth));
            temp_env(&env, Meta::from_env)
        };
        assert_eq!(call(None).unwrap().depth, 1);
        assert_eq!(call(Some("7")).unwrap().depth, 8);
        let refused = call(Some("8")).unwrap_err().to_string();
        assert!(refused.contains("more than 8 deep"), "{refused}");
        assert!(call(Some("x")).is_err());
        let refused = temp_env(&[(EXE_VAR, None)], Meta::from_env)
            .unwrap_err()
            .to_string();
        assert!(refused.contains("META_EXE is not set"), "{refused}");
    }

    #[test]
    fn a_call_passes_its_depth_and_only_the_lease_it_was_given() {
        let meta = Meta {
            exe: PathBuf::from("meta"),
            depth: 3,
            cwd: Some(PathBuf::from("/w")),
            args: vec!["exec".into()],
            lease: None,
        };
        let envs = |command: &Command| -> Vec<(String, Option<String>)> {
            command
                .get_envs()
                .map(|(k, v)| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.map(|v| v.to_string_lossy().into_owned()),
                    )
                })
                .collect()
        };
        let plain = meta.command(true);
        let args: Vec<_> = plain.get_args().collect();
        assert_eq!(args, ["--json", "exec"]);
        assert!(envs(&plain).contains(&(DEPTH_VAR.into(), Some("3".into()))));
        // one this program inherited is not passed on unasked
        assert!(envs(&plain).contains(&(crate::LEASE_VAR.into(), None)));
        let leased = meta.lease("tok").command(false);
        assert!(envs(&leased).contains(&(crate::LEASE_VAR.into(), Some("tok".into()))));
    }

    /// Run `f` with `vars` set (`None`: unset) in this test's environment.
    /// The tests that touch these variables hold one lock.
    fn temp_env<T>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> T) -> T {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _held = LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let old: Vec<_> = vars
            .iter()
            .map(|(k, _)| (*k, std::env::var_os(k)))
            .collect();
        for (k, v) in vars {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        let out = f();
        for (k, v) in old {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        out
    }
}
