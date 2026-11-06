//! Shell completion: `meta completions <shell>` prints a script, which calls
//! `meta __complete` on each TAB. Everything it offers comes from the index.

use crate::index;
use meta_plugin::manifest::{Index, Indexed, Opt};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a `complete` argv gets: a TAB that hangs is worse than none.
const DEADLINE: Duration = Duration::from_millis(500);

/// The most a `complete` argv is read of.
const MOST: u64 = 1 << 20;

/// The word being completed, for a `complete` argv.
const WORD_VAR: &str = "META_COMPLETE_WORD";

/// The script for `shell`.
pub fn script(shell: &str) -> &'static str {
    match shell {
        "bash" => {
            r#"_meta() {
    local IFS=$'\n'
    COMPREPLY=($(meta __complete -- "${COMP_WORDS[@]:1:COMP_CWORD}" 2>/dev/null))
}
complete -o default -o bashdefault -F _meta meta
"#
        }
        "zsh" => {
            r#"#compdef meta
_meta() {
    local -a found
    found=("${(@f)$(meta __complete -- "${(@)words[2,CURRENT]}" 2>/dev/null)}")
    compadd -a found
}
compdef _meta meta
"#
        }
        "fish" => {
            r#"function __meta_complete
    set -l words (commandline -opc)
    meta __complete -- $words[2..-1] (commandline -ct) 2>/dev/null
end
complete -c meta -f -a '(__meta_complete)'
"#
        }
        _ => {
            r#"Register-ArgumentCompleter -Native -CommandName meta -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $words = @($commandAst.CommandElements | Select-Object -Skip 1 | ForEach-Object { $_.ToString() })
    if ($wordToComplete -eq '') { $words += '' }
    meta __complete -- @words 2>$null | ForEach-Object {
        [System.Management.Automation.CompletionResult]::new($_)
    }
}
"#
        }
    }
}

/// The completions of `words` (what follows `meta`, the last word being the
/// one completed), sorted, each once.
pub fn complete(index: &Index, words: &[String]) -> Vec<String> {
    let Some((partial, before)) = words.split_last() else {
        return top(index);
    };
    let meta = meta_plugin::args::MetaOptions::of(&crate::command_line(), before);
    let command = before.get(meta.typed.len()..).unwrap_or_default();
    let found = match command.is_empty() {
        // a value of one of meta's options: nothing to offer but files
        true if meta
            .typed
            .last()
            .is_some_and(|word| takes_value(index, word)) =>
        {
            Vec::new()
        }
        true if partial.starts_with('-') => meta_spellings(index),
        true => top(index),
        false => after_command(index, command, partial),
    };
    let mut found: Vec<String> = found
        .into_iter()
        .filter(|word| word.starts_with(partial.as_str()))
        .collect();
    found.sort();
    found.dedup();
    found
}

/// The first words of the commands there are, and the plugin files'.
fn top(index: &Index) -> Vec<String> {
    index::top_words(index)
        .into_iter()
        .map(str::to_string)
        .chain(index.files.iter().map(|file| file.word.clone()))
        .collect()
}

fn spellings(option: &Opt) -> Vec<String> {
    option
        .long
        .iter()
        .chain(&option.alias)
        .map(|long| format!("--{long}"))
        .chain(option.short.iter().map(|short| format!("-{short}")))
        .collect()
}

fn meta_spellings(index: &Index) -> Vec<String> {
    index.meta_options.iter().flat_map(spellings).collect()
}

/// Whether `word` is one of meta's options that takes a value.
fn takes_value(index: &Index, word: &str) -> bool {
    index
        .meta_options
        .iter()
        .any(|option| !option.is_flag() && spellings(option).iter().any(|s| s == word))
}

/// The completions after `command` (the words typed, from the command word
/// on).
fn after_command(index: &Index, command: &[String], partial: &str) -> Vec<String> {
    let Some((held, len)) = index::resolve(index, command) else {
        return Vec::new();
    };
    let args = command.get(len..).unwrap_or_default();
    let words = command.get(..len).unwrap_or_default();
    let next_words = next_words(index, words);
    if held.command.raw {
        return match args.is_empty() {
            true => next_words
                .into_iter()
                .chain(listed(held, &held.command.complete, words, partial))
                .collect(),
            false => Vec::new(),
        };
    }
    let options = &held.command.options;
    let value_of = args.last().and_then(|word| {
        options
            .iter()
            .find(|o| !o.is_flag() && spellings(o).iter().any(|s| s == word))
    });
    if let Some(option) = value_of {
        return values(held, option, words, partial);
    }
    if partial.starts_with('-') {
        return options.iter().flat_map(spellings).collect();
    }
    let positional = options
        .iter()
        .filter(|o| o.positional.is_some())
        .nth(args.iter().filter(|word| !word.starts_with('-')).count());
    next_words
        .into_iter()
        .chain(
            positional
                .into_iter()
                .flat_map(|option| values(held, option, words, partial)),
        )
        .collect()
}

/// The word after `words` of each command under them.
fn next_words(index: &Index, words: &[String]) -> Vec<String> {
    index::under(index, words)
        .into_iter()
        .filter(|held| !held.command.hidden)
        .filter_map(|held| held.name.split(' ').nth(words.len()).map(str::to_string))
        .collect()
}

fn values(held: &Indexed, option: &Opt, words: &[String], partial: &str) -> Vec<String> {
    match option.choices.is_empty() {
        false => option.choices.clone(),
        true => listed(held, &option.complete, words, partial),
    }
}

/// What `argv` lists: the first word of each line it prints. It gets a
/// context request (`META_REQUEST`) and the word being completed, runs from
/// the current directory for at most [`DEADLINE`], and failing gives nothing:
/// a completion that can't be listed is no error.
fn listed(held: &Indexed, argv: &[String], words: &[String], partial: &str) -> Vec<String> {
    let Some((program, rest)) = argv.split_first() else {
        return Vec::new();
    };
    let Some(dir) = tempfile::tempdir().ok() else {
        return Vec::new();
    };
    let Ok(request) = context_request(held, words, dir.path()) else {
        return Vec::new();
    };
    #[expect(
        clippy::disallowed_methods,
        reason = "a plugin's completer is its own program, not one in a repo"
    )]
    let mut command = Command::new(program);
    command
        .args(rest)
        .env(meta_plugin::REQUEST_VAR, request)
        .env(WORD_VAR, partial)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let Ok(mut child) = meta_plugin::process::spawn_group(&mut command) else {
        return Vec::new();
    };
    let Some(stdout) = child.stdout() else {
        child.stop(true);
        let _ = child.wait();
        return Vec::new();
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = stdout.take(MOST).read_to_end(&mut text);
        let _ = sender.send(text);
    });
    let deadline = Instant::now() + DEADLINE;
    let text = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    child.stop(true);
    let _ = child.wait();
    let text = text.unwrap_or_default();
    String::from_utf8_lossy(&text)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

/// A request naming the command and no projects, written in `dir`.
fn context_request(
    held: &Indexed,
    words: &[String],
    dir: &Path,
) -> anyhow::Result<std::path::PathBuf> {
    let cwd = std::env::current_dir()?;
    let plugin_dir = held
        .provider
        .path
        .as_deref()
        .and_then(Path::parent)
        .unwrap_or(&cwd)
        .to_path_buf();
    let mut request = meta_plugin::Request::new(words.join(" "), &cwd);
    request.data_dir = dir.to_path_buf();
    request.plugin_dir = plugin_dir;
    let path = dir.join("request.json");
    std::fs::write(&path, serde_json::to_vec(&request)?)?;
    Ok(path)
}
