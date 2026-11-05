//! How meta's programs end, decided in one place: the exit codes are
//! [`CODES`], and nothing else defines them (the docs' table is
//! [`table`]'s). A plugin's own exit code passes through ([`ExitWith`]).

use crate::display::escape_lines;
use crate::out::{self, StdoutError};
use colored::Colorize;
use std::process::{ExitCode, ExitStatus};

/// Done.
pub const SUCCESS: i32 = 0;

/// Something failed; the error was printed.
pub const FAILURE: i32 = 1;

/// Exit status for a command line that can't be run, as clap's.
pub const USAGE: i32 = 2;

/// The exit status of a program that signal `n` ended (or that ended
/// because it got it): 128 + n, as a shell reports it.
pub const fn signalled(n: i32) -> i32 {
    128 + n
}

/// Exit status for output cut off by a closed stdout (SIGPIPE, 13): a
/// failure it hid isn't success.
pub const CLOSED_STDOUT: i32 = signalled(13);

/// Every exit code of meta's programs, and what it means.
pub const CODES: &[(&str, &str)] = &[
    ("0", "Done"),
    (
        "1",
        "Something failed: a command in at least one repo (the summary names them), or meta itself (`Error: …` on stderr; with `--json`, the Envelope's `error`)",
    ),
    (
        "2",
        "The command line is wrong: an unknown command, or an option or value clap rejects",
    ),
    (
        "141",
        "Its output was cut off: stdout closed early, as in `meta exec -- git log \\| head`",
    ),
    ("128 + n", "Stopped by signal n (Ctrl-C: 130), or a plugin was"),
];

/// [`CODES`] as the Markdown table the docs show.
pub fn table() -> String {
    let rows: String = CODES
        .iter()
        .map(|(code, meaning)| format!("| {code} | {meaning} |\n"))
        .collect();
    format!("| Code | Meaning |\n|------|---------|\n{rows}")
}

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
            return ExitWith(signalled(signal));
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
    let code = code(flushed(main()));
    // an ExitCode holds a byte; Windows passes on a plugin's 256 or -1
    match u8::try_from(code) {
        Ok(code) => ExitCode::from(code),
        Err(_) => now(code),
    }
}

/// `result`, after what is still buffered (a last line without its
/// newline) went out: a closed stdout found then ends the program as one
/// found before, whatever else happened.
pub fn flushed(result: anyhow::Result<()>) -> anyhow::Result<()> {
    match (result, out::flush()) {
        (result, Ok(())) => result,
        (Err(e), Err(StdoutError(io))) if io.kind() != std::io::ErrorKind::BrokenPipe => Err(e),
        (_, Err(closed)) => Err(closed.into()),
    }
}

/// The ending `error` carries, already reported: an [`ExitWith`] code, or
/// [`CLOSED_STDOUT`] for a closed stdout.
pub fn ending(error: &anyhow::Error) -> Option<i32> {
    error.chain().find_map(|cause| {
        if let Some(ExitWith(code)) = cause.downcast_ref() {
            return Some(*code);
        }
        cause
            .downcast_ref::<StdoutError>()
            .filter(|e| e.0.kind() == std::io::ErrorKind::BrokenPipe)
            .map(|_| CLOSED_STDOUT)
    })
}

/// The exit code for `result`, with its error reported, escaped, on
/// stderr. (Under `--json` meta reports it in its Envelope first and ends
/// with [`ExitWith`].)
pub fn code(result: anyhow::Result<()>) -> i32 {
    let Err(error) = result else {
        return SUCCESS;
    };
    if let Some(code) = ending(&error) {
        return code;
    }
    let kind = crate::envelope::code_of(&error);
    let usage = kind == crate::ErrorCode::Usage;
    let code = kind.exit_code();
    // escaped: an error can carry paths and names from git and the
    // filesystem
    if usage {
        crate::errln!(
            "{}: {}",
            out::label("error", |l| l.red().bold()),
            escape_lines(&format!("{error:#}"))
        );
    } else {
        // as Rust prints an error returned from main
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_ending_has_its_code() {
        let closed = || StdoutError(std::io::ErrorKind::BrokenPipe.into());
        assert_eq!(code(Ok(())), 0);
        assert_eq!(code(Err(anyhow::anyhow!("failed"))), 1);
        assert_eq!(code(Err(Usage("bad".into()).into())), USAGE);
        // found under the context added on the way up
        assert_eq!(
            code(Err(anyhow::Error::new(closed()).context("writing"))),
            CLOSED_STDOUT
        );
        assert_eq!(
            code(Err(anyhow::Error::new(ExitWith(3)).context("plugin"))),
            3
        );
        // Windows hands on a plugin's code past a byte
        assert_eq!(code(Err(ExitWith(256).into())), 256);
        assert_eq!(ending(&anyhow::anyhow!("failed")), None);
    }
}
