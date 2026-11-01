//! How meta's programs end, decided in one place.
//!
//! - 0: done
//! - 1: failed (the error is printed: `Error: …` on stderr, or with
//!   `--json`, before any other output, a JSON object on stdout)
//! - 2: the command line was wrong ([`Usage`], clap's usage errors)
//! - 128 + n: a plugin was killed by signal n, or this program was
//!   (see [`crate::children`])
//! - 141: stdout was closed (`meta … | head`), as a shell reports a
//!   program SIGPIPE stopped
//! - a plugin's own exit code passes through ([`ExitWith`])

use crate::display::escape_lines;
use crate::out::{self, StdoutError};
use colored::Colorize;
use std::process::{ExitCode, ExitStatus};

/// Exit status for output cut off by a closed stdout (128 + SIGPIPE): a
/// failure it hid isn't success.
pub const CLOSED_STDOUT: u8 = 141;

/// Exit status for a command line that can't be run, as clap's.
pub const USAGE: u8 = 2;

/// An ending that was already reported (by a child, by the summary of the
/// commands that failed, by clap's help or usage error): the program ends
/// with this code and prints nothing more.
#[derive(Debug)]
pub struct ExitWith(pub i32);

impl ExitWith {
    /// How `status` ends this program: its code, or 128 + the signal
    /// that killed it.
    pub fn of(status: ExitStatus) -> Self {
        #[cfg(unix)]
        if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
            return ExitWith(128 + signal);
        }
        ExitWith(status.code().unwrap_or(1))
    }
}

impl std::fmt::Display for ExitWith {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "exit code {}", self.0)
    }
}

impl std::error::Error for ExitWith {}

/// A command line that can't be run (an unknown command, say): reported
/// like any error, ending with exit code 2.
#[derive(Debug)]
pub struct Usage(pub String);

impl std::fmt::Display for Usage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Usage {}

/// Run `main` with colour decided ([`out::color`]) and report how it went.
pub fn run(main: impl FnOnce() -> anyhow::Result<()>) -> ExitCode {
    out::color();
    ExitCode::from(code(main()))
}

/// The exit code for `result`, with its error reported: as JSON on stdout
/// with `--json` while stdout is still empty (one JSON document), else
/// escaped on stderr. A code outside 0-255 (a negative Windows status)
/// becomes 1.
pub fn code(result: anyhow::Result<()>) -> u8 {
    let Err(error) = result else {
        return 0;
    };
    let mut code = 1;
    for cause in error.chain() {
        if let Some(StdoutError(e)) = cause.downcast_ref::<StdoutError>() {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                return CLOSED_STDOUT;
            }
        }
        if let Some(ExitWith(n)) = cause.downcast_ref::<ExitWith>() {
            return u8::try_from(*n).unwrap_or(1);
        }
        if cause.is::<Usage>() {
            code = USAGE;
        }
    }
    if out::json() && !out::wrote_stdout() {
        let report = serde_json::json!({ "success": false, "error": format!("{error:#}") });
        if crate::outln!("{report}").is_ok() {
            return code;
        }
    }
    if code == USAGE {
        crate::errln!(
            "{}: {}",
            "error".red().bold(),
            escape_lines(&error.to_string())
        );
    } else {
        // as Rust prints an error returned from main, but escaped: it can
        // carry paths and names from git and the filesystem
        crate::errln!("Error: {}", escape_lines(&format!("{error:?}")));
    }
    code
}

/// End the program now with `code`. Only for where no `main` can return
/// it: a plugin's harness and a signal that ends the program.
pub fn now(code: i32) -> ! {
    #[expect(
        clippy::exit,
        reason = "the one place a program ends without returning from main"
    )]
    std::process::exit(code)
}
