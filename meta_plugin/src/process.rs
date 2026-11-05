//! The processes a meta program starts, and what happens to them when it is
//! told to stop.
//!
//! On Unix, [`own`] makes the calling program the one owner of SIGINT,
//! SIGTERM and SIGHUP (those not already ignored when it started, as under
//! `nohup`; where that can't be told without unsafe code, everywhere but
//! Linux, SIGHUP is left as it was). Children started through [`spawn`] or
//! [`spawn_group`] are registered. When one of those signals arrives, no new
//! child starts, the signal is passed on to the
//! children, and once they are gone the program ends the way the signal ends
//! it, so a shell running it sees it was interrupted. Threads waiting on a
//! child then never resume: nothing is printed or done after the signal. A
//! second signal kills every child at once and ends the program.
//!
//! Where the program is the foreground job of a terminal, its children stay
//! in its process group, as before: they can prompt on the terminal, and the
//! terminal's Ctrl-C and hangup reach them directly; a signal another process
//! sends to the program alone is passed on to each of them, and that signal
//! or a hangup (no one is left to press Ctrl-C twice) gets the same bound as
//! below. Anywhere else (no terminal, a background job, a supervisor), each
//! child gets a process group of its own, so the signal reaches everything
//! it starts in turn; as there's no one to press Ctrl-C twice, what is still
//! running two seconds
//! later is killed (one second in a program meta started, so it kills its
//! own first), and two seconds after that the program ends regardless.
//!
//! SIGKILL can't be caught. Sent to the program's process group (as some CI
//! runners and `timeout -k` do), it ends the program but not the children in
//! groups of their own, which run on as orphans (a `git clone`, a command of
//! `meta exec`). A supervisor that kills a whole job (a cgroup, a job
//! object) stops them too.
//!
//! Elsewhere (Windows), children start as they always did; Ctrl-C reaches
//! every process attached to the console.
//!
//! A child started through [`spawn_group`] instead always gets a process
//! group of its own (a job object on Windows), wherever the program runs, so
//! the caller can tell whether anything it started still runs
//! ([`Owned::group_alive`]) and stop all of it ([`Owned::stop`]): a tool
//! call that is cancelled, a plugin that doesn't answer in time. It doesn't
//! get the terminal's Ctrl-C directly; on Unix a signal this program owns is
//! passed on to it as to any other group. On Windows a process the child
//! starts in the instant before it is put in its job is not in it.

use std::io;
use std::process::{Child, Command, ExitStatus, Output};

/// The variables that tie git to one repository, which git itself clears
/// when it runs in another (`git rev-parse --local-env-vars`, less the
/// `-c`/`GIT_CONFIG_COUNT` settings git keeps for a submodule: they are the
/// user's, not the repository's). A git hook or alias sets some of them
/// (`GIT_DIR`, `GIT_INDEX_FILE`); left in place, git would act on that
/// repository from every directory meta runs it in.
pub const REPO_ENV_VARS: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CONFIG",
    "GIT_DIR",
    "GIT_GRAFT_FILE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_INTERNAL_SUPER_PREFIX",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_OBJECT_DIRECTORY",
    "GIT_PREFIX",
    "GIT_REPLACE_REF_BASE",
    "GIT_SHALLOW_FILE",
    "GIT_WORK_TREE",
];

/// Remove [`REPO_ENV_VARS`] from what `command` will inherit: every
/// program started in a repository folder needs it, as does any program
/// a git hook starts.
pub fn clean_env(command: &mut Command) -> &mut Command {
    for key in REPO_ENV_VARS {
        command.env_remove(key);
    }
    command
}

/// A child started through [`spawn`] or [`spawn_group`]. Wait for it through [`Owned::wait`]
/// or [`Owned::exchange`]. One dropped unwaited for is let go: a
/// stop no longer waits for it (its process group still gets the signal).
pub struct Owned {
    child: Child,
    /// Off the list: waited for (its pid may now be another child's), or
    /// never on it ([`spawn_finishing`])
    waited: bool,
    /// Its process group or job, when started through [`spawn_group`]
    group: Option<imp::Group>,
}

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.waited {
            imp::let_go(&self.child);
        }
    }
}

