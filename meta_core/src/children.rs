//! The processes a meta program starts, and what happens to them when it is
//! told to stop.
//!
//! On Unix, [`own`] makes the calling program the one owner of SIGINT,
//! SIGTERM and SIGHUP (those not already ignored when it started, as under
//! `nohup`; where that can't be told without unsafe code, everywhere but
//! Linux, SIGHUP is left as it was). Children started through [`spawn`] are registered. When one of
//! those signals arrives, no new child starts, the signal is passed on to the
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

use std::io;
use std::process::{Child, Command, ExitStatus, Output};

/// A child started through [`spawn`]. Wait for it through [`Owned::wait`]
/// or [`Owned::exchange`]. One dropped unwaited for is let go: a
/// stop no longer waits for it (its process group still gets the signal).
pub struct Owned {
    child: Child,
    /// Off the list: waited for (its pid may now be another child's)
    waited: bool,
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
    imp::spawn(command).map(|child| Owned {
        child,
        waited: false,
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

    fn group_alive(group: Pid) -> bool {
        // A group's id can't be another's while any of it lives. Accepted, as
        // in meta-mcp: once it is empty its id could be reused before the
        // next check (groups are checked whenever a child is started or
        // reaped), which needs pid wraparound in between.
        rustix::process::test_kill_process_group(group).is_ok()
    }

    pub(super) fn spawn(command: &mut Command) -> io::Result<Child> {
        let own_group = !shared();
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
            children.groups.retain(|group| group_alive(*group));
            children.groups.push(pid);
        }
        // held from the check above to here: a signal that stops this
        // program either came first or sees the child
        drop(children);
        Ok(child)
    }

    /// Forget the process groups that have emptied: once one has, its id
    /// may become another's.
    pub(super) fn reaped() {
        lock().groups.retain(|group| group_alive(*group));
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
                    crate::exit::now(128 + origin.signal);
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
        children.groups.retain(|group| group_alive(*group));
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
            children.groups.retain(|group| group_alive(*group));
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
        crate::exit::now(128 + signal);
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

#[cfg(not(unix))]
mod imp {
    use std::io;
    use std::process::{Child, Command};

    pub(super) fn spawn(command: &mut Command) -> io::Result<Child> {
        command.spawn()
    }

    pub(super) fn exited(_child: &Child) {}

    pub(super) fn reaped() {}

    pub(super) fn let_go(_child: &Child) {}

    pub(super) fn own() -> io::Result<()> {
        Ok(())
    }
}
