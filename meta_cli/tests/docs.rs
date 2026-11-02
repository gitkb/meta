//! Every `meta …` command the README, docs, skills and rules show, and every
//! one a string in the crates' code names (help, examples, errors, hints), is
//! one the real binaries accept.
//!
//! Each runs as `meta --dry-run …` in a workspace of its own whose projects,
//! none of them cloned, have every name and tag the commands select, with
//! meta's build dir and the system's first on PATH and a plugin registry that
//! refuses connections. Exit code 2, meta's one usage code (an unknown
//! command, option or missing argument), or anything but 0 and 1 rejects it.
//! A command named in prose (`meta git clone`) may leave out its arguments:
//! `meta … --help` then has to succeed. A word the reader or the code fills
//! in (`<name>`, `{name}`) is given a value, unless it stands for the command
//! itself (`meta <word> ...`): then the words before it are checked as a
//! command name. One they may leave out (`[options]`, `...`, `*`) is left
//! out. `meta git WORD` has to name a git command, since meta passes it to
//! git unread.
//!
//! It runs the plugins built in the target dir, which a `cargo test -p meta`
//! alone doesn't rebuild: CI and `make test` build the workspace first.
//!
//! What this can't catch: text that says the wrong thing about a command
//! that exists (`exec` "runs sequentially"), arguments that reach a program
//! unread (`meta git pull --tag backend`, `meta exec -- …`), and `loop`
//! commands.

#![expect(
    clippy::disallowed_methods,
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use syn::visit::Visit;

/// A command found somewhere, as the words after `meta`.
struct Line {
    at: String,
    words: Vec<String>,
    /// Only its command name has to exist: it's named in prose, or the
    /// reader fills in the rest of it
    name_only: bool,
}

/// The documents whose commands are checked: a closed list.
fn documents(root: &Path) -> Vec<PathBuf> {
    let mut files = vec![
        root.join("README.md"),
        root.join("loop_cli/README.md"),
        root.join("claude-plugin/hooks/hooks.json"),
    ];
    for (dir, file) in [
        ("docs", None),
        ("claude-plugin/skills", Some("SKILL.md")),
        ("meta_cli/.claude/rules", None),
    ] {
        let mut found: Vec<PathBuf> = fs::read_dir(root.join(dir))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .map(|path| match file {
                Some(file) => path.join(file),
                None => path,
            })
            .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
            .collect();
        found.sort();
        files.append(&mut found);
    }
    files
}

/// The shell languages a fenced block's commands are written in.
const SHELL: &[&str] = &["", "bash", "sh", "shell", "console"];

/// Every `meta …` command of a Markdown text: each line of a shell block
/// that starts with one, and each inline code span that does.
fn markdown_lines(text: &str, at: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut fence: Option<&str> = None;
    let mut pending = String::new();
    for (n, line) in text.lines().enumerate() {
        let at = format!("{at}:{}", n + 1);
        let trimmed = line.trim();
        if let Some(lang) = trimmed.strip_prefix("```") {
            fence = match fence {
                Some(_) => None,
                None => Some(lang.trim()),
            };
            continue;
        }
        match fence {
            Some(lang) if SHELL.contains(&lang) => {
                let command = trimmed.strip_prefix("$ ").unwrap_or(trimmed);
                if let Some(continued) = command.strip_suffix('\\') {
                    pending.push_str(continued);
                    continue;
                }
                let command = std::mem::take(&mut pending) + command;
                if is_meta(&command) {
                    lines.extend(commands_of(&command, &at, false));
                }
            }
            Some(_) => {}
            None => lines.extend(spans(line, '`', &at)),
        }
    }
    lines
}

/// The commands of the spans of `text` between pairs of `quote`.
fn spans(text: &str, quote: char, at: &str) -> Vec<Line> {
    text.split(quote)
        .skip(1)
        .step_by(2)
        .filter(|span| is_meta(span))
        .flat_map(|span| commands_of(span, at, true))
        .collect()
}