/// Start `command` as a child this program owns. Fails with
/// [`io::ErrorKind::Interrupted`] once the program is stopping.
pub fn spawn(command: &mut Command) -> io::Result<Owned> {
    start(command, false)
}

/// Start `command` as a child this program owns, in a process group of its
/// own (a job object on Windows), so everything it starts can be stopped
/// with it (see the module documentation).
pub fn spawn_group(command: &mut Command) -> io::Result<Owned> {
    start(command, true)
}

/// Start `command`, cleanup that must finish (a rollback, removing what a
/// cancelled command made), out of reach of a stop: in a process group of
/// its own (on Windows, a console process group of its own, out of the
/// jobs of [`spawn_group`] where they let it), which the terminal's Ctrl-C
/// and every signal this program passes on miss. It starts even once the
/// program is stopping, and waiting for it is waiting for it alone: if the
/// program ends first, the cleanup runs on to its end.
pub fn spawn_finishing(command: &mut Command) -> io::Result<Owned> {
    imp::spawn_finishing(command).map(|child| Owned {
        child,
        waited: true,
        group: None,
    })
}

fn start(command: &mut Command, grouped: bool) -> io::Result<Owned> {
    // a plugin's call is its own: what it starts never writes its answer
    // or its events, unless the call is that program's (meta calling a
    // plugin sets them)
    for var in [
        crate::REQUEST_VAR,
        crate::RESPONSE_VAR,
        crate::event::EVENTS_VAR,
    ] {
        if !command.get_envs().any(|(name, _)| name == var) {
            command.env_remove(var);
        }
    }
    imp::spawn(command, grouped).map(|(child, group)| Owned {
        child,
        waited: false,
        group,
    })
}

/// Run `command` to completion, as [`Command::status`] does.
pub fn status(command: &mut Command) -> io::Result<ExitStatus> {
    spawn(command)?.wait()
}

/// Run `command` to completion capturing its output, as [`Command::output`]
/// does, with stdin from nothing.
pub fn output(command: &mut Command) -> io::Result<Output> {
    use std::process::Stdio;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    spawn(command)?.exchange(&[])
}

impl Owned {
    /// The child's stdout, if piped and not taken yet.
    pub fn stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child.stdout.take()
    }

    /// The child's stderr, if piped and not taken yet.
    pub fn stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child.stderr.take()
    }

    /// Wait for the child to exit.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        if !self.waited {
            imp::exited(&self.child);
            self.waited = true;
        }
        let status = self.child.wait();
        imp::reaped();
        status
    }

    /// The child's exit status if it has exited, without waiting, as
    /// [`Child::try_wait`] does.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if !self.waited {
            if !imp::try_exited(&self.child) {
                return Ok(None);
            }
            self.waited = true;
        }
        let status = self.child.try_wait();
        imp::reaped();
        status
    }

    /// Whether anything is left of the child's process group or job: the
    /// child itself, or anything it started that is still running. A child
    /// not started through [`spawn_group`] has no group of its own: `false`.
    pub fn group_alive(&mut self) -> bool {
        match &mut self.group {
            Some(group) => imp::group_alive(group, &mut self.child),
            None => false,
        }
    }

    /// Ask the child and everything in its process group to stop (SIGTERM),
    /// or kill them (`kill`). On Windows the whole job is killed either way.
    /// A child not started through [`spawn_group`] is killed alone.
    pub fn stop(&mut self, kill: bool) {
        match &mut self.group {
            Some(group) => imp::stop_group(group, &mut self.child, kill),
            None => {
                let _ = self.child.kill();
            }
        }
    }

    /// Write `input` to the child's piped stdin and close it, while reading
    /// its piped stdout and stderr to their end, then wait for it, as
    /// [`Child::wait_with_output`] does: neither side can block the other on
    /// a full pipe. A child that ends without reading all its input is no
    /// error; its exit status says how it went. Whatever goes wrong, the
    /// child is waited for (killed first if its output can't be read).
    pub fn exchange(mut self, input: &[u8]) -> io::Result<Output> {
        use std::io::{Read, Write};
        use std::thread::{Builder, ScopedJoinHandle};
        fn read_all(mut pipe: impl Read) -> io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).map(|_| bytes)
        }
        fn joined<T>(thread: ScopedJoinHandle<'_, io::Result<T>>) -> io::Result<T> {
            thread
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("a pipe's thread panicked")))
        }
        let stdin = self.child.stdin.take();
        let stdout = self.child.stdout.take();
        let stderr = self.child.stderr.take();
        let child = &mut self.child;
        let served: io::Result<_> = std::thread::scope(|scope| {
            // a pipe that can't be served is let go by killing the child, so
            // nothing waits on it
            let mut kill = |e: io::Error| {
                let _ = child.kill();
                e
            };
            // dropped once written: the child reads its end
            let sent = stdin
                .map(|mut pipe| {
                    Builder::new().spawn_scoped(scope, move || match pipe.write_all(input) {
                        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
                        sent => sent,
                    })
                })
                .transpose()
                .map_err(&mut kill)?;
            let stderr = stderr
                .map(|pipe| Builder::new().spawn_scoped(scope, move || read_all(pipe)))
                .transpose()
                .map_err(&mut kill)?;
            let stdout = stdout.map(read_all).transpose().map_err(&mut kill)?;
            let stderr = stderr.map(joined).transpose().map_err(&mut kill)?;
            sent.map(joined).transpose()?;
            Ok((stdout.unwrap_or_default(), stderr.unwrap_or_default()))
        });
        let status = self.wait()?;
        let (stdout, stderr) = served?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

