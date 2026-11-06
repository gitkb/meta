//! A manifest's options as a command line: clap built at run time from the
//! declaration, and what it parsed as the JSON object a request carries.

use crate::manifest::{Opt, Type};
use crate::wire_path::Word;
use clap::builder::{PossibleValuesParser, ValueParser};
use clap::{Arg, ArgAction, ArgMatches};
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::path::PathBuf;

/// `options` as clap arguments, in the order declared.
pub fn args(options: &[Opt]) -> Vec<Arg> {
    let mut index = 0;
    options
        .iter()
        .map(|option| {
            let at = option.positional.is_some().then(|| {
                index += 1;
                index
            });
            arg(option, at)
        })
        .collect()
}

/// `option` as a clap argument; `at` is a positional's place.
fn arg(option: &Opt, at: Option<usize>) -> Arg {
    let mut arg = Arg::new(option.id().to_string())
        .help(option.help.clone())
        .required(option.required);
    if let Some(help) = &option.long_help {
        arg = arg.long_help(help.clone());
    }
    if let Some(long) = &option.long {
        arg = arg.long(long.clone());
    }
    if let Some(short) = option.short {
        arg = arg.short(short);
    }
    for alias in &option.alias {
        arg = arg.visible_alias(alias.clone());
    }
    if let Some(at) = at {
        arg = arg.index(at);
    }
    if let Some(value) = &option.value {
        arg = arg.value_name(value.clone());
    }
    arg = match option.is_flag() {
        true => arg.action(ArgAction::SetTrue),
        false => arg
            .action(match option.is_list() {
                true => ArgAction::Append,
                false => ArgAction::Set,
            })
            .value_parser(value_parser(option)),
    };
    if option.rest || option.last {
        arg = arg.num_args(0..).trailing_var_arg(option.rest);
    }
    if option.last {
        arg = arg.last(true);
    }
    if let Some(delimiter) = option.delimiter {
        arg = arg.value_delimiter(delimiter);
    }
    if option.allow_hyphen {
        arg = arg.allow_hyphen_values(true);
    }
    if let Some(default) = &option.default {
        arg = arg.default_value(default.clone());
    }
    for id in &option.requires {
        arg = arg.requires(id.clone());
    }
    arg.conflicts_with_all(option.conflicts.clone())
}

fn value_parser(option: &Opt) -> ValueParser {
    match option.kind {
        // words for another program: any bytes
        Type::String if option.is_words() => ValueParser::os_string(),
        Type::String if !option.choices.is_empty() => {
            PossibleValuesParser::new(option.choices.clone()).into()
        }
        Type::String | Type::Bool => ValueParser::string(),
        Type::Int => clap::value_parser!(i64).into(),
        Type::Uint => clap::value_parser!(usize).into(),
        Type::Path => clap::value_parser!(PathBuf),
        Type::Duration => ValueParser::new(|value: &str| {
            crate::duration::parse_duration(value).map_err(|e| e.to_string())
        }),
    }
}

/// How `options` differ from `command`, a clap type of the same command, as
/// lines naming each difference. For the test that keeps a plugin's clap
/// types and its manifest in step, until the host parses for plugins and
/// the clap types go.
pub fn differences(summary: &str, options: &[Opt], command: &clap::Command) -> Vec<String> {
    let mut declared = clap::Command::new("x")
        .disable_help_flag(true)
        .args(args(options));
    declared.build();
    let mut actual = command
        .clone()
        .disable_help_flag(true)
        .disable_version_flag(true);
    actual.build();
    let mut differences = Vec::new();
    let about = actual
        .get_about()
        .map(ToString::to_string)
        .unwrap_or_default();
    if about != summary {
        differences.push(format!("summary: {summary:?}, clap has {about:?}"));
    }
    let (declared, actual) = (shapes(&declared), shapes(&actual));
    for shape in declared.iter().filter(|shape| !actual.contains(shape)) {
        differences.push(format!("declared: {shape}"));
    }
    for shape in actual.iter().filter(|shape| !declared.contains(shape)) {
        differences.push(format!("in clap: {shape}"));
    }
    differences
}