/// Every `meta …` command a string in code names: its spans in backticks or
/// single quotes, each of its indented lines that starts with one (an
/// example in help), and the whole string when it is one line that starts
/// with one. A doc comment's spans only: its lines are prose.
fn string_lines(text: &str, at: &str, doc: bool) -> Vec<Line> {
    let mut lines = spans(text, '`', at);
    if doc {
        return lines;
    }
    lines.extend(spans(text, '\'', at));
    for line in text.lines() {
        let command = line.trim_start();
        if command.len() < line.len() && is_meta(command) {
            lines.extend(commands_of(command, at, false));
        }
    }
    if !text.contains('\n') && is_meta(text) {
        lines.extend(commands_of(text, at, false));
    }
    lines
}

fn is_meta(command: &str) -> bool {
    command == "meta" || command.starts_with("meta ")
}

/// meta's options that take a value (`meta --help`): the word after one isn't
/// the command.
const VALUED: &[&str] = &[
    "-c",
    "--config",
    "-e",
    "--exclude",
    "-i",
    "--include",
    "-t",
    "--tag",
    "--depth",
];

/// The `meta` commands of a shell line (`a && b` has two), each the words
/// after `meta` up to where the shell takes over. A word filled in
/// (`<name>`, `{name}`) becomes [`FILL`], or ends the command where the
/// command word goes; one that may be left out (`[options]`, `...`, `*`) is
/// left out.
fn commands_of(command: &str, at: &str, in_prose: bool) -> Vec<Line> {
    let Some(split) = shlex::split(command) else {
        return vec![Line {
            at: format!("{at}: `{command}` has unbalanced quotes"),
            words: vec!["--unbalanced-quotes".into()],
            name_only: false,
        }];
    };
    let mut lines = Vec::new();
    let mut words: Option<Vec<String>> = None;
    let mut cut = false;
    let mut end = |words: &mut Option<Vec<String>>, cut: &mut bool| {
        if let Some(words) = words.take() {
            lines.push(Line {
                at: at.to_string(),
                words,
                name_only: in_prose || *cut,
            });
        }
        *cut = false;
    };
    for word in split {
        if word.starts_with('#') {
            break;
        }
        if matches!(
            word.as_str(),
            "|" | "||" | "&&" | ";" | "&" | ">" | ">>" | "<" | "2>&1"
        ) || word.starts_with("2>")
        {
            end(&mut words, &mut cut);
            continue;
        }
        match &mut words {
            None if word == "meta" => words = Some(Vec::new()),
            None => {}
            Some(_) if cut => {}
            Some(found) => {
                let filled = (word.contains('<') && word.contains('>'))
                    || (word.contains('{') && word.contains('}'));
                if !filled {
                    if !((word.starts_with('[') && word.ends_with(']'))
                        || matches!(word.as_str(), "..." | "…" | "*"))
                    {
                        found.push(word);
                    }
                } else if at_command_word(found) {
                    cut = true;
                } else {
                    found.push(FILL.to_string());
                }
            }
        }
    }
    end(&mut words, &mut cut);
    lines
}

/// `words` with meta's `--dry-run` among the options before the command,
/// whatever the command's own arguments hold (`exec -- x --dry-run`).
fn previewed(words: &[String]) -> Vec<String> {
    let mut words = words.to_vec();
    let options = words.get(..command_word(&words)).unwrap_or_default();
    if !options.iter().any(|word| word == "--dry-run") {
        words.insert(0, "--dry-run".into());
    }
    words
}

/// Where the command word is in `words`: the first word after meta's
/// options and their values.
fn command_word(words: &[String]) -> usize {
    let mut expects_value = false;
    for (at, word) in words.iter().enumerate() {
        if expects_value {
            expects_value = false;
        } else if word.starts_with('-') {
            expects_value = VALUED.contains(&word.as_str());
        } else {
            return at;
        }
    }
    words.len()
}

/// Whether the next word after `words` is where a command word goes: the
/// first word after meta's options, or the one after a command that has
/// subcommands.
fn at_command_word(words: &[String]) -> bool {
    let after_command = words.last().is_some_and(|word| {
        ["git", "plugin", "project", "init", "help", "worktree"].contains(&word.as_str())
    });
    after_command
        || command_word(words) == words.len()
            && !words
                .last()
                .is_some_and(|word| VALUED.contains(&word.as_str()))
}

/// What a filled-in word becomes: a project and a tag of the sandbox, and a
/// plain word anywhere else.
const FILL: &str = "api";

