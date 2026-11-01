//! The processes a meta program starts, and what happens to them when it is
//! told to stop.
//!
//! On Unix, [`own`] makes the calling program the one owner of SIGINT,
//! SIGTERM and SIGHUP (those not already ignored when it started, as under
//! `nohup`). Children started through [`spawn`] are registered. When one of
//! those signals arrives, no new child starts, the signal is passed on to the
//! children, and once they are gone the program ends the way the signal ends
//! it, so a shell running it sees it was interrupted. Threads waiting on a
//! child then never resume: nothing is printed or done after the signal. A
//! second signal kills every child at once and ends the program.
//!
//! Where the program is the foreground job of a terminal, its children stay
//! in its process group, as before: they can prompt on the terminal, and the
//! terminal's Ctrl-C and hangup reach them directly; a signal another process
//! sends to the program alone is passed on to each of them. Anywhere else
//! (no terminal, a background job, a supervisor), each child gets a process
//! group of its own, so the signal reaches everything it starts in turn; as
//! there's no one to press Ctrl-C twice, what is still running two seconds
//! later is killed.
//!
//! Elsewhere (Windows), children start as they always did; Ctrl-C reaches
//! every process attached to the console.

use std::io;
use std::process::{Child, Command, ExitStatus, Output};

/// A child started through [`spawn`]. Wait for it through [`Owned::wait`]
/// or [`Owned::wait_with_output`]. One dropped unwaited for is let go: a
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

impl std::ops::Deref for Owned {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.child
    }
}

impl std::ops::DerefMut for Owned {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
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
/// does (stdin from nothing unless set).
pub fn output(command: &mut Command) -> io::Result<Output> {
    use std::process::Stdio;
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    spawn(command)?.wait_with_output()
}

impl Owned {
    /// Wait for the child to exit.
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        if !self.waited {
            imp::exited(&self.child);
            self.waited = true;
        }
        self.child.wait()
    }

