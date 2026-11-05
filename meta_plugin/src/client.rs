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
//!     .args(["exec", "--", "pwd"])
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
/// The flag a plugin built with [`crate::run_plugin`] answers by waiting for
/// its stdin to close, then running the command after it (see
/// [`Meta::finishing`]).
pub const FINISH_FLAG: &str = "--meta-plugin-finish";

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
    env: Vec<(OsString, OsString)>,
}

impl Meta {
    /// The meta that started this program. An error when none did (no
    /// [`EXE_VAR`]), and when the call would be deeper than [`MAX_DEPTH`].
    pub fn from_env() -> Result<Self> {
        let exe = std::env::var_os(EXE_VAR).with_context(|| {
            format!("{EXE_VAR} is not set: only a program meta started can call meta back")
        })?;
        Self::new(exe)
    }

    /// The meta at `exe` (a path, or a name looked up in `PATH`), for a
    /// program that may have been started by something other than meta (an
    /// agent's MCP server). As with [`Meta::from_env`], the call is one
    /// level deeper than this program, and an error past [`MAX_DEPTH`].
    pub fn new(exe: impl Into<PathBuf>) -> Result<Self> {
        let depth = depth()? + 1;
        if depth > MAX_DEPTH {
            anyhow::bail!(
                "Calls back to meta nest more than {MAX_DEPTH} deep: a command calls itself through meta"
            );
        }
        Ok(Meta {
            exe: exe.into(),
            depth,
            cwd: None,
            args: Vec::new(),
            lease: None,
            env: Vec::new(),
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

    /// Set the variable `key` to `value` for it (`META_WORKTREES`, say).
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
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
            reason = "the meta executable itself, with no repository's git env"
        )]
        let mut command = Command::new(&self.exe);
        crate::process::clean_env(&mut command);
        if json {
            command.arg("--json");
        }
        command
            .args(&self.args)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .env(DEPTH_VAR, self.depth.to_string());
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
    /// command whose output the user watches), and its exit status. On a
    /// terminal it shares this program's process group, the terminal's
    /// foreground one, so it may read the terminal and gets the terminal's
    /// Ctrl-C itself; off one it gets a group of its own, which this
    /// program's stop reaches ([`crate::process::spawn`]).
    pub fn inherit(self) -> Result<ExitStatus> {
        self.start_inherited()?.wait()
    }

    /// [`Meta::inherit`], started: its process id is known before it ends
    /// (for [`Finishing::running`]).
    pub fn start_inherited(self) -> Result<Inherited> {
        let mut command = self.command(false);
        let child = crate::process::spawn(&mut command)
            .with_context(|| format!("Cannot run {}", escape_path(&self.exe)))?;
        Ok(Inherited { child })
    }
}

impl Meta {
    /// Start it now, to run once this program has ended, however it ends
    /// (a Ctrl-C, SIGTERM, even SIGKILL), or once [`Finishing::run`] says
    /// so: cleanup that must finish, such as removing what a cancelled
    /// command made. A copy of this program (it must be built with
    /// [`crate::run_plugin`]) waits, out of reach of every stop
    /// ([`crate::process::spawn_finishing`]), until this program's end of
    /// its stdin closes, then runs the command, its output this program's.
    pub fn finishing(self) -> Result<Finishing> {
        use std::process::Stdio;
        let waiter = std::env::current_exe().context("Cannot tell which program this is")?;
        #[expect(
            clippy::disallowed_methods,
            reason = "this program itself, which runs meta, with no repository's git env"
        )]
        let mut command = Command::new(&waiter);
        crate::process::clean_env(&mut command);
        command
            .arg(FINISH_FLAG)
            .arg(&self.exe)
            .args(&self.args)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .env(DEPTH_VAR, self.depth.to_string())
            .stdin(Stdio::piped());
        match &self.lease {
            Some(token) => command.env(crate::LEASE_VAR, token),
            None => command.env_remove(crate::LEASE_VAR),
        };
        if let Some(dir) = &self.cwd {
            command.current_dir(dir);
        }
        let mut child = crate::process::spawn_finishing(&mut command)
            .with_context(|| format!("Cannot start {}", escape_path(&waiter)))?;
        let stdin = child.stdin();
        Ok(Finishing { child, stdin })
    }
}

/// A [`Meta::inherit`] call that is running.
pub struct Inherited {
    child: crate::process::Owned,
}

impl Inherited {
    /// Its process id.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Wait for it to end: its exit status.
    pub fn wait(mut self) -> Result<ExitStatus> {
        self.child.wait().context("Cannot wait for meta")
    }
}

/// A command waiting to run once this program ends (see [`Meta::finishing`]).
pub struct Finishing {
    child: crate::process::Owned,
    /// Held open until it is time: closed, the command runs
    stdin: Option<std::process::ChildStdin>,
}

