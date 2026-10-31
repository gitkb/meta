//! The tool call being run, and the programs it starts: each one can be
//! stopped when the client cancels the call.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
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
        self.state().queued.insert(id, false);
    }

    /// Starts the call `id`, unless it was cancelled while it waited.
    pub(crate) fn start(&self, id: String) -> bool {
        let mut state = self.state();
        if state.queued.remove(&id) == Some(true) {
            return false;
        }
        state.running = Some(id);
        self.cancelled.store(false, Ordering::SeqCst);
        true
    }

    /// Ends the running call; whether it ran to its end, uncancelled.
    pub(crate) fn finish(&self) -> bool {
        self.state().running = None;
        !self.cancelled.swap(false, Ordering::SeqCst)
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
        // in the background may still hold them
        let mut status = None;
        let mut terminated: Option<Instant> = None;
        while status.is_none() || !stdout.is_finished() || !stderr.is_finished() {
            if status.is_none() {
                status = child.try_wait()?;
            }
            if self.cancelled.load(Ordering::SeqCst) {
                let kill = terminated.is_some_and(|at| at.elapsed() > GRACE);
                stop(&mut child, kill);
                terminated.get_or_insert_with(Instant::now);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let status = status.ok_or_else(|| std::io::Error::other("the program's status is lost"))?;
        let lost = || std::io::Error::other("reading the program's output failed");
        let (stdout, truncated) = stdout.join().map_err(|_| lost())??;
        let stderr = stderr.join().map_err(|_| lost())??;
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
    /// even one that ignores SIGTERM, and the call starts nothing more.
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
                let script = "trap '' TERM; sleep 60 & echo $$ $! > pids; wait";
                calls.output(
                    Command::new("sh")
                        .args(["-c", script])
                        .current_dir(dir.path()),
                )
            });
            while std::fs::read_to_string(&pids).map_or(true, |p| !p.ends_with('\n')) {
                std::thread::sleep(Duration::from_millis(10));
            }
            calls.cancel("1");
            let out = run.join().unwrap().unwrap();
            assert!(!out.status.success());
        });
        assert!(started.elapsed() < GRACE + Duration::from_secs(5));
        for pid in std::fs::read_to_string(&pids).unwrap().split_whitespace() {
            let alive = Command::new("kill").args(["-0", pid]).output().unwrap();
            assert!(!alive.status.success(), "{pid} still runs");
        }
        let err = calls.output(&mut Command::new("true")).unwrap_err();
        assert!(err.to_string().contains("cancelled"), "{err}");
        assert!(!calls.finish());
        // the next call runs
        assert!(calls.start("2".to_string()));
        assert!(calls
            .output(&mut Command::new("true"))
            .unwrap()
            .status
            .success());
        assert!(calls.finish());
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
        assert!(calls.finish());
        // done: a late cancel changes nothing
        calls.cancel("2");
        assert!(calls.start("3".to_string()));
        assert!(calls.finish());
    }
}
