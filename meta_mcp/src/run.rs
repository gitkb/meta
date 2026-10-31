//! The tool call being run, and the programs it starts: each one can be
//! stopped when the client cancels the call.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a cancelled program has to stop after SIGTERM (git removes its
/// lock files then) before it is killed.
const GRACE: Duration = Duration::from_secs(2);

/// The tool calls waiting or running, by request id (as JSON text).
#[derive(Default)]
pub(crate) struct Calls {
    state: Mutex<State>,
    /// Whether the running call was cancelled: its program is stopped, and
    /// it starts no other.
    cancelled: AtomicBool,
}

#[derive(Default)]
struct State {
    running: Option<String>,
    /// Calls read but not started yet; `true` once cancelled.
    queued: HashMap<String, bool>,
    /// The server is stopping: no call starts.
    stopping: bool,
    /// A message is being answered, from reading it to writing the reply.
    busy: bool,
}

impl Calls {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        // The state stays whole whatever a panicking holder did
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Notes a call read from the client, to be run later.
    pub(crate) fn queue(&self, id: String) {
        let mut state = self.state();
        if state.stopping {
            // said with the state free: stderr may be slow to take it
            drop(state);
            not_started(&id);
        } else {
            state.queued.insert(id, false);
        }
    }

    /// Drops the call `id`, read but refused before it could start.
    pub(crate) fn forget(&self, id: &str) {
        self.state().queued.remove(id);
    }

    /// Starts the call `id`, unless it was cancelled while it waited.
    pub(crate) fn start(&self, id: String) -> bool {
        let mut state = self.state();
        if state.queued.remove(&id) == Some(true) || state.stopping {
            return false;
        }
        state.running = Some(id);
        self.cancelled.store(false, Ordering::SeqCst);
        true
    }

    /// Ends the running call, once `then` has had whether it was cancelled
    /// (and done what that asks) while it still counts as running.
    pub(crate) fn finish<T>(&self, then: impl FnOnce(bool) -> T) -> T {
        let done = then(self.cancelled.swap(false, Ordering::SeqCst));
        self.state().running = None;
        done
    }

