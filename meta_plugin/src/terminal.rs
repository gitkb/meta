//! A live command's own terminal (Unix). A command meta runs live while the
//! user is at a terminal runs on a pseudo-terminal instead, so meta sees
//! what it writes (for the summary) while the command still has a terminal
//! of its own, as under script(1):
//!
//! - [`terminal_like`] makes the pseudo-terminal, with the settings and size
//!   of the user's;
//! - the command starts through [`shim`] (meta's own program, run again),
//!   which makes it the leader of a session whose controlling terminal is
//!   the pseudo-terminal: `/dev/tty`, a pager's `ttyname(2)`, a password
//!   prompt all reach it;
//! - [`Relay`] puts the user's terminal in raw mode for as long as the
//!   command runs, sends what they type to the command, and the window's
//!   size when it changes. Their terminal's settings come back however meta
//!   ends ([`restore`]: when the command does, on a signal, an exit, a
//!   panic, and while meta is stopped by `^Z`).
//!
//! With line editing on the pseudo-terminal (the command reads lines, as a
//! shell's commands do), `^C` interrupts the command and meta (as the user's
//! terminal would do to both), and `^Z` stops both until `fg`. A command that
//! turned them off (an editor, `less`) gets the keys themselves.

use std::ffi::OsString;

use std::sync::atomic::{AtomicBool, Ordering};

/// The first argument meta gives its own program to start a command on a
/// pseudo-terminal (see [`shim`]).
const SHIM: &str = "--meta-run-on-terminal";

/// The program calls [`shim`]: it can start a command on a pseudo-terminal.
static SHIMMED: AtomicBool = AtomicBool::new(false);

/// Call first thing in `main` of a program that runs commands live (meta,
/// loop). Run as the shim (`PROGRAM --meta-run-on-terminal COMMAND…`, with
/// a pseudo-terminal on stdin), it makes itself the leader of a session
/// with that terminal as its own, then becomes COMMAND; it never returns
/// then. Otherwise it notes that the program can do that, and returns.
pub fn shim() {
    let args: Vec<OsString> = std::env::args_os().collect();
    match args.get(1) {
        Some(first) if first == SHIM => {}
        _ => {
            SHIMMED.store(true, Ordering::Relaxed);
            return;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let _ = rustix::process::setsid();
        let _ = rustix::process::ioctl_tiocsctty(std::io::stdin());
        let Some((program, rest)) = args.get(2..).and_then(|args| args.split_first()) else {
            crate::exit::now(127);
        };
        #[expect(
            clippy::disallowed_methods,
            reason = "the command meta was asked to run, already set up in its folder"
        )]
        let error = std::process::Command::new(program).args(rest).exec();
        crate::errln!(
            "Failed to run {}: {error}",
            crate::display::escape(&program.to_string_lossy())
        );
    }
    crate::exit::now(127)
}

/// The program and arguments that start `argv` on the pseudo-terminal on
/// its stdin (see [`shim`]); None when this program can't (it didn't call
/// [`shim`], or knows no path to itself that is text).
pub fn shimmed(argv: &[String]) -> Option<Vec<String>> {
    if !cfg!(unix) || !SHIMMED.load(Ordering::Relaxed) {
        return None;
    }
    let me = std::env::current_exe()
        .ok()?
        .into_os_string()
        .into_string()
        .ok()?;
    Some(
        [me, SHIM.to_string()]
            .into_iter()
            .chain(argv.iter().cloned())
            .collect(),
    )
}

/// Put the user's terminal (meta's stdin) back as it was before [`Relay`]
/// made it raw, if it did. Safe to call from anywhere, any number of times.
pub fn restore() {
    #[cfg(unix)]
    imp::restore();
}

#[cfg(unix)]
pub use imp::{terminal_like, Relay};

