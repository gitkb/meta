//! Command-line errors that never echo control characters.

use crate::display::escape;
use crate::exit::ExitWith;
use crate::out;
use crate::out::StdoutError;
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

/// Parse `P` from this process's arguments. An error is printed with
/// [`escaped_error`] (help and version print as usual, and so does clap's
/// invalid UTF-8 error, which doesn't echo the value) and returned as the
/// [`ExitWith`] code clap gives it: 0 for help, 2 for a usage error.
pub fn parse<P: clap::Parser>() -> anyhow::Result<P> {
    use clap::error::ErrorKind;
    let e = match P::try_parse() {
        Ok(parsed) => return Ok(parsed),
        Err(e) => e,
    };
    let e = match e.kind() {
        // a bare command group (`meta plugin`) answers with its help, as
        // asking for it does
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            out!("{}", e.render())?;
            return Err(ExitWith(0).into());
        }
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion | ErrorKind::InvalidUtf8 => e,
        _ => with_option_tip::<P>(escaped_error::<P>(std::env::args_os())),
    };
    match e.print() {
        Err(io) if !e.use_stderr() => Err(StdoutError(io).into()),
        _ => Err(ExitWith(e.exit_code()).into()),
    }
}

/// `error`, for an option of `P`'s own given after a subcommand
/// (`--dry-run` after `meta exec`), with the tip that it goes before the
/// subcommand in place of clap's, which would pass it to the command.
fn with_option_tip<P: clap::Parser>(mut error: clap::Error) -> clap::Error {
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    if error.kind() != ErrorKind::UnknownArgument {
        return error;
    }
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
    let Some(own) = own else {
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

/// The one word of `candidates` that `typed` is a typo of: the nearest by
/// edits (a swap of two letters counts as one), at most one edit away for a
/// word under five letters and two for a longer one. `None` when no word is
/// that near, or two are equally near.
pub fn nearest<'a>(typed: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let limit = if typed.chars().count() < 5 { 1 } else { 2 };
    let mut best: Option<(usize, &str)> = None;
    let mut tied = false;
    for candidate in candidates {
        let distance = strsim::osa_distance(typed, candidate);
        if distance == 0 || distance > limit {
            continue;
        }
        match best {
            Some((nearest, word)) if distance == nearest && word != candidate => tied = true,
            Some((nearest, _)) if distance >= nearest => {}
            _ => {
                best = Some((distance, candidate));
                tied = false;
            }
        }
    }
    best.filter(|_| !tied).map(|(_, word)| word)
}

#[cfg(test)]
mod tests {
    use super::nearest;

    const COMMANDS: &[&str] = &[
        "context", "exec", "git", "help", "init", "plugin", "project",
    ];

    #[test]
    fn a_typo_names_the_one_command_it_is_near() {
        assert_eq!(nearest("gti", COMMANDS.iter().copied()), Some("git"));
        assert_eq!(nearest("exce", COMMANDS.iter().copied()), Some("exec"));
        assert_eq!(nearest("projet", COMMANDS.iter().copied()), Some("project"));
        assert_eq!(
            nearest("lsit", ["list", "check", "dependents"]),
            Some("list")
        );
    }

    #[test]
    fn a_word_near_nothing_or_near_two_names_none() {
        assert_eq!(nearest("query", COMMANDS.iter().copied()), None);
        assert_eq!(nearest("npm", COMMANDS.iter().copied()), None);
        // the word itself is no typo
        assert_eq!(nearest("git", COMMANDS.iter().copied()), None);
        assert_eq!(nearest("ab", ["ax", "ay"]), None);
    }
}
