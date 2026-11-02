//! What meta's programs print, and in which colours.
//!
//! Output goes through here, never `print!`: a write to stdout that fails
//! is an error ([`StdoutError`]) the caller returns, so a closed pipe
//! (`meta … | head`) ends the program quietly instead of a panic. Writes
//! to stderr ([`errln!`](crate::errln!)) ignore a failure: there is nowhere left to say it.

use crate::display::escape_lines;
use colored::Colorize;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

/// A write to stdout failed.
#[derive(Debug)]
pub struct StdoutError(pub io::Error);

impl fmt::Display for StdoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Failed to write output: {}", self.0)
    }
}

impl std::error::Error for StdoutError {}

/// `print!` that returns a failed write as a [`StdoutError`].
#[macro_export]
macro_rules! out {
    ($($arg:tt)*) => { $crate::out::stdout(format_args!($($arg)*)) };
}

/// `println!` that returns a failed write as a [`StdoutError`].
#[macro_export]
macro_rules! outln {
    () => { $crate::out::stdout(format_args!("\n")) };
    ($($arg:tt)*) => { $crate::out::stdout(format_args!("{}\n", format_args!($($arg)*))) };
}

/// `eprintln!` that ignores a failed write.
#[macro_export]
macro_rules! errln {
    () => { $crate::out::stderr(format_args!("\n")) };
    ($($arg:tt)*) => { $crate::out::stderr(format_args!("{}\n", format_args!($($arg)*))) };
}

/// Whether anything was written to stdout yet: with `--json`, an error
/// after a report goes to stderr, so stdout holds one JSON document.
static WROTE: AtomicBool = AtomicBool::new(false);

/// Write `args` to stdout (see [`out!`]).
pub fn stdout(args: fmt::Arguments<'_>) -> Result<(), StdoutError> {
    if args.as_str() == Some("") {
        return Ok(());
    }
    WROTE.store(true, Ordering::Relaxed);
    io::stdout().lock().write_fmt(args).map_err(StdoutError)
}

/// Write `bytes` to stdout as they are: a command's output, which need not
/// be UTF-8.
pub fn stdout_bytes(bytes: &[u8]) -> Result<(), StdoutError> {
    if bytes.is_empty() {
        return Ok(());
    }
    WROTE.store(true, Ordering::Relaxed);
    io::stdout().lock().write_all(bytes).map_err(StdoutError)
}

/// Whether this program wrote anything to stdout.
pub fn wrote_stdout() -> bool {
    WROTE.load(Ordering::Relaxed)
}

/// Flush stdout, before a child writes to it or a prompt waits.
pub fn flush() -> Result<(), StdoutError> {
    io::stdout().flush().map_err(StdoutError)
}

/// Write `args` to stderr (see [`errln!`](crate::errln!)).
pub fn stderr(args: fmt::Arguments<'_>) {
    let _ = io::stderr().lock().write_fmt(args);
}

/// Write `bytes` to stderr as they are.
pub fn stderr_bytes(bytes: &[u8]) {
    let _ = io::stderr().lock().write_all(bytes);
}

/// Tell the user about a problem the command goes on despite. The message
/// can carry names and paths from git and the filesystem: control
/// characters in it are escaped.
pub fn warn(message: impl fmt::Display) {
    errln!(
        "{} {}",
        "warning:".yellow().bold(),
        escape_lines(&message.to_string())
    );
}

/// [`warn`], or with `--strict` (warnings are errors) the error
/// `<message> (strict mode)`.
pub fn warn_or_bail(strict: bool, message: impl fmt::Display) -> anyhow::Result<()> {
    anyhow::ensure!(!strict, "{message} (strict mode)");
    warn(message);
    Ok(())
}

/// Whether output is coloured, decided once for the whole program and
/// applied to `colored`: never with `--json` (see [`set_json`]), never with
/// `NO_COLOR`, always with `CLICOLOR_FORCE` (not `0`), never with
/// `CLICOLOR=0` or on a `TERM=dumb` terminal, and otherwise only when
/// stdout is a terminal.
pub fn color() -> bool {
    static FROM_ENV: OnceLock<bool> = OnceLock::new();
    let on = !json()
        && *FROM_ENV.get_or_init(|| {
            let var = |name| std::env::var_os(name).filter(|value| !value.is_empty());
            decide_color(
                var("NO_COLOR"),
                var("CLICOLOR_FORCE"),
                var("CLICOLOR"),
                var("TERM"),
                io::stdout().is_terminal(),
            )
        });
    colored::control::set_override(on);
    on
}

fn decide_color(
    no_color: Option<OsString>,
    force: Option<OsString>,
    clicolor: Option<OsString>,
    term: Option<OsString>,
    terminal: bool,
) -> bool {
    if no_color.is_some() {
        return false;
    }
    if force.is_some_and(|force| force != "0") {
        return true;
    }
    terminal && clicolor.is_none_or(|c| c != "0") && term.is_none_or(|term| term != "dumb")
}

/// Have `command`, whose output meta reads through a pipe (to show, relay
/// or put in JSON), colour as meta does ([`color`]): with colour, the
/// conventional variables that force it (`CLICOLOR_FORCE`, Node's
/// `FORCE_COLOR`, unless the user set their own) are set; without it a
/// user's `CLICOLOR_FORCE` is removed, since it would win over the pipe,
/// and under `--json` their `FORCE_COLOR` too. They reach what that program
/// starts as well (a plugin's hooks and git); one whose output meta parses
/// removes them.
pub fn pass_color(command: &mut std::process::Command) {
    if color() {
        command.env("CLICOLOR_FORCE", "1");
        if std::env::var_os("FORCE_COLOR").is_none() {
            command.env("FORCE_COLOR", "1");
        }
        // a terminal's programs expect a TERM; a pipe's get none made up
        if std::env::var_os("TERM").is_none() && io::stdout().is_terminal() {
            command.env("TERM", "xterm-256color");
        }
    } else {
        command.env_remove("CLICOLOR_FORCE");
        if json() {
            command.env_remove("FORCE_COLOR");
        }
    }
}

static JSON: AtomicBool = AtomicBool::new(false);

/// Output is JSON (`--json`), for programs that read it: no colour, and
/// errors are JSON on stdout. Returns `json`.
pub fn set_json(json: bool) -> bool {
    JSON.store(json, Ordering::Relaxed);
    color();
    json
}

/// Whether output is JSON (see [`set_json`]).
pub fn json() -> bool {
    JSON.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warn_or_bail_is_an_error_naming_strict_mode_only_with_strict() {
        assert!(warn_or_bail(false, "a problem").is_ok());
        let error = warn_or_bail(true, "a problem").unwrap_err().to_string();
        assert_eq!(error, "a problem (strict mode)");
    }

    #[test]
    fn color_is_off_for_no_color_and_dumb_terminals_and_forced_on_by_clicolor_force() {
        let some = |s: &str| Some(OsString::from(s));
        assert!(decide_color(None, None, None, some("xterm"), true));
        assert!(!decide_color(None, None, None, some("xterm"), false));
        assert!(!decide_color(some("1"), None, None, None, true));
        assert!(!decide_color(some("1"), some("1"), None, None, true));
        assert!(!decide_color(None, None, None, some("dumb"), true));
        assert!(!decide_color(None, None, some("0"), None, true));
        assert!(decide_color(
            None,
            some("1"),
            some("0"),
            some("dumb"),
            false
        ));
        assert!(!decide_color(None, some("0"), None, None, false));
    }
}