/// The `meta …` commands named by the strings of every crate's code (tests
/// left out): literals, macro arguments and attributes (doc comments and
/// clap's help).
fn code_lines(root: &Path) -> Vec<Line> {
    let mut lines = Vec::new();
    // the workspace's members: a cloned repo the workspace excludes isn't its code
    let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let members = manifest
        .split_once("members = [")
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(members, _)| members)
        .unwrap();
    let crates: Vec<PathBuf> = members
        .split(',')
        .map(|member| member.trim().trim_matches('"'))
        .filter(|member| !member.is_empty())
        .map(|member| root.join(member))
        .collect();
    assert!(
        crates.iter().all(|dir| dir.join("src").is_dir()),
        "{crates:?}"
    );
    for dir in crates {
        rust_files(&dir.join("src"), &mut |file| {
            let at = file.strip_prefix(root).unwrap().display().to_string();
            let parsed = syn::parse_file(&fs::read_to_string(file).unwrap()).unwrap();
            Strings {
                at: &at,
                lines: &mut lines,
                in_doc: false,
            }
            .visit_file(&parsed);
        });
    }
    lines
}

fn rust_files(dir: &Path, each: &mut impl FnMut(&Path)) {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            rust_files(&path, each);
        } else if path.extension().is_some_and(|ext| ext == "rs")
            // test helpers that tests include
            && !path.file_stem().is_some_and(|stem| stem == "test_support" || stem == "tests")
        {
            each(&path);
        }
    }
}

struct Strings<'a> {
    at: &'a str,
    lines: &'a mut Vec<Line>,
    in_doc: bool,
}

impl Strings<'_> {
    fn tokens(&mut self, tokens: proc_macro2::TokenStream) {
        for token in tokens {
            match token {
                proc_macro2::TokenTree::Group(group) => self.tokens(group.stream()),
                proc_macro2::TokenTree::Literal(literal) => {
                    if let syn::Lit::Str(string) = syn::Lit::new(literal) {
                        self.string(&string);
                    }
                }
                _ => {}
            }
        }
    }

    fn string(&mut self, string: &syn::LitStr) {
        let at = format!("{}:{}", self.at, string.span().start().line);
        self.lines
            .extend(string_lines(&string.value(), &at, self.in_doc));
    }
}

/// Whether `attrs` mark test-only code.
fn is_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("test")
            || (attr.path().is_ident("cfg")
                && attr
                    .meta
                    .require_list()
                    .is_ok_and(|list| list.tokens.to_string() == "test"))
    })
}

impl<'ast> Visit<'ast> for Strings<'_> {
    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if !is_test(&item.attrs) {
            syn::visit::visit_item_mod(self, item);
        }
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        if !is_test(&item.attrs) {
            syn::visit::visit_item_fn(self, item);
        }
    }

    fn visit_lit_str(&mut self, string: &'ast syn::LitStr) {
        self.string(string);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        self.tokens(mac.tokens.clone());
    }

    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        match &attr.meta {
            syn::Meta::NameValue(value) => {
                self.in_doc = attr.path().is_ident("doc");
                self.visit_expr(&value.value);
                self.in_doc = false;
            }
            syn::Meta::List(list) => self.tokens(list.tokens.clone()),
            syn::Meta::Path(_) => {}
        }
    }
}

/// The values `lines` give meta's selection options, tags then names: the
/// sandbox has each, so no command is refused for naming one.
fn selected(lines: &[Line]) -> (Vec<String>, Vec<String>) {
    let (mut tags, mut names) = (vec![FILL.to_string()], vec![]);
    for line in lines {
        let mut words = line.words.iter();
        while let Some(word) = words.next() {
            let (option, value) = match word.split_once('=') {
                Some((option, value)) => (option, Some(value.to_string())),
                None => (word.as_str(), None),
            };
            let into = match option {
                "-t" | "--tag" => &mut tags,
                "-i" | "--include" | "-e" | "--exclude" => &mut names,
                _ => continue,
            };
            let Some(value) = value.or_else(|| words.next().cloned()) else {
                continue;
            };
            into.extend(
                value
                    .split(',')
                    .filter(|v| {
                        *v != "."
                            && !v.is_empty()
                            && v.chars()
                                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
                    })
                    .map(String::from),
            );
        }
    }
    for list in [&mut tags, &mut names] {
        list.sort();
        list.dedup();
    }
    (tags, names)
}