    /// Cancels the running call, starts no other, and waits until it has
    /// ended, an atomic batch's rollback included, and its answer is out.
    /// (A signal stops the server only on Unix.)
    #[cfg(unix)]
    pub(crate) fn stop_running(&self) {
        self.cancel_all();
        while {
            let state = self.state();
            state.running.is_some() || state.busy
        } {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Cancels the running call and starts no other; the ones waiting are
    /// dropped, each said on stderr.
    pub(crate) fn cancel_all(&self) {
        let dropped: Vec<String> = {
            let mut state = self.state();
            state.stopping = true;
            if state.running.is_some() {
                self.cancelled.store(true, Ordering::SeqCst);
            }
            let queued = state.queued.drain();
            queued
                .filter(|(_, cancelled)| !cancelled)
                .map(|(id, _)| id)
                .collect()
        };
        // said with the state free: stderr may be slow to take it
        for id in dropped {
            not_started(&id);
        }
    }

    /// Answers a message with `work`, which a stopping server waits for:
    /// a call that ran to its end gets its reply written.
    pub(crate) fn answering<T>(&self, work: impl FnOnce() -> T) -> T {
        self.state().busy = true;
        let done = work();
        self.state().busy = false;
        done
    }

    /// Cancels the call `id`: a waiting one never starts, the running one's
    /// program is stopped. Other ids (done, or never seen) are ignored.
    pub(crate) fn cancel(&self, id: &str) {
        let mut state = self.state();
        if state.running.as_deref() == Some(id) {
            self.cancelled.store(true, Ordering::SeqCst);
        } else if let Some(cancelled) = state.queued.get_mut(id) {
            *cancelled = true;
        }
    }

    /// Whether the running call was cancelled.
    pub(crate) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Runs `command` to its end, as `Command::output`, but with no input,
    /// and stopped (with whatever it started) if the call is cancelled. A
    /// cancelled call starts nothing more.
    pub(crate) fn output(&self, command: &mut Command) -> std::io::Result<Output> {
        Ok(self.output_lines(command, usize::MAX)?.0)
    }

    /// Like [`Calls::output`], but stdout holds at most `max_lines` lines:
    /// then the program's stdout is closed, which stops it. The flag says
    /// whether there was more.
    pub(crate) fn output_lines(
        &self,
        command: &mut Command,
        max_lines: usize,
    ) -> std::io::Result<(Output, bool)> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("the tool call was cancelled"));
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Its own process group, so cancelling stops what it starts too
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(command, 0);
        let mut child = command.spawn()?;
        let (Some(stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
            stop(&mut child, true);
            return Err(std::io::Error::other("no pipe to the program"));
        };
        let stdout = std::thread::spawn(move || read_lines(stdout, max_lines));
        let stderr = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map(|_| bytes)
        });
        // Until it has ended and its pipes are closed: something it started
        // in the background may still hold them. Once cancelled, until its
        // whole process group is gone, even what closed its pipes; then
        // nothing waits for a pipe held from outside the group.
        let mut status = None;
        let mut terminated: Option<Instant> = None;
        // short programs end soon; a long one is looked at less often
        let mut nap = Duration::from_millis(1);
        loop {
            if status.is_none() {
                status = child.try_wait()?;
            }
            let cancelled = self.cancelled.load(Ordering::SeqCst);
            let running = status.is_none()
                || if cancelled {
                    group_alive(&child)
                } else {
                    !stdout.is_finished() || !stderr.is_finished()
                };
            // A cancelled call waits no longer than twice the grace: a dead
            // process no one reaps still counts as in its group
            let given_up = terminated.is_some_and(|at| at.elapsed() > 2 * GRACE);
            if !running || given_up {
                break;
            }
            // SIGTERM once, then SIGKILL until the group is gone
            match terminated {
                _ if !cancelled => {}
                None => {
                    stop(&mut child, false);
                    terminated = Some(Instant::now());
                }
                Some(at) if at.elapsed() > GRACE => stop(&mut child, true),
                Some(_) => {}
            }
            std::thread::sleep(nap);
            nap = (nap * 2).min(Duration::from_millis(if cancelled { 5 } else { 50 }));
        }
        let status = status
            .ok_or_else(|| std::io::Error::other("the program did not stop when cancelled"))?;
        let lost = || std::io::Error::other("reading the program's output failed");
        // A reader still waiting (cancelled) is left behind with what it read
        let (stdout, truncated) = if stdout.is_finished() {
            stdout.join().map_err(|_| lost())??
        } else {
            (Vec::new(), false)
        };
        let stderr = if stderr.is_finished() {
            stderr.join().map_err(|_| lost())??
        } else {
            Vec::new()
        };
        Ok((
            Output {
                status,
                stdout,
                stderr,
            },
            truncated,
        ))
    }
}

/// Says on stderr that the call `id` won't run, as the server is stopping.
fn not_started(id: &str) {
    let _ = writeln!(
        std::io::stderr(),
        "meta-mcp: request {id} not started: the server is stopping"
    );
}

/// Up to `max` lines of `pipe`, and whether there were more.
fn read_lines(pipe: impl Read, max: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut reader = BufReader::new(pipe);
    let mut bytes = Vec::new();
    for _ in 0..max {
        if reader.read_until(b'\n', &mut bytes)? == 0 {
            return Ok((bytes, false));
        }
    }
    Ok((bytes, !reader.fill_buf()?.is_empty()))
}

/// Whether anything is left of the program's process group, after the
/// program itself ended. Elsewhere than Unix there is no group to wait for.
fn group_alive(child: &Child) -> bool {
    // The group's id can't be another's while any of it lives. Accepted:
    // once the group is empty its id could be reused before the next check,
    // which needs pid wraparound within a few milliseconds.
    #[cfg(unix)]
    return rustix::process::test_kill_process_group(rustix::process::Pid::from_child(child))
        .is_ok();
    #[cfg(not(unix))]
    {
        let _ = child;
        false
    }
}

