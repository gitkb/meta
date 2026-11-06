//! The manifest: what a plugin declares, in `meta-plugin.toml`. One
//! declaration per command, read by the host's index and by
//! [`crate::build`]. A plugin is its commands: each has `run` (a program in
//! any language) or `plan` (an argv meta runs itself), never both; a
//! manifest-level `run` is the default for a command with no `plan`.
//!
//! An unknown key is an error, so a typo can't quietly mean nothing. What a
//! single command gets wrong is that command's problem alone
//! ([`Manifest::command_problems`]): the host disables it and keeps the
//! others.

use crate::Scope;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The manifest's file name, in the plugin's own folder.
pub const FILE: &str = "meta-plugin.toml";

/// The most of a manifest that is read: more than this is no manifest, and
/// is refused rather than held in memory.
pub const MOST: u64 = 1 << 20;

/// A plugin's declaration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The program of every command that has no `plan` of its own
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Run>,
    /// Commands of other providers this manifest replaces
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<String>,
    /// Commands this plugin calls back through meta
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<String>,
    /// meta's own options: `meta-host.toml` alone declares them
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub meta_options: Vec<Opt>,
    #[serde(default)]
    pub commands: IndexMap<String, Command>,
    /// What this plugin reads from `.meta.yaml`
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub settings: IndexMap<String, Setting>,
    /// Commands of others this plugin hooks into
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extend: Vec<Extend>,
}

/// One command, by the words that reach it (`[commands."git worktree list"]`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub effects: Effect,
    #[serde(default)]
    pub scope: Scope,
    #[serde(default)]
    pub preview: Preview,
    /// Its process group is stopped after this long (`5m`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    #[serde(default)]
    pub output: Output,
    #[serde(default)]
    pub text: Text,
    /// What the host adds to the request
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<Need>,
    /// A project holding none of these files is skipped
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only_if: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<Run>,
    /// An argv meta runs itself in each project; `{raw}` and `{option}`
    /// are filled in as whole words
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<Vec<String>>,
    /// It reaches the network
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub network: bool,
    /// At most this many projects at once
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<Checkout>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<Opt>,
    /// The JSON Schema of its results' `data`, beside the manifest
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// The argv that lists completions for the words after a raw command
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub complete: Vec<String>,
    /// The words after it are the tool's: meta parses none of them
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub raw: bool,
    /// Other last words that reach it
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alias: Vec<String>,
    /// Left out of help and completions
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// What a command does to the repos it runs in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    Read,
    Write,
    Destructive,
    #[default]
    Unknown,
}

/// What `--dry-run` may do with a command.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preview {
    /// Ask it for its plan, and print that
    Plan,
    /// Say it would run, and run nothing
    #[default]
    None,
}

/// How a command's output reaches the user.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Output {
    /// The host prints the one Envelope
    #[default]
    Envelope,
    /// The command owns the terminal, and refuses `--json`
    Terminal,
    /// Events as they happen, then the Envelope
    Stream,
}

/// Who renders a command's results as text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Text {
    #[default]
    Host,
    Plugin,
}

/// What a command asks the host to add to its request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Need {
    /// Every worktree set of the workspace
    Sets,
}

/// How a provider's checkouts are told from any other folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkout {
    pub marker: String,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    pub env: IndexMap<String, String>,
}

/// A plugin setting from `.meta.yaml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub repeat: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

/// A hook into another command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extend {
    pub command: String,
    pub when: When,
    #[serde(default)]
    pub on: On,
    pub call: String,
}

/// Which side of the command a hook runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum When {
    Before,
    After,
}

/// Which endings of the command an after-hook runs on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum On {
    #[default]
    Success,
    Always,
}

/// The program a command runs: one argv, or per platform a list of argvs
/// of which the first found wins (`python3` isn't on every Windows).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Run {
    Argv(Vec<String>),
    Platform(PlatformRun),
}

/// The `[run]` table.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformRun {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unix: Vec<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<Vec<String>>,
}