/// Make this program the owner of its children's signals (see the module
/// documentation). Call it once, first thing in `main`.
pub fn own() -> io::Result<()> {
    imp::own()
}

#[cfg(unix)]
mod imp {
    use rustix::process::{Pid, Signal};
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};
    use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
    use std::time::{Duration, Instant};

    /// How long children with no terminal get to stop before they're killed,
    /// in meta itself. A program meta started (a plugin) gives its own
    /// children half as long, so it has killed them before meta kills it.
    const GRACE: Duration = Duration::from_secs(2);

    /// How deep this program is among owners: 0 for meta, 1 for a program
    /// it started, and so on.
    const DEPTH: &str = "META_OWNER_DEPTH";

    fn depth() -> u32 {
        std::env::var(DEPTH)
            .ok()
            .and_then(|d| d.parse().ok())
            .unwrap_or(0)
    }

    fn grace() -> Duration {
        GRACE / 2u32.saturating_pow(depth().min(3))
    }

    #[derive(Default)]
    struct Children {
        /// Each running child, and whether it leads a process group of its own.
        running: Vec<(Pid, bool)>,
        /// The process groups this program made that may still have members.
        groups: Vec<Pid>,
        /// A signal came: this program is stopping.
        stopping: bool,
    }

    struct Registry {
        children: Mutex<Children>,
        changed: Condvar,
        /// Whether children share this program's process group: it was the
        /// foreground job of a terminal when it took ownership.
        shared: OnceLock<bool>,
    }

    fn registry() -> &'static Registry {
        static REGISTRY: OnceLock<Registry> = OnceLock::new();
        REGISTRY.get_or_init(|| Registry {
            children: Mutex::new(Children::default()),
            changed: Condvar::new(),
            shared: OnceLock::new(),
        })
    }

    fn lock() -> MutexGuard<'static, Children> {
        // a thread that panicked holding the lock left the lists consistent
        registry()
            .children
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether children share this program's process group (see [`own`]).
    fn shared() -> bool {
        *registry().shared.get_or_init(|| {
            std::fs::File::open("/dev/tty")
                .ok()
                .and_then(|tty| rustix::termios::tcgetpgrp(&tty).ok())
                .is_some_and(|group| group == rustix::process::getpgrp())
        })
    }

    fn alive(group: Pid) -> bool {
        // A group's id can't be another's while any of it lives. Accepted:
        // once it is empty its id could be reused before the next check
        // (groups are checked whenever a child is started or reaped, and
        // while a stopped group is waited for), which needs pid wraparound
        // in between.
        rustix::process::test_kill_process_group(group).is_ok()
    }

    /// The process group a child leads.
    pub(super) type Group = Pid;

    pub(super) fn group_alive(group: &mut Group, _child: &mut Child) -> bool {
        alive(*group)
    }

    pub(super) fn stop_group(group: &mut Group, _child: &mut Child, kill: bool) {
        let signal = if kill { Signal::KILL } else { Signal::TERM };
        // its group lives on while the child or anything it started runs
        let _ = rustix::process::kill_process_group(*group, signal);
    }

    /// Start `command`, in a process group of its own when there is no
    /// terminal to share or `grouped` asks for one; the group is returned
    /// only when asked for.
    pub(super) fn spawn(
        command: &mut Command,
        grouped: bool,
    ) -> io::Result<(Child, Option<Group>)> {
        let own_group = grouped || !shared();
        let mut children = lock();
        if children.stopping {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "stopping: nothing new starts",
            ));
        }
        if own_group {
            command.process_group(0);
        }
        command.env(DEPTH, (depth() + 1).to_string());
        let child = command.spawn()?;
        let pid = Pid::from_child(&child);
        children.running.push((pid, own_group));
        if own_group {
            children.groups.retain(|group| alive(*group));
            children.groups.push(pid);
        }
        // held from the check above to here: a signal that stops this
        // program either came first or sees the child
        drop(children);
        Ok((child, grouped.then_some(pid)))
    }

    /// Start `command` in a process group of its own, off the lists: no
    /// stop signals it, and it starts while the program is stopping.
    pub(super) fn spawn_finishing(command: &mut Command) -> io::Result<Child> {
        command.process_group(0);
        command.env(DEPTH, (depth() + 1).to_string());
        command.spawn()
    }

    /// Forget the process groups that have emptied: once one has, its id
    /// may become another's.
    pub(super) fn reaped() {
        lock().groups.retain(|group| alive(*group));
    }

    /// Take `child` off the list of running children, if it is still there.
    pub(super) fn let_go(child: &Child) {
        let pid = Pid::from_child(child);
        lock().running.retain(|(running, _)| *running != pid);
        registry().changed.notify_all();
    }

    /// Wait until `child` has exited, without reaping it, then take it off
    /// the list: the owner never signals a reaped pid, which may be reused.
    /// When the program is stopping, this reaps it (so its process group can
    /// empty) and never returns.
    pub(super) fn exited(child: &Child) {
        use rustix::process::{waitid, WaitId, WaitIdOptions};
        let pid = Pid::from_child(child);
        loop {
            match waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
            ) {
                Err(rustix::io::Errno::INTR) => {}
                // macOS can answer for a child that is only stopped
                Ok(Some(status)) if status.stopped() || status.continued() => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                // ended, or not this program's to wait for (already reaped):
                // std's wait reports that
                _ => break,
            }
        }
        gone(pid);
    }

    /// Whether `child` has exited, without waiting or reaping it; once it
    /// has, as [`exited`].
    pub(super) fn try_exited(child: &Child) -> bool {
        use rustix::process::{waitid, WaitId, WaitIdOptions};
        let pid = Pid::from_child(child);
        match waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG,
        ) {
            // still running; macOS can answer for a child that is only stopped
            Ok(None) | Err(rustix::io::Errno::INTR) => return false,
            Ok(Some(status)) if status.stopped() || status.continued() => return false,
            // ended, or not this program's to wait for: std's try_wait says
            _ => {}
        }
        gone(pid);
        true
    }

    /// Take the exited child `pid` off the list (see [`exited`]).
    fn gone(pid: Pid) {
        let mut children = lock();
        children.running.retain(|(running, _)| *running != pid);
        registry().changed.notify_all();
        if children.stopping {
            drop(children);
            let _ = rustix::process::waitpid(Some(pid), rustix::process::WaitOptions::empty());
            loop {
                std::thread::park();
            }
        }
    }

    /// The signals this program started out ignoring, as a mask (bit
    /// `n - 1` for signal `n`), if it can tell: only Linux says so without
    /// unsafe code (`/proc`).
    fn ignored() -> Option<u64> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let hex = status
            .lines()
            .find_map(|line| line.strip_prefix("SigIgn:"))?;
        u64::from_str_radix(hex.trim(), 16).ok()
    }

    pub(super) fn own() -> io::Result<()> {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        use signal_hook::iterator::exfiltrator::WithOrigin;
        let listened: Vec<i32> = match ignored() {
            Some(mask) => [SIGINT, SIGTERM, SIGHUP]
                .into_iter()
                .filter(|s| {
                    mask.checked_shr(s.unsigned_abs() - 1)
                        .is_some_and(|m| m & 1 == 0)
                })
                .collect(),
            // Unknown whether it started ignoring SIGHUP (`nohup`): leave it
            // as it was. A terminal's hangup reaches the children anyway.
            None => vec![SIGINT, SIGTERM],
        };
        // decided once: a job moved between foreground and background
        // keeps its children's groups
        shared();
        let mut signals = signal_hook::iterator::SignalsInfo::<WithOrigin>::new(listened)?;
        std::thread::spawn(move || {
            let mut stopping = false;
            for origin in signals.forever() {
                if stopping {
                    kill_all();
                    crate::exit::now(crate::exit::signalled(origin.signal));
                }
                stopping = true;
                std::thread::spawn(move || stop(origin.signal, origin.process.is_some()));
            }
        });
        Ok(())
    }

    /// Pass `signal` on, wait for every child, then end as `signal` ends a
    /// program. `sent` says another process sent it, rather than the
    /// terminal, whose Ctrl-C reaches the children in this program's group
    /// too.
    fn stop(signal: i32, sent: bool) {
        let mut children = lock();
        children.stopping = true;
        // the three signals `own` listens for all have names
        let sig = Signal::from_named_raw(signal).unwrap_or(Signal::TERM);
        // a group that emptied since its last spawn is not signalled: its id
        // may be another's by now
        children.groups.retain(|group| alive(*group));
        // A hangup may have reached only this program (the session leader of
        // a terminal that went), so it is passed on too
        let hangup = signal == signal_hook::consts::SIGHUP;
        for (pid, own_group) in &children.running {
            if !own_group && (sent || hangup) {
                let _ = rustix::process::kill_process(*pid, sig);
            }
        }
        for group in &children.groups {
            let _ = rustix::process::kill_process_group(*group, sig);
        }
        // With no terminal, when another process sent it, or on a hangup
        // (the terminal is gone), what still runs after the grace is killed,
        // and after as long again this program ends whatever is left (a dead
        // process no one reaps still counts as in its group). After the
        // terminal's own Ctrl-C, a second one does that.
        let mut deadline = (sent || hangup || !shared()).then(|| Instant::now() + grace());
        let mut killed = false;
        loop {
            children.groups.retain(|group| alive(*group));
            if children.running.is_empty() && children.groups.is_empty() {
                break;
            }
            if deadline.is_some_and(|at| Instant::now() >= at) {
                if killed {
                    break;
                }
                kill(&children);
                killed = true;
                deadline = Some(Instant::now() + grace());
            }
            children = registry()
                .changed
                .wait_timeout(children, Duration::from_millis(10))
                .map(|(guard, _)| guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner().0);
        }
        drop(children);
        let _ = signal_hook::low_level::emulate_default_handler(signal);
        // SIGHUP, SIGINT and SIGTERM all end a program by default
        crate::exit::now(crate::exit::signalled(signal));
    }

    /// Kill every child and every group this program made.
    fn kill_all() {
        kill(&lock());
    }

    fn kill(children: &Children) {
        for (pid, own_group) in &children.running {
            if !own_group {
                let _ = rustix::process::kill_process(*pid, Signal::KILL);
            }
        }
        for group in &children.groups {
            let _ = rustix::process::kill_process_group(*group, Signal::KILL);
        }
    }
}