    /// Read the child's piped stdout and stderr to their end, then wait for
    /// it, as [`Child::wait_with_output`] does.
    pub fn wait_with_output(mut self) -> io::Result<Output> {
        use std::io::Read;
        drop(self.child.stdin.take());
        let stderr = self.child.stderr.take().map(|mut pipe| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                pipe.read_to_end(&mut bytes).map(|_| bytes)
            })
        });
        let mut stdout = Vec::new();
        if let Some(mut pipe) = self.child.stdout.take() {
            pipe.read_to_end(&mut stdout)?;
        }
        let stderr = match stderr {
            Some(reader) => reader
                .join()
                .map_err(|_| io::Error::other("the stderr reader panicked"))??,
            None => Vec::new(),
        };
        let status = self.wait()?;
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

    /// How long children with no terminal get to stop before they're killed.
    const GRACE: Duration = Duration::from_secs(2);

    #[derive(Default)]
    struct Children {
        /// Each running child, and whether it leads a process group of its own.
        running: Vec<(Pid, bool)>,
        /// The process groups this program made that may still have members.
        groups: Vec<Pid>,
        /// The signal that stops this program, once one came.
        stopping: Option<i32>,
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
        // next check, which needs pid wraparound within milliseconds.
        rustix::process::test_kill_process_group(group).is_ok()
    }

    pub(super) fn spawn(command: &mut Command) -> io::Result<Child> {
        let own_group = !shared();
        let mut children = lock();
        if children.stopping.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "stopping: nothing new starts",
            ));
        }
        if own_group {
            command.process_group(0);
        }
        let child = command.spawn()?;
        let pid = Pid::from_child(&child);
        children.running.push((pid, own_group));
        if own_group {
            children.groups.retain(|group| group_alive(*group));
            children.groups.push(pid);
        }
        Ok(child)
    }

    /// Take `child` off the list of running children, if it is still there.
    pub(super) fn let_go(child: &Child) {
        let pid = Pid::from_child(child);
        lock().running.retain(|(running, _)| *running != pid);
        registry().changed.notify_all();
    }

    /// Wait until `child` has exited, without reaping it, then take it off
    /// the list: the owner never signals a reaped pid, which may be reused.
    /// When the program is stopping, this never returns.
    pub(super) fn exited(child: &Child) {
        use rustix::process::{waitid, WaitId, WaitIdOptions};
        let pid = Pid::from_child(child);
        // Interrupted: try again. Any other error (already reaped) is for
        // std's wait to report.
        while let Err(rustix::io::Errno::INTR) = waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
        ) {}
        let mut children = lock();
        children.running.retain(|(running, _)| *running != pid);
        registry().changed.notify_all();
        if children.stopping.is_some() {
            drop(children);
            loop {
                std::thread::park();
            }
        }
    }

    /// The signals among `signals` that this program started out ignoring.
    /// Only Linux says so without unsafe code (`/proc`); elsewhere none.
    fn ignored(signals: &[i32]) -> Vec<i32> {
        let mask = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                status
                    .lines()
                    .find_map(|line| line.strip_prefix("SigIgn:"))
                    .and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok())
            })
            .unwrap_or(0);
        signals
            .iter()
            .copied()
            .filter(|signal| {
                u32::try_from(signal - 1)
                    .ok()
                    .and_then(|bit| mask.checked_shr(bit))
                    .is_some_and(|m| m & 1 == 1)
            })
            .collect()
    }

    pub(super) fn own() -> io::Result<()> {
        use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
        use signal_hook::iterator::exfiltrator::WithOrigin;
        let all = [SIGINT, SIGTERM, SIGHUP];
        let ignored = ignored(&all);
        let listened: Vec<i32> = all.into_iter().filter(|s| !ignored.contains(s)).collect();
        // decided once: a job moved between foreground and background
        // keeps its children's groups
        shared();
        let mut signals = signal_hook::iterator::SignalsInfo::<WithOrigin>::new(listened)?;
        std::thread::spawn(move || {
            let mut stopping = false;
            for origin in signals.forever() {
                if stopping {
                    kill_all();
                    std::process::exit(128 + origin.signal);
                }
                stopping = true;
                std::thread::spawn(move || stop(origin.signal, origin.process.is_some()));
            }
        });
        Ok(())
    }

    /// Pass `signal` on, wait for every child, then end as `signal` ends a
    /// program. `sent` says another process sent it, rather than the
    /// terminal, which sends it to the children in this program's group too.
    fn stop(signal: i32, sent: bool) {
        let mut children = lock();
        children.stopping = Some(signal);
        // the three signals `own` listens for all have names
        let sig = Signal::from_named_raw(signal).unwrap_or(Signal::TERM);
        for (pid, own_group) in &children.running {
            if !own_group && sent {
                let _ = rustix::process::kill_process(*pid, sig);
            }
        }
        for group in &children.groups {
            let _ = rustix::process::kill_process_group(*group, sig);
        }
        let deadline = Instant::now() + GRACE;
        let mut killed = false;
        loop {
            children.groups.retain(|group| group_alive(*group));
            let groups_left = !children.groups.is_empty();
            if children.running.is_empty() && !groups_left {
                break;
            }
            let now = Instant::now();
            if groups_left && now >= deadline {
                if killed {
                    // twice the grace: a dead process no one reaps still
                    // counts as in its group
                    if now >= deadline + GRACE && children.running.is_empty() {
                        break;
                    }
                } else {
                    for group in &children.groups {
                        let _ = rustix::process::kill_process_group(*group, Signal::KILL);
                    }
                    killed = true;
                }
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
        std::process::exit(128 + signal);
    }

    /// Kill every child and every group this program made.
    fn kill_all() {
        let children = lock();
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

    pub(super) fn let_go(_child: &Child) {}

    pub(super) fn own() -> io::Result<()> {
        Ok(())
    }
}