impl Run {
    /// The argvs to try on this platform, in order.
    pub fn candidates(&self) -> Vec<&[String]> {
        match self {
            Run::Argv(argv) => vec![argv.as_slice()],
            Run::Platform(run) => {
                let list = if cfg!(windows) {
                    &run.windows
                } else {
                    &run.unix
                };
                list.iter().map(Vec::as_slice).collect()
            }
        }
    }

    /// The argv to start, from `dir` (the plugin's folder): the first
    /// candidate whose program is there, or in `path`'s folders, with the
    /// program as found. `None` when no candidate's is.
    pub fn resolve(&self, dir: &Path, path: &[PathBuf]) -> Option<Vec<String>> {
        self.candidates().into_iter().find_map(|argv| {
            let (program, rest) = argv.split_first()?;
            let found = find_program(program, dir, path)?;
            Some(
                std::iter::once(found.to_string_lossy().into_owned())
                    .chain(rest.iter().cloned())
                    .collect(),
            )
        })
    }
}

/// `program`, with the platform's executable suffix if it has one: from
/// `dir` when it names a path or is there, else from a folder of `path`.
fn find_program(program: &str, dir: &Path, path: &[PathBuf]) -> Option<PathBuf> {
    let suffixed = |folder: &Path| {
        let plain = folder.join(program);
        let with_suffix = plain.with_file_name(format!(
            "{}{}",
            plain.file_name()?.to_string_lossy(),
            std::env::consts::EXE_SUFFIX
        ));
        [with_suffix, plain].into_iter().find(|file| file.is_file())
    };
    let named_path = program.contains('/') || program.contains(std::path::MAIN_SEPARATOR);
    suffixed(dir).or_else(|| match named_path {
        true => None,
        false => path.iter().find_map(|folder| suffixed(folder)),
    })
}

/// The kind of value an option takes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Type {
    #[default]
    String,
    Int,
    Uint,
    Bool,
    Path,
    Duration,
}

/// An option or positional of a command, or one of meta's own.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Opt {
    /// `--long`, without its dashes. With no `value` it is a flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long: Option<String>,
    /// `-s`, without its dash
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short: Option<char>,
    /// The name of a positional argument
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub positional: Option<String>,
    /// What help calls its value; an option with one takes a value
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "is_string")]
    pub kind: Type,
    /// It may be given more than once
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub repeat: bool,
    /// One value holds several, split at this character
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<char>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// This positional takes every word left
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rest: bool,
    /// This positional takes the words after `--`
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub last: bool,
    /// The ids of the options it can't be given with
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
    /// The ids of the options it needs
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    /// Its value may start with `-`
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_hyphen: bool,
    #[serde(default)]
    pub help: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_help: Option<String>,
    /// Other long spellings
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alias: Vec<String>,
    /// The argv that lists its values, for completion
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub complete: Vec<String>,
}

fn is_string(kind: &Type) -> bool {
    *kind == Type::String
}

impl Opt {
    /// What it is called in `conflicts`, `requires` and a request's
    /// options: its long name, or its positional's.
    pub fn id(&self) -> &str {
        self.long
            .as_deref()
            .or(self.positional.as_deref())
            .unwrap_or_default()
    }

    /// Whether it is a flag: an option that takes no value.
    pub fn is_flag(&self) -> bool {
        self.positional.is_none() && self.value.is_none()
    }

    /// Whether it holds any number of values.
    pub fn is_list(&self) -> bool {
        self.repeat || self.rest || self.last || self.delimiter.is_some()
    }

    /// Whether it holds words for another program, as typed.
    pub fn is_words(&self) -> bool {
        self.kind == Type::String && (self.rest || self.last)
    }

