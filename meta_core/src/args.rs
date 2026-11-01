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
        // a bare command group (`meta plugin`) answers with its help, as
        // asking for it does
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            print!("{}", e.render());
            std::process::exit(0)
        }
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion | ErrorKind::InvalidUtf8 => e.exit(),
        _ => with_option_tip::<P>(escaped_error::<P>(std::env::args_os())).exit(),
    })
}

/// `error`, for an option of `P`'s own given after a subcommand
/// (`meta exec --dry-run -- rm x`), with the tip that it goes before the
/// subcommand in place of clap's, which would pass it to the command.
fn with_option_tip<P: clap::Parser>(mut error: clap::Error) -> clap::Error {
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    let Some(ContextValue::String(arg)) = error.get(ContextKind::InvalidArg) else {
        return error;
    };
    let option = arg.split('=').next().unwrap_or(arg).to_string();
    let command = P::command();
    let own = command.get_arguments().find(|a| {
        a.get_long()
            .is_some_and(|long| option == format!("--{long}"))
            || a.get_short()
                .is_some_and(|short| option == format!("-{short}"))
    });
    let Some(own) = own.filter(|_| error.kind() == ErrorKind::UnknownArgument) else {
        return error;
    };
    let value = match own.get_value_names() {
        Some([value, ..]) if own.get_action().takes_values() => {
            format!(" <{value}>")
        }
        _ => String::new(),
    };
    let name = command.get_name();
    let tip = format!(
        "{option} is an option of {name} itself: it goes before the command, as in \
         `{name} {option}{value} <COMMAND>`"
    );
    error.insert(
        ContextKind::Suggested,
        ContextValue::StyledStrs(vec![tip.into()]),
    );
    error
}