#[cfg(unix)]
mod imp {
    use rustix::termios::{self, LocalModes, OptionalActions, SpecialCodeIndex, Termios};
    use std::io::{self, Read, Write};
    use std::os::fd::AsFd;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, Once};
    use std::thread::JoinHandle;
    use std::time::Duration;

    /// The user's terminal's settings, while [`Relay`] has it raw
    static SAVED: Mutex<Option<Termios>> = Mutex::new(None);

    fn saved() -> MutexGuard<'static, Option<Termios>> {
        SAVED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn restore() {
        if let Some(settings) = saved().take() {
            let _ = termios::tcsetattr(io::stdin(), OptionalActions::Now, &settings);
        }
    }

    /// The user's terminal raw, its settings kept for [`restore`].
    fn raw(settings: &Termios) -> io::Result<()> {
        let mut raw = settings.clone();
        raw.make_raw();
        termios::tcsetattr(io::stdin(), OptionalActions::Now, &raw)?;
        *saved() = Some(settings.clone());
        Ok(())
    }

    /// A pseudo-terminal like the terminal `like` (its settings and size),
    /// for a command to run on as it would on that terminal: meta's end
    /// (closed in what meta starts), then the command's. `as_is`: what the
    /// command writes reaches meta as it wrote it, for a terminal that does
    /// its own output processing (one meta doesn't make raw); else the
    /// pseudo-terminal does it, as the user's would, for a raw one.
    pub fn terminal_like(
        like: impl AsFd,
        as_is: bool,
    ) -> io::Result<(std::fs::File, std::fs::File)> {
        use rustix::fs::{Mode, OFlags};
        use rustix::pty::OpenptFlags;
        let ours = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
        rustix::io::fcntl_setfd(&ours, rustix::io::FdFlags::CLOEXEC)?;
        rustix::pty::grantpt(&ours)?;
        rustix::pty::unlockpt(&ours)?;
        let name = rustix::pty::ptsname(&ours, Vec::new())?;
        let theirs = rustix::fs::open(
            name.as_c_str(),
            OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        if let Ok(mut settings) = termios::tcgetattr(&like) {
            if as_is {
                settings.output_modes.remove(termios::OutputModes::OPOST);
            }
            let _ = termios::tcsetattr(&theirs, OptionalActions::Now, &settings);
        }
        if let Ok(size) = termios::tcgetwinsize(&like) {
            let _ = termios::tcsetwinsize(&theirs, size);
        }
        Ok((std::fs::File::from(ours), std::fs::File::from(theirs)))
    }

    /// While a command runs on a pseudo-terminal: the window's size follows
    /// the user's (`SIGWINCH`), and with `input`, the user's terminal is
    /// raw and what they type goes to the command. Dropped, it stops, and
    /// their terminal is as it was.
    pub struct Relay {
        stop: Arc<AtomicBool>,
        threads: Vec<JoinHandle<()>>,
        resized: signal_hook::iterator::Handle,
    }

    impl Relay {
        /// Relay for `command` (its pid), running on the pseudo-terminal
        /// whose end meta has is `ours`.
        pub fn start(ours: &std::fs::File, command: u32, input: bool) -> io::Result<Relay> {
            static ON_PANIC: Once = Once::new();
            ON_PANIC.call_once(|| {
                let before = std::panic::take_hook();
                std::panic::set_hook(Box::new(move |info| {
                    restore();
                    before(info);
                }));
            });
            let stop = Arc::new(AtomicBool::new(false));
            let mut threads = Vec::new();
            let mut signals = signal_hook::iterator::Signals::new([signal_hook::consts::SIGWINCH])?;
            let resized = signals.handle();
            let size_to = ours.try_clone()?;
            threads.push(std::thread::Builder::new().spawn(move || {
                for _ in signals.forever() {
                    if let Ok(size) = termios::tcgetwinsize(io::stdin()) {
                        let _ = termios::tcsetwinsize(&size_to, size);
                    }
                }
            })?);
            if input {
                let settings = termios::tcgetattr(io::stdin())?;
                raw(&settings)?;
                let to = ours.try_clone()?;
                let stop = stop.clone();
                let typing = std::thread::Builder::new()
                    .spawn(move || type_into(to, command, &settings, &stop));
                match typing {
                    Ok(thread) => threads.push(thread),
                    Err(e) => {
                        restore();
                        return Err(e);
                    }
                }
            }
            Ok(Relay {
                stop,
                threads,
                resized,
            })
        }
    }

    impl Drop for Relay {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            self.resized.close();
            for thread in std::mem::take(&mut self.threads) {
                let _ = thread.join();
            }
            restore();
        }
    }

    /// Send what the user types (meta's stdin) to the command's terminal
    /// `to`, until `stop`. With line editing on it (`ISIG`), its interrupt
    /// key also interrupts meta, and its suspend key stops the command and
    /// meta (`settings`: the user's terminal's, put back meanwhile).
    fn type_into(mut to: std::fs::File, command: u32, settings: &Termios, stop: &AtomicBool) {
        use rustix::event::{poll, PollFd, PollFlags, Timespec};
        let stdin = io::stdin();
        let mut chunk = [0u8; 4096];
        let wait = Timespec {
            tv_sec: 0,
            tv_nsec: 100_000_000,
        };
        while !stop.load(Ordering::Relaxed) {
            let mut fds = [PollFd::new(&stdin, PollFlags::IN)];
            match poll(&mut fds, Some(&wait)) {
                Ok(0) | Err(rustix::io::Errno::INTR) => continue,
                Ok(_) => {}
                Err(_) => return,
            }
            let n = match stdin.lock().read(&mut chunk) {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            let typed = chunk.get(..n).unwrap_or_default();
            // the keys as the command's terminal has them now
            let keys = termios::tcgetattr(&to)
                .ok()
                .filter(|theirs| theirs.local_modes.contains(LocalModes::ISIG));
            let Some(keys) = keys else {
                if to.write_all(typed).is_err() {
                    return;
                }
                continue;
            };
            let interrupt = keys.special_codes[SpecialCodeIndex::VINTR];
            let suspend = keys.special_codes[SpecialCodeIndex::VSUSP];
            for piece in typed.split_inclusive(|&b| b == interrupt || b == suspend) {
                match piece.split_last() {
                    Some((&last, before)) if last == suspend => {
                        if to.write_all(before).is_err() {
                            return;
                        }
                        stop_both(command, settings, &to);
                    }
                    Some((&last, _)) if last == interrupt => {
                        // meta (and what shares its terminal) is interrupted
                        // as the user's terminal would, first, so the run
                        // stops with the command; then the command's terminal
                        // interrupts it
                        let _ = rustix::process::kill_current_process_group(
                            rustix::process::Signal::INT,
                        );
                        for _ in 0..50 {
                            if crate::process::stopping() {
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        if to.write_all(piece).is_err() {
                            return;
                        }
                    }
                    _ => {
                        if to.write_all(piece).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    }

    /// `^Z`: stop the command, put the user's terminal back, and stop meta
    /// with what shares its terminal, as the user's terminal would; when
    /// continued (`fg`), take the terminal again and continue the command.
    fn stop_both(command: u32, settings: &Termios, to: &std::fs::File) {
        use rustix::process::{kill_current_process_group, kill_process_group, Pid, Signal};
        let group = i32::try_from(command).ok().and_then(Pid::from_raw);
        if let Some(group) = group {
            let _ = kill_process_group(group, Signal::STOP);
        }
        restore();
        // delivered to this thread before it returns: meta stops here
        let _ = kill_current_process_group(Signal::TSTP);
        std::thread::sleep(Duration::from_millis(10));
        let _ = raw(settings);
        if let Ok(size) = termios::tcgetwinsize(io::stdin()) {
            let _ = termios::tcsetwinsize(to, size);
        }
        if let Some(group) = group {
            let _ = kill_process_group(group, Signal::CONT);
        }
    }
}