    /// What is wrong with it on its own.
    fn problems(&self) -> Vec<String> {
        let id = self.id();
        let mut problems = Vec::new();
        let mut say = |problem: String| problems.push(problem);
        match (&self.long, &self.positional) {
            (Some(_), Some(_)) => say(format!("{id}: both long and positional")),
            (None, None) if self.short.is_some() => say("a short option needs a long".to_string()),
            (None, None) => say("an option needs long or positional".to_string()),
            _ => {}
        }
        if !id.is_empty() && !id.chars().all(|c| c.is_alphanumeric() || "-_".contains(c)) {
            say(format!("{id:?} is no option name"));
        }
        if id.starts_with('-') {
            say(format!("{id:?}: write it without its dashes"));
        }
        if self.short.is_some_and(|c| !c.is_ascii_alphanumeric()) {
            say(format!("{id}: short must be a letter or digit"));
        }
        if self.kind == Type::Bool && !self.is_flag() {
            say(format!("{id}: a bool is a flag, with no value"));
        }
        if self.is_flag() && self.kind != Type::Bool && self.kind != Type::String {
            say(format!("{id}: a typed option needs a value"));
        }
        if self.is_flag() && (self.repeat || self.default.is_some() || !self.choices.is_empty()) {
            say(format!("{id}: a flag has no values"));
        }
        if (self.rest || self.last) && self.positional.is_none() {
            say(format!("{id}: rest and last are for positionals"));
        }
        if !self.choices.is_empty() && self.kind != Type::String {
            say(format!("{id}: choices are for strings"));
        }
        if let Some(problem) = self.default.as_deref().and_then(|d| self.bad_value(d)) {
            say(format!("{id}: default: {problem}"));
        }
        problems
    }

    /// Why `value` can't be a value of this option.
    fn bad_value(&self, value: &str) -> Option<String> {
        let bad = match self.kind {
            Type::Int => value.parse::<i64>().err().map(|e| e.to_string()),
            Type::Uint => value.parse::<usize>().err().map(|e| e.to_string()),
            Type::Duration => crate::duration::parse_duration(value)
                .err()
                .map(|e| e.to_string()),
            Type::String | Type::Bool | Type::Path => None,
        };
        let choice = !self.choices.is_empty() && !self.choices.iter().any(|c| c == value);
        bad.or_else(|| choice.then(|| format!("{value:?} is not among its choices")))
    }
}

impl Manifest {
    /// The manifest in `text`.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| {
            let line = e.span().and_then(|span| text.get(..span.start));
            let line = line.map_or(1, |before| before.matches('\n').count() + 1);
            format!("line {line}: {}", e.message())
        })
    }

    /// The manifest in the file `path`. One larger than [`MOST`] is an error.
    pub fn load(path: &Path) -> Result<Self, String> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .and_then(|file| file.take(MOST + 1).read_to_end(&mut bytes))
            .map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MOST {
            return Err(format!("larger than {} MiB", MOST >> 20));
        }
        Self::parse(&String::from_utf8(bytes).map_err(|e| e.to_string())?)
    }

    /// What is wrong with the manifest as a whole: no command of it can be
    /// trusted to mean what it says.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let plain = |c: char| c.is_alphanumeric() || "-_".contains(c);
        if self.name.is_empty() || !self.name.chars().all(plain) {
            problems.push(format!("{:?} is no plugin name", self.name));
        }
        let run_empty = self
            .run
            .as_ref()
            .is_some_and(|run| run.candidates().iter().all(|argv| argv.is_empty()));
        if run_empty {
            problems.push("run names no program".to_string());
        }
        problems
    }

    /// What is wrong with the command `name`: each is a reason to disable
    /// it, and it alone.
    pub fn command_problems(&self, name: &str, command: &Command) -> Vec<String> {
        let mut problems = Vec::new();
        let words: Vec<&str> = name.split(' ').collect();
        let word_ok = |w: &&str| {
            !w.is_empty() && !w.starts_with('-') && w.chars().all(|c| !c.is_control() && c != '=')
        };
        if !words.iter().all(word_ok) {
            problems.push(format!("{name:?} is no command"));
        }
        match (&command.run, &command.plan, &self.run) {
            (Some(_), Some(_), _) => problems.push("it has both run and plan".to_string()),
            (None, None, None) => problems.push("it has neither run nor plan".to_string()),
            (None, Some(plan), _) if plan.is_empty() => problems.push("plan is empty".to_string()),
            _ => {}
        }
        if let Some(timeout) = &command.timeout {
            if let Err(e) = crate::duration::parse_duration(timeout) {
                problems.push(format!("timeout: {e}"));
            }
        }
        if command.max_parallel == Some(0) {
            problems.push("max_parallel is 0".to_string());
        }
        if command.raw && !command.options.is_empty() {
            problems.push("a raw command's words are the tool's: it has no options".to_string());
        }
        problems.extend(command.options.iter().flat_map(Opt::problems));
        problems.extend(option_set_problems(&command.options));
        problems.extend(plan_problems(command));
        problems
    }
}