/// Each argument of `command` as one line of what it means, its order
/// included: ids differ between the two, so they are named by spelling.
fn shapes(command: &clap::Command) -> Vec<String> {
    let name = |arg: &Arg| match arg.get_long() {
        Some(long) => format!("--{long}"),
        None => arg.get_id().as_str().replace('_', "-"),
    };
    command
        .get_arguments()
        .filter(|arg| !matches!(arg.get_id().as_str(), "help" | "version"))
        .map(|arg| {
            let mut conflicts: Vec<String> = command
                .get_arg_conflicts_with(arg)
                .into_iter()
                .map(name)
                .collect();
            conflicts.sort();
            format!(
                "{} short={:?} at={:?} value={} many={} required={} default={:?} last={} \
                 hyphen={} conflicts={conflicts:?} help={:?}",
                name(arg),
                arg.get_short(),
                arg.get_index(),
                arg.get_action().takes_values(),
                matches!(arg.get_action(), ArgAction::Append),
                arg.is_required_set(),
                arg.get_default_values(),
                arg.is_last_set(),
                arg.is_allow_hyphen_values_set(),
                arg.get_help().map(ToString::to_string).unwrap_or_default(),
            )
        })
        .collect()
}

/// What `matches` holds of `options`, by option id: a flag as a bool, a
/// value as its type, a list as an array. An option not given is left out.
pub fn values(matches: &ArgMatches, options: &[Opt]) -> Map<String, Value> {
    options
        .iter()
        .filter_map(|option| Some((option.id().to_string(), value(matches, option)?)))
        .collect()
}

fn value(matches: &ArgMatches, option: &Opt) -> Option<Value> {
    let id = option.id();
    if option.is_flag() {
        return Some(Value::Bool(matches.get_flag(id)));
    }
    let list = |values: Vec<Value>| match option.is_list() {
        true => Some(Value::Array(values)),
        false => values.into_iter().next(),
    };
    fn each<T: Clone + Send + Sync + 'static>(
        matches: &ArgMatches,
        id: &str,
        to_json: impl Fn(&T) -> Value,
    ) -> Vec<Value> {
        matches
            .get_many::<T>(id)
            .into_iter()
            .flatten()
            .map(to_json)
            .collect()
    }
    let values = match option.kind {
        Type::String if option.is_words() => each::<OsString>(matches, id, |v| json!(Word::of(v))),
        Type::String | Type::Bool => each::<String>(matches, id, |v| json!(v)),
        Type::Int => each::<i64>(matches, id, |v| json!(v)),
        Type::Uint => each::<usize>(matches, id, |v| json!(v)),
        Type::Duration => each::<u64>(matches, id, |v| json!(v)),
        Type::Path => each::<PathBuf>(matches, id, |v| crate::wire_path::to_json(v)),
    };
    match values.is_empty() && !option.is_list() {
        true => None,
        false => list(values),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;

    fn options() -> Vec<Opt> {
        let text = r#"
name = "a"
run = ["p"]
[commands.x]
options = [
  { positional = "name", required = true },
  { long = "tag", short = "t", value = "T", delimiter = "," },
  { long = "depth", value = "N", type = "uint" },
  { long = "wait", value = "D", type = "duration", default = "1m" },
  { long = "mode", value = "M", choices = ["fast", "slow"] },
  { long = "every", conflicts = ["tag"] },
  { long = "dir", value = "PATH", type = "path" },
  { positional = "command", last = true },
]
"#;
        let manifest = Manifest::parse(text).unwrap();
        manifest.commands["x"].options.clone()
    }

    fn parse(line: &[&str]) -> Result<Value, clap::Error> {
        let options = options();
        let matches = clap::Command::new("x")
            .args(args(&options))
            .try_get_matches_from(std::iter::once(&"x").chain(line))?;
        Ok(Value::Object(values(&matches, &options)))
    }

    #[test]
    fn what_is_parsed_is_typed_by_its_declaration() {
        let got = parse(&[
            "n", "-t", "a,b", "--depth", "2", "--dir", "/w", "--", "ls", "-l",
        ]);
        assert_eq!(
            got.unwrap(),
            json!({"name": "n", "tag": ["a", "b"], "depth": 2, "wait": 60, "every": false,
                   "dir": "/w", "command": [{"text": "ls"}, {"text": "-l"}]})
        );
    }

    #[test]
    fn the_declaration_refuses_what_it_rules_out() {
        for line in [
            &["--depth", "2"][..],
            &["n", "--depth", "x"],
            &["n", "--wait", "soon"],
            &["n", "--mode", "other"],
            &["n", "--every", "-t", "a"],
        ] {
            assert!(parse(line).is_err(), "{line:?}");
        }
    }
}