#[cfg(not(any(unix, windows)))]
compile_error!("only Unix and Windows are supported");

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::io::AsRawHandle;
    use std::process::{Child, Command};
    use win32job::{ExtendedLimitInfo, Job};

    /// The jobs a child is put in. Closing a job leaves its processes
    /// running, unless it was told to kill them first.
    pub(super) struct Group {
        /// Holds the child and everything it starts until they have exited:
        /// what [`group_alive`] reads, during a stop too
        pub(super) tracked: Job,
        /// Nested in `tracked`: told to kill and closed by a stop
        kill: Option<Job>,
    }

    fn job_of(child: &Child) -> Result<Job, win32job::JobError> {
        let job = Job::create()?;
        // what must finish may leave it (spawn_finishing)
        job.set_extended_limit_info(ExtendedLimitInfo::new().limit_breakaway_ok())?;
        job.assign_process(child.as_raw_handle() as isize)?;
        Ok(job)
    }

    pub(super) fn spawn(
        command: &mut Command,
        grouped: bool,
    ) -> io::Result<(Child, Option<Group>)> {
        let mut child = command.spawn()?;
        if !grouped {
            return Ok((child, None));
        }
        // the second assignment nests its job in the first
        let jobs = job_of(&child).and_then(|tracked| Ok((tracked, job_of(&child)?)));
        match jobs {
            Ok((tracked, kill)) => Ok((
                child,
                Some(Group {
                    tracked,
                    kill: Some(kill),
                }),
            )),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(io::Error::other(e))
            }
        }
    }

    pub(super) fn group_alive(group: &mut Group, _child: &mut Child) -> bool {
        // what can't be told counts as alive, so it gets stopped
        match group.tracked.query_process_id_list() {
            Ok(ids) => !ids.is_empty(),
            Err(_) => true,
        }
    }

    pub(super) fn stop_group(group: &mut Group, child: &mut Child, _kill: bool) {
        let Some(job) = group.kill.take() else {
            return;
        };
        let mut limits = ExtendedLimitInfo::new();
        limits.limit_kill_on_job_close().limit_breakaway_ok();
        if job.set_extended_limit_info(&limits).is_ok() {
            // closed: everything in it is killed
            drop(job);
        } else {
            // kept for the next stop to try again
            group.kill = Some(job);
            let _ = child.kill();
        }
    }

    /// In a console process group of its own (Ctrl-C misses it), and out
    /// of this program's jobs when every one of them lets it go.
    pub(super) fn spawn_finishing(command: &mut Command) -> io::Result<Child> {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        const ERROR_ACCESS_DENIED: i32 = 5;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        match command.spawn() {
            // a job that doesn't let it go (not one of spawn_group's) keeps it
            Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => {
                command.creation_flags(CREATE_NEW_PROCESS_GROUP).spawn()
            }
            started => started,
        }
    }

    pub(super) fn exited(_child: &Child) {}

    /// No list to take it off: [`Owned::try_wait`] asks std.
    pub(super) fn try_exited(_child: &Child) -> bool {
        true
    }

    pub(super) fn reaped() {}

    pub(super) fn let_go(_child: &Child) {}

    pub(super) fn own() -> io::Result<()> {
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn stopping_a_group_ends_what_its_program_started() {
        let running = |pid: usize| {
            #[expect(clippy::disallowed_methods, reason = "not a program in a repo")]
            let out = Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
        };
        // cmd starts ping in the background and ends, a second after it
        // started: by then it is in its jobs (see the module documentation)
        #[expect(clippy::disallowed_methods, reason = "not a program in a repo")]
        let mut command = Command::new("cmd");
        command.args([
            "/c",
            "ping -n 2 127.0.0.1 >NUL & start /b ping -n 60 127.0.0.1 >NUL",
        ]);
        let mut owned = spawn_group(&mut command).unwrap();
        owned.wait().unwrap();
        let group = owned.group.as_ref().expect("a job");
        let left = group.tracked.query_process_id_list().unwrap();
        assert!(!left.is_empty(), "ping is still in the job");
        assert!(owned.group_alive());
        owned.stop(true);
        let deadline = Instant::now() + Duration::from_secs(10);
        while owned.group_alive() || left.iter().any(|pid| running(*pid)) {
            assert!(Instant::now() < deadline, "{left:?} still run after stop");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
