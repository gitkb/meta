//! Command-line errors that never echo control characters.

use crate::display::escape;
use std::ffi::OsStr;

/// The error to show for `args` that `P` rejected. Clap quotes rejected
/// values as given, so the error comes from parsing the escaped arguments:
/// the same error, with values that can't move the cursor or forge a line.
/// If the escaped arguments parse, a plain error stands in.
pub fn escaped_error<P: clap::Parser>(
    args: impl IntoIterator<Item = impl AsRef<OsStr>>,
) -> clap::Error {
    let shown = args
        .into_iter()
        .map(|arg| escape(&arg.as_ref().to_string_lossy()).into_owned())
        .collect::<Vec<_>>();
    match P::try_parse_from(shown) {
        Err(e) => e,
        Ok(_) => P::command().error(
            clap::error::ErrorKind::InvalidValue,
            "an argument contains a control character",
        ),
    }
}

/// Parse `P` from this process's arguments, exiting on an error with
/// [`escaped_error`] (help and version print as usual, and so does clap's
/// invalid UTF-8 error, which doesn't echo the value).
pub fn parse<P: clap::Parser>() -> P {
    use clap::error::ErrorKind;
    P::try_parse().unwrap_or_else(|e| match e.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayVersion
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        | ErrorKind::InvalidUtf8 => e.exit(),
        _ => escaped_error::<P>(std::env::args_os()).exit(),
    })
}
