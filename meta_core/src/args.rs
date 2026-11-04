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
/// usage error; with `--json` before the command, a usage error is the
/// JSON error report on stdout.
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
    let check = |moved: &[String], rest: &[String]| {
        let line = std::iter::once("meta")
            .chain(meta.typed.iter().map(String::as_str))
            .chain(moved.iter().map(String::as_str))
            .chain(rest.iter().map(String::as_str));
        verdict(P::try_parse_from(line))
    };
    let command = args.get(meta.typed.len()..).unwrap_or_default();
    if let Some(tip) = meta.rejected(&e, command, &check) {
        e.insert(
            ContextKind::Suggested,
            ContextValue::StyledStrs(vec![tip.into()]),
        );
    }
    // with --json, the JSON error report on stdout (crate::exit::code)
    let shown = matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion);
    if !shown && meta.typed.iter().any(|word| word == "--json") {
        crate::out::set_json(true);
        return Err(crate::exit::Usage(message(&e)).into());
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
        let find = |spelling: &str| {
            options
                .iter()
                .find(|o| o.spellings.iter().any(|s| s == spelling))
        };
        let mut typed = Vec::new();
        let mut words = args.iter();
        while let Some(word) = words.next() {
            // `--include=api`, `-i api`, and a cluster of short ones (`-rv`,
            // `-iapi`: a value-taking one ends it, the rest its value)
            let value_follows = match word.strip_prefix('-') {
                Some(long) if long.starts_with('-') => {
                    match find(word.split('=').next().unwrap_or(word)) {
                        Some(option) => option.takes_value && !word.contains('='),
                        None => break,
                    }
                }
                Some(shorts) if !shorts.is_empty() => {
                    // each a short option of meta's; one that takes a value
                    // takes the rest of the word, or the next word
                    let mut value_follows = false;
                    let mut known = true;
                    for (at, short) in shorts.char_indices() {
                        match find(&format!("-{short}")) {
                            Some(option) if option.takes_value => {
                                value_follows = at + short.len_utf8() == shorts.len();
                                break;
                            }
                            Some(_) => {}
                            None => {
                                known = false;
                                break;
                            }
                        }
                    }
                    if !known {
                        break;
                    }
                    value_follows
                }
                _ => break,
            };
            typed.push(word.clone());
            if value_follows {
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
    pub fn rejected(
        &self,
        error: &clap::Error,
        command: &[String],
        check: Check,
    ) -> Option<String> {
        self.tip(command, &rejected_word(error)?, check)
    }

    /// The tip for `rejected`, a word that `command` (the words from the
    /// command word on, as typed) refused, when it is one of meta's options:
    /// it goes before the command, as in the line typed with it (and its
    /// value) moved there, and with each other option of meta's that `check`
    /// then finds rejected moved too. A line that would fail again is never
    /// shown: none for an option the command never takes, none that moves an
    /// option taking no value away from the word typed right after it
    /// (`--parallel 8`), and none that `check` rejects for anything else; the
    /// rule alone is.
    pub fn tip(&self, command: &[String], rejected: &str, check: Check) -> Option<String> {
        let spelling = |word: &str| word.split('=').next().unwrap_or(word).to_string();
        let find = |spelling: &str| {
            self.options
                .iter()
                .find(|o| o.spellings.iter().any(|s| s == spelling))
        };
        let first = spelling(rejected);
        if find(&first)?.refused {
            return Some(format!(
                "{first} is an option of meta itself, and this command doesn't take it"
            ));
        }
        let rule = format!("{first} is an option of meta itself: it goes before the command");
        let mut rest = command.to_vec();
        let mut moved: Vec<String> = Vec::new();
        let mut rejected = rejected.to_string();
        for _ in 0..command.len() {
            let name = spelling(&rejected);
            let Some(option) = find(&name).filter(|o| !o.refused) else {
                return Some(rule);
            };
            let at = rest.iter().position(|word| {
                *word == rejected || rejected == name && word.starts_with(&format!("{name}="))
            });
            let Some((at, word)) = at.and_then(|at| Some((at, rest.get(at)?))) else {
                return Some(rule);
            };
            let taken = match (option.takes_value && !word.contains('='), rest.get(at + 1)) {
                (true, Some(_)) => 2,
                (true, None) => return Some(rule),
                (false, Some(next)) if !next.starts_with('-') => return Some(rule),
                (false, _) => 1,
            };
            // a flag already before the command is said once
            let said = |w: &String| option.spellings.contains(&spelling(w));
            let again = !option.takes_value && self.typed.iter().chain(&moved).any(said);
            if !again {
                moved.extend(rest.iter().skip(at).take(taken).cloned());
            }
            rest = rest
                .iter()
                .take(at)
                .chain(rest.iter().skip(at + taken))
                .cloned()
                .collect();
            match check(&moved, &rest) {
                Ok(()) => {
                    let line: Vec<String> = std::iter::once("meta".to_string())
                        .chain(self.typed.iter().cloned())
                        .chain(moved)
                        .chain(rest)
                        .collect();
                    return Some(format!(
                        "{rule}, as in {}",
                        escape(&crate::display::display_command(&line))
                    ));
                }
                Err(Some(next)) => rejected = next,
                Err(None) => return Some(rule),
            }
        }
        Some(rule)
    }
}

/// Whether a corrected line parses, given meta's options moved before the
/// command and the command's words left: `Ok`, the word the command
/// rejects (`Err(Some)`), or another error (`Err(None)`).
pub type Check<'a> = &'a dyn Fn(&[String], &[String]) -> Result<(), Option<String>>;

/// `parsed`, a parse of a corrected line, as a [`Check`] answers.
pub fn verdict<T>(parsed: Result<T, clap::Error>) -> Result<(), Option<String>> {
    use clap::error::ErrorKind;
    match parsed {
        Ok(_) => Ok(()),
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) => Ok(()),
        Err(e) => Err(rejected_word(&e)),
    }
}

/// The word clap's `error` says it doesn't know, if that is its error.
fn rejected_word(error: &clap::Error) -> Option<String> {
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    if error.kind() != ErrorKind::UnknownArgument {
        return None;
    }
    match error.get(ContextKind::InvalidArg) {
        Some(ContextValue::String(arg)) => Some(arg.clone()),
        _ => None,
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