/// A workspace (a git repo) whose projects are `names` and two more, each
/// with every tag in `tags`, none of them cloned, and a home, config and
/// data dir of its own.
struct Sandbox {
    _temp: tempfile::TempDir,
    ws: PathBuf,
    bin: PathBuf,
    env: Vec<(&'static str, PathBuf)>,
    path: String,
}

impl Sandbox {
    fn new(bin: &Path, tags: &[String], names: &[String]) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let mut env = vec![
            ("HOME", base.join("home")),
            ("XDG_CONFIG_HOME", base.join("config")),
            ("XDG_DATA_HOME", base.join("data")),
            ("XDG_CACHE_HOME", base.join("cache")),
            ("META_DATA_DIR", base.join("meta")),
            ("TMPDIR", base.join("tmp")),
        ];
        for (_, dir) in &env {
            fs::create_dir_all(dir).unwrap();
        }
        env.push(("GIT_CONFIG_GLOBAL", base.join("gitconfig")));
        fs::write(base.join("gitconfig"), "").unwrap();
        // Nothing reaches a real plugin registry
        fs::write(
            base.join("meta/config.yaml"),
            "registries: [\"http://127.0.0.1:9\"]\n",
        )
        .unwrap();
        let ws = base.join("ws");
        fs::create_dir_all(&ws).unwrap();
        let mut all: Vec<&str> = ["web", FILL]
            .into_iter()
            .chain(names.iter().map(String::as_str))
            .collect();
        all.sort();
        all.dedup();
        let mut projects = String::from("projects:\n");
        for name in all {
            projects.push_str(&format!(
                "  {name}:\n    repo: https://example.invalid/{name}.git\n    tags: [{}]\n",
                tags.join(", ")
            ));
        }
        fs::write(ws.join(".meta.yaml"), projects).unwrap();
        let sandbox = Self {
            _temp: temp,
            ws,
            bin: bin.to_path_buf(),
            env,
            path: format!(
                "{0}:{0}/examples:/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin",
                bin.display()
            ),
        };
        let init = sandbox.command("git").args(["init", "-q"]).status();
        assert!(init.unwrap().success());
        sandbox
    }

    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(&self.ws)
            .env_clear()
            .envs(self.env.iter().map(|(key, value)| (key, value)))
            .env("PATH", &self.path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("EDITOR", "false")
            .env("GIT_EDITOR", "false")
            .stdin(Stdio::null());
        command
    }

    fn meta(&self, words: &[String]) -> Output {
        self.command(self.bin.join("meta"))
            .args(words)
            .output()
            .unwrap()
    }

    /// Why meta doesn't accept `line`, if it doesn't.
    fn rejects(&self, line: &Line) -> Option<String> {
        let mut words = previewed(&line.words);
        let out = self.meta(&words);
        let accepted = matches!(out.status.code(), Some(0 | 1))
            || (line.name_only && out.status.code() == Some(2) && {
                // `meta exec -- <command>` names exec; `meta --tag` an option
                if words.last().is_some_and(|word| word == "--") {
                    words.pop();
                }
                if words
                    .last()
                    .is_some_and(|word| VALUED.contains(&word.as_str()))
                {
                    words.push(FILL.into());
                }
                words.push("--help".into());
                self.meta(&words).status.success()
            });
        if !accepted {
            let said = if out.stderr.is_empty() {
                &out.stdout
            } else {
                &out.stderr
            };
            let said = String::from_utf8_lossy(said);
            let said = said.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            let said: String = said.chars().take(160).collect();
            return Some(format!("{:?}: {said}", out.status.code()));
        }
        // `meta git WORD` passes WORD to git: it has to be one of git's
        let git = line.words.iter().position(|word| word == "git")?;
        let word = line.words.get(git + 1)?;
        if word.starts_with('-') || ADAPTED.contains(&word.as_str()) {
            return None;
        }
        let out = self.command("git").args([word, "-h"]).output().unwrap();
        let known = matches!(out.status.code(), Some(0 | 129))
            && !String::from_utf8_lossy(&out.stderr).contains("is not a git command");
        (!known).then(|| format!("'{word}' is not a git command"))
    }
}

/// The git commands meta's git plugin answers itself; any other word after
/// `meta git` goes to git.
const ADAPTED: &[&str] = &["clone", "commit", "update", "worktree"];