/// Asks the program and everything in its process group to stop, or kills
/// them. Elsewhere than Unix only the program itself is killed.
fn stop(child: &mut Child, kill: bool) {
    #[cfg(unix)]
    {
        use rustix::process::{kill_process_group, Pid, Signal};
        let signal = if kill { Signal::KILL } else { Signal::TERM };
        // Its group lives on while the program or anything it started runs
        let _ = kill_process_group(Pid::from_child(child), signal);
    }
    #[cfg(not(unix))]
    {
        let _ = kill;
        let _ = child.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use crate::test_support::{gone, wait_for};

    /// The name of the program `pid` runs.
    #[cfg(unix)]
    fn program(pid: &str) -> String {
        let out = Command::new("ps")
            .args(["-o", "comm=", "-p", pid])
            .output()
            .unwrap();
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        name.rsplit('/').next().unwrap_or_default().to_string()
    }

    /// Whether `pid` leads a session of its own (it called setsid).
    #[cfg(target_os = "linux")]
    fn leads_a_session(pid: &str) -> bool {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        let fields: Vec<&str> = stat
            .rsplit_once(')')
            .map_or(vec![], |(_, rest)| rest.split_whitespace().collect());
        fields.get(3) == Some(&pid)
    }

    #[test]
    #[cfg(unix)]
    fn output_lines_stops_at_the_cap_and_says_so() {
        let calls = Calls::default();
        let mut seq = Command::new("seq");
        let (out, truncated) = calls.output_lines(seq.args(["1", "100000"]), 3).unwrap();
        assert_eq!(out.stdout, b"1\n2\n3\n");
        assert!(truncated);
        let (out, truncated) = calls.output_lines(Command::new("seq").arg("3"), 3).unwrap();
        assert_eq!(out.stdout, b"1\n2\n3\n");
        assert!(!truncated && out.status.success());
    }

    /// Cancelling the running call stops its program and what that started,
    /// even one that ignores SIGTERM and holds none of its pipes, and the
    /// call starts nothing more.
    #[test]
    #[cfg(unix)]
    fn cancel_stops_the_program_and_its_children() {
        let calls = Calls::default();
        let dir = tempfile::tempdir().unwrap();
        let pids = dir.path().join("pids");
        assert!(calls.start("1".to_string()));
        let started = std::time::Instant::now();
        std::thread::scope(|scope| {
            let run = scope.spawn(|| {
                let script = "(trap '' TERM; exec sleep 60) >/dev/null 2>&1 & \
                              echo $$ $! > pids.tmp && mv pids.tmp pids; wait";
                calls.output(
                    Command::new("sh")
                        .args(["-c", script])
                        .current_dir(dir.path()),
                )
            });
            // the child has set its trap: it is `sleep` now
            let child = || {
                let pids = std::fs::read_to_string(&pids).ok()?;
                Some(program(pids.split_whitespace().nth(1)?) == "sleep")
            };
            wait_for("the child to ignore SIGTERM", || child() == Some(true));
            calls.cancel("1");
            let out = run.join().unwrap().unwrap();
            assert!(!out.status.success());
        });
        assert!(started.elapsed() < GRACE + Duration::from_secs(5));
        for pid in std::fs::read_to_string(&pids).unwrap().split_whitespace() {
            wait_for(&format!("{pid} to end"), || gone(pid));
        }
        let err = calls.output(&mut Command::new("true")).unwrap_err();
        assert!(err.to_string().contains("cancelled"), "{err}");
        assert!(calls.finish(|cancelled| cancelled));
        // the next call runs
        assert!(calls.start("2".to_string()));
        assert!(calls
            .output(&mut Command::new("true"))
            .unwrap()
            .status
            .success());
        assert!(!calls.finish(|cancelled| cancelled));
    }

    /// A process that left the group but holds its pipes doesn't keep a
    /// cancelled call waiting.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_cancelled_call_does_not_wait_for_pipes_held_outside_its_group() {
        let calls = Calls::default();
        let dir = tempfile::tempdir().unwrap();
        let pid = dir.path().join("pid");
        assert!(calls.start("1".to_string()));
        let started = std::time::Instant::now();
        std::thread::scope(|scope| {
            let run = scope.spawn(|| {
                let script = "setsid sleep 60 & echo $! > pid.tmp && mv pid.tmp pid; wait";
                calls.output(
                    Command::new("sh")
                        .args(["-c", script])
                        .current_dir(dir.path()),
                )
            });
            let escaped = || Some(leads_a_session(std::fs::read_to_string(&pid).ok()?.trim()));
            wait_for("the child to leave the group", || escaped() == Some(true));
            calls.cancel("1");
            assert!(run.join().unwrap().is_ok());
        });
        let elapsed = started.elapsed();
        let escaped = std::fs::read_to_string(&pid).unwrap();
        drop(Command::new("kill").arg(escaped.trim()).status());
        assert!(elapsed < GRACE + Duration::from_secs(5), "{elapsed:?}");
    }

    /// A dead member of the group that no one reaps doesn't keep a
    /// cancelled call waiting: its parent left the group and never waits.
    #[test]
    #[cfg(target_os = "linux")]
    fn a_cancelled_call_does_not_wait_for_a_zombie_in_its_group() {
        let calls = Calls::default();
        let dir = tempfile::tempdir().unwrap();
        let pid = dir.path().join("pid");
        assert!(calls.start("1".to_string()));
        let mut cancelled = None;
        std::thread::scope(|scope| {
            let run = scope.spawn(|| {
                let script = "(sleep 60 & exec setsid sleep 61) >/dev/null 2>&1 & \
                              echo $! > pid.tmp && mv pid.tmp pid; wait";
                calls.output(
                    Command::new("sh")
                        .args(["-c", script])
                        .current_dir(dir.path()),
                )
            });
            // the parent has left the group (it leads a session of its own),
            // so the child it started before is the one no one will reap
            let escaped = || Some(leads_a_session(std::fs::read_to_string(&pid).ok()?.trim()));
            wait_for("the parent to leave the group", || escaped() == Some(true));
            cancelled = Some(std::time::Instant::now());
            calls.cancel("1");
            drop(run.join().unwrap());
        });
        let elapsed = cancelled.unwrap().elapsed();
        let parent = std::fs::read_to_string(&pid).unwrap();
        drop(Command::new("kill").arg(parent.trim()).status());
        // it waited for the zombie, then gave up
        assert!(elapsed >= 2 * GRACE, "{elapsed:?}");
        assert!(elapsed < 2 * GRACE + Duration::from_secs(5), "{elapsed:?}");
    }

    /// A stopping server waits for the reply being written, and starts no
    /// call waiting or read later.
    #[test]
    #[cfg(unix)]
    fn stopping_waits_for_the_answer_and_starts_nothing_more() {
        let calls = Calls::default();
        calls.queue("1".to_string());
        let (started, written) = (AtomicBool::new(false), AtomicBool::new(false));
        std::thread::scope(|scope| {
            scope.spawn(|| {
                calls.answering(|| {
                    started.store(true, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(300));
                    written.store(true, Ordering::SeqCst);
                })
            });
            wait_for("an answer to be under way", || {
                started.load(Ordering::SeqCst)
            });
            calls.stop_running();
            assert!(written.load(Ordering::SeqCst));
        });
        assert!(!calls.start("1".to_string()));
        calls.queue("2".to_string());
        assert!(calls.state().queued.is_empty());
        assert!(!calls.start("2".to_string()));
    }

    #[test]
    fn a_server_that_cancels_all_drops_its_waiting_calls() {
        let calls = Calls::default();
        calls.queue("1".to_string());
        calls.cancel_all();
        assert!(calls.state().queued.is_empty());
        assert!(!calls.start("1".to_string()));
    }

    #[test]
    fn a_cancel_for_a_waiting_call_skips_it_and_others_are_ignored() {
        let calls = Calls::default();
        calls.queue("1".to_string());
        calls.queue("2".to_string());
        calls.cancel("1");
        calls.cancel("99");
        assert!(!calls.start("1".to_string()));
        assert!(calls.start("2".to_string()));
        assert!(!calls.finish(|cancelled| cancelled));
        // done: a late cancel changes nothing, and a refused call is dropped
        calls.cancel("2");
        calls.queue("4".to_string());
        calls.forget("4");
        assert!(calls.state().queued.is_empty());
        assert!(calls.start("3".to_string()));
        assert!(!calls.finish(|cancelled| cancelled));
    }
}