/// What is wrong with `options` together: a name, spelling or reference
/// that isn't unique or isn't there, and positionals out of order.
pub fn option_set_problems(options: &[Opt]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for option in options {
        let spellings = option
            .alias
            .iter()
            .map(|alias| format!("--{alias}"))
            .chain(option.long.iter().map(|long| format!("--{long}")))
            .chain(option.short.iter().map(|short| format!("-{short}")))
            .chain(option.positional.clone());
        for spelling in spellings {
            if !seen.insert(spelling.clone()) {
                problems.push(format!("{spelling} is declared twice"));
            }
        }
        for reference in option.conflicts.iter().chain(&option.requires) {
            if !options.iter().any(|other| other.id() == reference) {
                problems.push(format!("{} names no option {reference}", option.id()));
            }
        }
    }
    let positionals: Vec<&Opt> = options.iter().filter(|o| o.positional.is_some()).collect();
    let open = positionals
        .iter()
        .position(|o| o.is_list() && !o.last)
        .map_or(0, |at| at + 1);
    let (_, after) = positionals.split_at(open.min(positionals.len()));
    if open > 0 {
        if let Some(next) = after.iter().find(|o| !o.last) {
            problems.push(format!("{} follows a positional list", next.id()));
        }
    }
    problems
}

/// The placeholders of a command's `plan` that name nothing.
fn plan_problems(command: &Command) -> Vec<String> {
    command
        .plan
        .iter()
        .flatten()
        .filter_map(|word| word.strip_prefix('{')?.strip_suffix('}'))
        .filter(|name| {
            let raw = *name == "raw" && command.raw;
            !raw && !command.options.iter().any(|o| o.id() == *name)
        })
        .map(|name| format!("plan names {{{name}}}, which it doesn't declare"))
        .collect()
}

/// What `meta --json help` prints: every command there is, by whom it is
/// answered, and what is wrong.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Index {
    /// meta's own options, which go before the command
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub meta_options: Vec<Opt>,
    pub commands: Vec<Indexed>,
    /// `meta-<word>` files with no manifest
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<PluginFile>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<Problem>,
}

/// A command in the index.
#[derive(Debug, Clone, Serialize)]
pub struct Indexed {
    pub name: String,
    pub provider: Provider,
    /// Why it can't run; every other command still can
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<String>,
    /// The file that runs it today, when one does: the router runs the
    /// `meta-<word>` file of a word, whoever declares the command
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::wire_path::option::serialize"
    )]
    pub file: Option<PathBuf>,
    /// Providers that declare `overrides` for it, which the rules allow. They
    /// run nothing yet: `provider` keeps the command until the index routes
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub waiting: Vec<Provider>,
    #[serde(flatten)]
    pub command: Command,
}

/// Who declares a command.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Provider {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub source: Source,
    /// Its manifest, when it has a file
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::wire_path::option::serialize"
    )]
    pub path: Option<PathBuf>,
}

/// Where a manifest was found, farthest first: a nearer one may override a
/// farther one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// meta itself
    Host,
    /// Built into meta, for an install that has no manifest files yet
    Embedded,
    /// Beside meta's own files
    Release,
    /// The user's meta folder
    User,
    /// A `.meta/plugins` folder of the workspace
    Workspace,
}

/// A plugin file with no manifest, as the router finds it.
#[derive(Debug, Clone, Serialize)]
pub struct PluginFile {
    pub word: String,
    #[serde(serialize_with = "crate::wire_path::serialize")]
    pub path: PathBuf,
    /// The command that keeps this file from ever running
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadowed_by: Option<String>,
}