impl Finishing {
    /// Say that the run with process id `pid` (a meta this program started)
    /// works on what the command cleans up. Should this program end before
    /// it says [`Finishing::ended`] (killed outright), the waiter stops that
    /// run (SIGTERM, then SIGKILL after 10 seconds) and waits for its end
    /// before it runs the command, so the cleanup never pulls the ground
    /// from under it.
    pub fn running(&mut self, pid: u32) {
        self.say(&format!("run {pid}"));
    }

    /// Say that the run named with [`Finishing::running`] has ended.
    pub fn ended(&mut self) {
        self.say("ended");
    }

    fn say(&mut self, line: &str) {
        use std::io::Write;
        if let Some(stdin) = &mut self.stdin {
            let _ = writeln!(stdin, "{line}");
            let _ = stdin.flush();
        }
    }

    /// Run it now and wait for it: its exit status. A stop that comes
    /// meanwhile ends this program, not the command.
    pub fn run(mut self) -> Result<ExitStatus> {
        drop(self.stdin.take());
        self.child.wait().context("Cannot wait for meta")
    }
}

/// The waiter of [`Meta::finishing`]: wait for stdin to close, then run the
/// command `args` names (a program, then its arguments) and end as it ends.
pub(crate) fn finish(args: &[std::ffi::OsString]) -> Result<()> {
    use std::io::BufRead;
    // the run its caller named and didn't see end, if any
    let mut running: Option<u32> = None;
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        match line.split_once(' ') {
            Some(("run", pid)) => running = pid.trim().parse().ok(),
            _ if line == "ended" => running = None,
            _ => {}
        }
    }
    // elsewhere (Windows) the job meta put its plugin in ends that run
    #[cfg(unix)]
    if let Some(pid) = running.and_then(|pid| i32::try_from(pid).ok()) {
        stop_and_wait(pid);
    }
    #[cfg(not(unix))]
    let _ = running;
    let Some((program, args)) = args.split_first() else {
        anyhow::bail!("{FINISH_FLAG} takes the command to run");
    };
    #[expect(
        clippy::disallowed_methods,
        reason = "meta, which its caller named, with the env it was given"
    )]
    let mut command = Command::new(program);
    command.args(args).stdin(std::process::Stdio::null());
    // the lease its caller passed on is the command's (a child doesn't get
    // one unless given it)
    if let Some(token) = std::env::var_os(crate::LEASE_VAR) {
        command.env(crate::LEASE_VAR, token);
    }
    let status = crate::process::status(&mut command)
        .with_context(|| format!("Cannot run {}", escape_path(std::path::Path::new(program))))?;
    match status.success() {
        true => Ok(()),
        false => Err(crate::exit::ExitWith::of(status).into()),
    }
}

/// Stop the process `pid` (SIGTERM; SIGKILL after 10 seconds) and wait
/// for its end.
#[cfg(unix)]
fn stop_and_wait(pid: i32) {
    use rustix::process::{kill_process, test_kill_process, Pid, Signal};
    let Some(pid) = Pid::from_raw(pid) else {
        return;
    };
    let gone = || test_kill_process(pid).is_err();
    let _ = kill_process(pid, Signal::TERM);
    let start = std::time::Instant::now();
    while !gone() {
        if start.elapsed() > std::time::Duration::from_secs(10) {
            let _ = kill_process(pid, Signal::KILL);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
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
    /// Its process id.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

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
                "The call of meta printed more than {} MiB of JSON; refused, not cut",
                MAX_ENVELOPE >> 20
            );
        }
        serde_json::from_slice(&bytes).with_context(|| {
            format!("The call of meta ended ({status}) without an Envelope on its stdout")
        })
    }

    /// Whether meta has ended (its Envelope may still be being read), so a
    /// caller waiting for it can [`Running::stop`] it meanwhile.
    pub fn ended(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
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
        assert!(refused.contains("nest more than 8 deep"), "{refused}");
        assert!(call(Some("x")).is_err());
        let refused = temp_env(&[(EXE_VAR, None)], Meta::from_env)
            .unwrap_err()
            .to_string();
        assert!(refused.contains("META_EXE is not set"), "{refused}");
        // a meta named by its caller counts its depth the same way
        let named = |depth| temp_env(&[(DEPTH_VAR, depth)], || Meta::new("meta"));
        assert_eq!(named(None).unwrap().exe, PathBuf::from("meta"));
        assert_eq!(named(Some("7")).unwrap().depth, 8);
        assert!(named(Some("8")).is_err());
    }

    #[test]
    fn a_call_passes_its_depth_and_only_the_lease_it_was_given() {
        let meta = Meta {
            exe: PathBuf::from("meta"),
            depth: 3,
            cwd: Some(PathBuf::from("/w")),
            args: vec!["exec".into()],
            lease: None,
            env: Vec::new(),
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
        let _held = LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