#[test]
fn every_command_the_docs_and_the_code_name_is_one_meta_accepts() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let bin = Path::new(env!("CARGO_BIN_EXE_meta")).parent().unwrap();
    // the plugin guide's example plugin serves `meta hello`
    for plugin in [
        "meta-git",
        "meta-project",
        "meta-cargo",
        "examples/meta-hello",
    ] {
        assert!(
            bin.join(plugin).is_file(),
            "{plugin} is not in {}: build the workspace first (cargo build --workspace --bins --examples)",
            bin.display()
        );
    }

    let mut lines = Vec::new();
    for file in documents(root) {
        let at = file.strip_prefix(root).unwrap().display().to_string();
        lines.extend(markdown_lines(&fs::read_to_string(&file).unwrap(), &at));
    }
    let documented = lines.len();
    lines.extend(code_lines(root));
    // Both found what they look for: a moved folder would leave nothing
    assert!(documented > 200, "only {documented} commands in the docs");
    assert!(
        lines.len() - documented > 50,
        "only {} commands in the code",
        lines.len() - documented
    );

    let (tags, names) = selected(&lines);
    let sandbox = Sandbox::new(bin, &tags, &names);
    let rejected: Vec<String> = lines
        .iter()
        .filter_map(|line| {
            sandbox
                .rejects(line)
                .map(|why| format!("{}: meta {} → {why}", line.at, line.words.join(" ")))
        })
        .collect();
    assert!(
        rejected.is_empty(),
        "{} of {} commands aren't ones meta accepts:\n{}",
        rejected.len(),
        lines.len(),
        rejected.join("\n")
    );
}

#[test]
fn a_command_ends_where_the_shell_takes_over_and_its_blanks_are_filled_in() {
    let found = |lines: Vec<Line>| -> Vec<(Vec<String>, bool)> {
        lines
            .into_iter()
            .map(|line| (line.words, line.name_only))
            .collect()
    };
    let md = |text: &str| found(markdown_lines(text, "x"));
    let w = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    assert_eq!(
        md("```bash\n$ meta project list | wc -l  # count\nmeta exec -- \\\n  ls\n```\n"),
        vec![
            (w(&["project", "list"]), false),
            (w(&["exec", "--", "ls"]), false)
        ]
    );
    assert_eq!(
        md("```\nmeta git add -A && meta git commit -m wip > log\n```"),
        vec![
            (w(&["git", "add", "-A"]), false),
            (w(&["git", "commit", "-m", "wip"]), false)
        ]
    );
    assert_eq!(
        md("Run `meta plugin install <name> [--local]` or `meta git status`.\n```yaml\nmeta: x\n```\n"),
        vec![
            (w(&["plugin", "install", FILL]), true),
            (w(&["git", "status"]), true)
        ]
    );
    // meta's --dry-run goes before the command, whatever the command takes
    let w2 = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    assert_eq!(
        previewed(&w2(&["--tag", "x", "exec", "--", "rm", "--dry-run"])),
        w2(&["--dry-run", "--tag", "x", "exec", "--", "rm", "--dry-run"])
    );
    assert_eq!(
        previewed(&w2(&["--dry-run", "git", "update"])),
        w2(&["--dry-run", "git", "update"])
    );
    // a blank where the command goes leaves a command name
    assert_eq!(
        md("```\nmeta --tag x <command>\nmeta git <subcommand> --help\n```"),
        vec![(w(&["--tag", "x"]), true), (w(&["git"]), true)]
    );
    assert_eq!(
        found(string_lines(
            "Use 'meta git worktree remove {name}' first.\n  meta exec -- ls\nmeta runs it",
            "x",
            false
        )),
        vec![
            (w(&["git", "worktree", "remove", FILL]), true),
            (w(&["exec", "--", "ls"]), false)
        ]
    );
    assert_eq!(
        found(string_lines("meta git status", "x", false)),
        vec![(w(&["git", "status"]), false)]
    );
    // a command that doesn't exist is found, to be rejected
    assert_eq!(
        found(string_lines("meta worktree remove fix", "x", false)),
        vec![(w(&["worktree", "remove", "fix"]), false)]
    );
    assert_eq!(
        found(string_lines(" meta runs `meta exec`", "x", true)),
        vec![(w(&["exec"]), true)]
    );
}