/// Something wrong in the index: only what it names is disabled.
#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    /// The manifest, or the folder, it was found in
    pub place: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(text: &str) -> Manifest {
        Manifest::parse(text).unwrap()
    }

    fn problems(text: &str) -> Vec<String> {
        let manifest = manifest(text);
        manifest
            .commands
            .iter()
            .flat_map(|(name, command)| manifest.command_problems(name, command))
            .collect()
    }

    #[test]
    fn a_command_has_run_or_plan_never_both_and_a_manifest_run_is_the_default() {
        let both = "name = \"a\"\n[commands.x]\nrun = [\"p\"]\nplan = [\"q\"]";
        assert_eq!(problems(both), ["it has both run and plan"]);
        let neither = "name = \"a\"\n[commands.x]\nsummary = \"s\"";
        assert_eq!(problems(neither), ["it has neither run nor plan"]);
        let default = "name = \"a\"\nrun = [\"p\"]\n[commands.x]\n[commands.y]\nplan = [\"q\"]";
        assert!(problems(default).is_empty());
    }

    #[test]
    fn a_typo_in_a_key_is_an_error_not_a_quiet_nothing() {
        let error = Manifest::parse("name = \"a\"\n[commands.x]\nsumary = \"s\"").unwrap_err();
        assert!(error.contains("sumary"), "{error}");
    }

    #[test]
    fn a_file_too_large_to_be_a_manifest_is_refused_unread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MOST + 1).unwrap();
        assert!(Manifest::load(&path).unwrap_err().contains("larger than"));
        std::fs::write(&path, "name = \"a\"").unwrap();
        assert_eq!(Manifest::load(&path).unwrap().name, "a");
    }

    #[test]
    fn a_plan_may_name_only_what_the_command_declares() {
        let good = "name = \"a\"\n[commands.x]\nraw = true\nplan = [\"t\", \"{raw}\"]";
        assert!(problems(good).is_empty());
        let bad = "name = \"a\"\n[commands.x]\nplan = [\"t\", \"{raw}\", \"{who}\"]";
        assert_eq!(problems(bad).len(), 2);
    }

    #[test]
    fn options_are_checked_alone_and_together() {
        let text = |options: &str| {
            format!("name = \"a\"\nrun = [\"p\"]\n[commands.x]\noptions = [{options}]")
        };
        for bad in [
            "{ long = \"a\" }, { long = \"a\" }",
            "{ long = \"a\", conflicts = [\"b\"] }",
            "{ long = \"a\", positional = \"b\" }",
            "{ long = \"--a\" }",
            "{ long = \"a\", value = \"N\", type = \"uint\", default = \"x\" }",
            "{ long = \"a\", type = \"path\" }",
            "{ positional = \"a\", rest = true }, { positional = \"b\" }",
            "{ short = \"a\" }",
        ] {
            assert!(!problems(&text(bad)).is_empty(), "{bad}");
        }
        let good = "{ positional = \"a\" }, { long = \"b\", conflicts = [\"a\"] }, \
                    { positional = \"c\", last = true }";
        assert_eq!(problems(&text(good)), Vec::<String>::new());
    }

    #[test]
    fn run_finds_the_first_candidate_there_is_with_the_platforms_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let file = format!("tool{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(bin.join(&file), "").unwrap();
        let run = Run::Platform(PlatformRun {
            unix: vec![vec!["gone".into()], vec!["tool".into(), "x.py".into()]],
            windows: vec![vec!["gone".into()], vec!["tool".into(), "x.py".into()]],
        });
        let argv = run.resolve(dir.path(), std::slice::from_ref(&bin)).unwrap();
        assert_eq!(
            argv,
            [
                bin.join(&file).to_string_lossy().into_owned(),
                "x.py".into()
            ]
        );
        // a path names one place, not the folders of PATH
        let beside = Run::Argv(vec!["./tool".into()]);
        assert_eq!(beside.resolve(dir.path(), &[bin]), None);
    }
}
