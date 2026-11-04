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

/// Parse `P`, meta's command line, from this process's arguments. An error
/// is printed with [`escaped_error`] (help and version print as usual, and
/// so does clap's invalid UTF-8 error, which doesn't echo the value) and
/// returned as the [`ExitWith`] code clap gives it: 0 for help, 2 for a
/// usage error.
pub fn parse<P: clap::Parser>() -> anyhow::Result<P> {
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    let e = match P::try_parse() {
        Ok(parsed) => return Ok(parsed),
        Err(e) => e,
    };
    let mut e = match e.kind() {
        // a bare command group (`meta plugin`) answers with its help, as
        // asking for it does
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            out!("{}", e.render())?;
            return Err(ExitWith(0).into());
        }
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion | ErrorKind::InvalidUtf8 => e,
        _ => escaped_error::<P>(std::env::args_os()),
    };
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let meta = MetaOptions::of(&P::command(), &args);
    if let Some(tip) = meta.rejected(&e, args.get(meta.typed.len()..).unwrap_or_default()) {
        e.insert(
            ContextKind::Suggested,
            ContextValue::StyledStrs(vec![tip.into()]),
        );
    }
    match e.print() {
        Err(io) if !e.use_stderr() => Err(StdoutError(io).into()),
        _ => Err(ExitWith(e.exit_code()).into()),
    }
}

/// `error` as the text of a usage error: without clap's `error: ` label.
pub fn message(error: &clap::Error) -> String {
    let message = error.to_string();
    let message = message.strip_prefix("error: ").unwrap_or(&message);
    message.trim_end().to_string()
}

/// One of meta's own options: its spellings (`--recursive`, `-r`).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MetaOption {
    pub spellings: Vec<String>,
    /// Whether a value follows it (`--include api`)
    pub takes_value: bool,
    /// The command at hand never takes it, before it or after it
    #[serde(skip)]
    pub refused: bool,
}

/// meta's own options, which go before the command, and what was typed of
/// them: what a command needs to tell a user who typed one after it where
/// it goes, with the line to run.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MetaOptions {
    pub options: Vec<MetaOption>,
    /// The words between `meta` and the command word: meta's options, and
    /// their values, as typed
    pub typed: Vec<String>,
}

impl MetaOptions {
    /// The options of `meta` (meta's command line, unbuilt: without
    /// `--help`), and what of `args` (the words after the program's name)
    /// is them, up to the command word.
    pub fn of(meta: &clap::Command, args: &[String]) -> Self {
        let options: Vec<MetaOption> = meta
            .get_arguments()
            .filter(|arg| !arg.is_positional())
            .map(|arg| MetaOption {
                spellings: arg
                    .get_long()
                    .map(|long| format!("--{long}"))
                    .into_iter()
                    .chain(arg.get_short().map(|short| format!("-{short}")))
                    .collect(),
                takes_value: arg.get_action().takes_values(),
                refused: false,
            })
            .collect();
        let mut typed = Vec::new();
        let mut words = args.iter();
        while let Some(word) = words.next() {
            let spelling = word.split('=').next().unwrap_or(word);
            let Some(option) = options
                .iter()
                .find(|o| o.spellings.iter().any(|s| s == spelling))
            else {
                break;
            };
            typed.push(word.clone());
            if option.takes_value && !word.contains('=') {
                typed.extend(words.next().cloned());
            }
        }
        Self { options, typed }
    }

    /// These, with the ones named by a long spelling in `longs` refused:
    /// the command they are for takes none of them.
    pub fn refusing(&self, longs: &[&str]) -> Self {
        let mut refused = self.clone();
        for option in &mut refused.options {
            option.refused |= option.spellings.iter().any(|s| longs.contains(&s.as_str()));
        }
        refused
    }

    /// The tip for clap's `error` about `command` (the words from the
    /// command word on, as typed): see [`Self::tip`].
    pub fn rejected(&self, error: &clap::Error, command: &[String]) -> Option<String> {
        use clap::error::{ContextKind, ContextValue, ErrorKind};
        if error.kind() != ErrorKind::UnknownArgument {
            return None;
        }
        match error.get(ContextKind::InvalidArg) {
            Some(ContextValue::String(arg)) => self.tip(command, arg),
            _ => None,
        }
    }

    /// The tip for `rejected`, a word that `command` (the words from the
    /// command word on, as typed) refused, when it is one of meta's options:
    /// it goes before the command, as in the line typed with it (and its
    /// value) moved there. A line that would fail again is never shown: none
    /// for an option the command never takes, and none that moves an option
    /// taking no value away from the word typed right after it
    /// (`--parallel 8`); the rule alone is.
    pub fn tip(&self, command: &[String], rejected: &str) -> Option<String> {
        let spelling = rejected.split('=').next().unwrap_or(rejected);
        let option = self
            .options
            .iter()
            .find(|o| o.spellings.iter().any(|s| s == spelling))?;
        if option.refused {
            return Some(format!(
                "{spelling} is an option of meta itself, and this command doesn't take it"
            ));
        }
        let rule = format!("{spelling} is an option of meta itself: it goes before the command");
        let at = command.iter().position(|word| {
            word == rejected || rejected == spelling && word.starts_with(&format!("{spelling}="))
        });
        let Some((at, word)) = at.and_then(|at| Some((at, command.get(at)?))) else {
            return Some(rule);
        };
        let next = command.get(at + 1);
        let moved = match (option.takes_value && !word.contains('='), next) {
            (true, Some(_)) => 2,
            (true, None) => return Some(rule),
            (false, Some(next)) if !next.starts_with('-') => return Some(rule),
            (false, _) => 1,
        };
        let line: Vec<String> = std::iter::once("meta".to_string())
            .chain(self.typed.iter().cloned())
            .chain(command.iter().skip(at).take(moved).cloned())
            .chain(command.iter().take(at).cloned())
            .chain(command.iter().skip(at + moved).cloned())
            .collect();
        Some(format!(
            "{rule}, as in {}",
            escape(&crate::display::display_command(&line))
        ))
    }
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
