//! One source of truth for every fact meta declares, as a ratchet.
//!
//! meta's main branch at f44f0a4 declared 25 groups of facts more than once
//! (a command list in routing, help, dispatch and docs; exit codes in code
//! and two docs; the version in five files). Each group below has a check:
//! the rule it must keep once it has one source, or, until a change gives
//! it one, the copies found then, which the check finds still there. The
//! test fails when an enabled group breaks its rule, and when a group not
//! yet enabled already passes: the change that made it pass enables it.
//!
//! Files are the ones git tracks. `SINGLE_SOURCE_WRITE=1` rewrites the
//! generated sections (`<!-- generated:NAME -->` … `<!-- /generated -->`)
//! from their sources instead of failing on them.

#![expect(
    clippy::disallowed_methods,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The groups whose facts have one source now.
const ENABLED: &[u8] = &[3, 4, 8, 11, 12, 17, 18, 19, 22, 24, 25];

/// How a group is checked.
enum Check {
    /// The rule the group keeps
    Rule(fn(&Repo) -> Result<(), String>),
    /// Copies found at f44f0a4: a file, and a line of the copy in it. The
    /// group passes once none is left; enabling it replaces these with its
    /// rule where one is needed.
    Copies(&'static [(&'static str, &'static str)]),
}

const GROUPS: [(u8, &str, Check); 16] = [
    (
        2,
        "git worktree's subcommands: declared once",
        Check::Copies(&[(
            "meta_git_cli/src/commands/worktree/cli_types.rs",
            "pub enum WorktreeCommands",
        )]),
    ),
    (
        3,
        "the commands that find their own repos: declared by their scope",
        Check::Rule(scope_once),
    ),
    (
        4,
        "meta project's commands: declared once",
        Check::Rule(project_once),
    ),
    (
        6,
        "help and usage: rendered from the declarations",
        Check::Copies(&[
            (
                "meta_rust_cli/src/main.rs",
                "usage: \"Usage: meta [OPTIONS] cargo <CARGO_ARGS>...\"",
            ),
            ("meta_cli/src/main.rs", "fn as_typed_help"),
            ("meta_cli/src/main.rs", "fn help_with_plugins"),
            ("meta_plugin/src/lib.rs", "pub fn help_text"),
        ]),
    ),
    (
        8,
        "meta's option spellings: never in a plugin",
        Check::Rule(no_option_spellings),
    ),
    (
        10,
        "a plugin command's options in prose: checked against its declaration",
        Check::Copies(&[("claude-plugin/skills/meta-git/SKILL.md", "--parallel")]),
    ),
    (11, "exit codes: meta_plugin::exit", Check::Rule(exit_codes)),
    (12, "--json output: one envelope", Check::Rule(one_envelope)),
    (
        17,
        "the installed binaries: listed once, by cargo",
        Check::Rule(programs_once),
    ),
    (
        18,
        "the Homebrew formula: release.yml's",
        Check::Rule(homebrew),
    ),
    (19, "the version: VERSION", Check::Rule(version)),
    (
        21,
        "worktree hook event names: their serde names",
        Check::Copies(&[(
            "meta_core/src/config.rs",
            "pub fn name(self) -> &'static str",
        )]),
    ),
    (
        22,
        "the plugin protocol's docs: the SDK's rustdoc",
        Check::Rule(protocol_docs),
    ),
    (
        23,
        "the shipped skills and rules: the skills folder and history",
        Check::Copies(&[("meta_cli/src/shipped.txt", "meta")]),
    ),
    (
        24,
        "agent and contributor notes: one file each",
        Check::Rule(notes),
    ),
    (
        25,
        "a request's fields: the SDK's rustdoc",
        Check::Rule(request_fields),
    ),
];

#[test]
fn every_fact_has_one_source_in_each_enabled_group() {
    let repo = Repo::open();
    let mut problems = Vec::new();
    for (n, what, check) in &GROUPS {
        let result = match check {
            Check::Rule(rule) => rule(&repo),
            Check::Copies(copies) => repo.copies_left(copies),
        };
        match (ENABLED.contains(n), result) {
            (true, Err(why)) => problems.push(format!("group {n} ({what}): {why}")),
            (false, Ok(())) => problems.push(format!(
                "group {n} ({what}) has one source now: add {n} to ENABLED"
            )),
            _ => {}
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The repo, as git tracks it.
struct Repo {
    root: PathBuf,
    files: Vec<String>,
}

impl Repo {
    fn open() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let out = Command::new("git")
            .args(["ls-files", "-z"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success(), "git ls-files: {out:?}");
        let files: Vec<String> = String::from_utf8(out.stdout)
            .unwrap()
            .split('\0')
            .filter(|f| !f.is_empty())
            .map(String::from)
            .collect();
        // a checkout the test can't read would pass every rule
        assert!(files.len() > 150, "only {} tracked files", files.len());
        Repo {
            root: root.to_path_buf(),
            files,
        }
    }

    fn tracked(&self, path: &str) -> bool {
        self.files.iter().any(|f| f == path)
    }

    /// The tracked text files and what they hold, but for the history
    /// (`CHANGELOG.md`) and the knowledge base's notes.
    fn texts(&self) -> impl Iterator<Item = (&str, String)> {
        self.files
            .iter()
            .filter(|f| *f != "CHANGELOG.md" && !f.starts_with(".kb/"))
            .filter_map(|f| Some((f.as_str(), std::fs::read_to_string(self.root.join(f)).ok()?)))
    }

    fn copies_left(&self, copies: &[(&str, &str)]) -> Result<(), String> {
        let left: Vec<String> = copies
            .iter()
            .filter(|(file, line)| {
                std::fs::read_to_string(self.root.join(file)).is_ok_and(|text| text.contains(line))
            })
            .map(|(file, line)| format!("{file} still has `{line}`"))
            .collect();
        match left.is_empty() {
            true => Ok(()),
            false => Err(left.join("; ")),
        }
    }

    /// Check (or with `SINGLE_SOURCE_WRITE=1`, rewrite) the section `name`
    /// of `file` against `render`.
    fn generated(&self, file: &str, name: &str, render: &str) -> Result<(), String> {
        let path = self.root.join(file);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{file}: {e}"))?;
        // as a Windows checkout has it, with CRLF line endings: the same text
        let text = text.replace("\r\n", "\n");
        let open = format!("<!-- generated:{name} -->\n");
        let close = "<!-- /generated -->";
        let start = text
            .find(&open)
            .map(|at| at + open.len())
            .ok_or_else(|| format!("{file} has no generated:{name} section"))?;
        let end = start
            + text[start..]
                .find(close)
                .ok_or_else(|| format!("{file}: generated:{name} is never closed"))?;
        if text[start..end] == *render {
            return Ok(());
        }
        if std::env::var_os("SINGLE_SOURCE_WRITE").is_some() {
            let written = format!("{}{render}{}", &text[..start], &text[end..]);
            std::fs::write(&path, written).unwrap();
            return Ok(());
        }
        Err(format!(
            "{file}: generated:{name} isn't its source's; SINGLE_SOURCE_WRITE=1 rewrites it as\n{render}"
        ))
    }
}

/// Exit codes are `meta_plugin::exit`'s: the docs' table is generated from
/// it, no other table lists them, and no code works out `128 + n` itself.
fn exit_codes(repo: &Repo) -> Result<(), String> {
    let mut problems = Vec::new();
    if let Err(e) = repo.generated(
        "docs/advanced_usage.md",
        "exit-codes",
        &meta_plugin::exit::table(),
    ) {
        problems.push(e);
    }
    let codes: Vec<String> = meta_plugin::exit::CODES
        .iter()
        .map(|(code, _)| format!("| {code} |"))
        .collect();
    for (file, text) in repo.texts() {
        if file.ends_with(".md") {
            let mut generated = false;
            for (at, line) in text.lines().enumerate() {
                generated = match line {
                    l if l.starts_with("<!-- generated:") => true,
                    l if l.starts_with("<!-- /generated") => false,
                    _ => generated,
                };
                if !generated && codes.iter().any(|row| line.starts_with(row.as_str())) {
                    problems.push(format!("{file}:{}: an exit code's table row", at + 1));
                }
            }
        }
        if file.ends_with(".rs") && file != "meta_plugin/src/exit.rs" && !file.contains("/tests/") {
            let code = text.split("#[cfg(test)]").next().unwrap_or_default();
            for (at, line) in code.lines().enumerate() {
                let line = line.trim_start();
                if !line.starts_with("//") && (line.contains("128 +") || line.contains("128+")) {
                    problems.push(format!("{file}:{}: 128 + n worked out again", at + 1));
                }
            }
        }
    }
    match problems.is_empty() {
        true => Ok(()),
        false => Err(problems.join("\n")),
    }
}

/// The release workflow writes the Homebrew formula; no copy is kept.
fn homebrew(repo: &Repo) -> Result<(), String> {
    match repo
        .files
        .iter()
        .find(|f| f.starts_with("distribution/homebrew/"))
    {
        Some(file) => Err(format!("{file} is a second formula")),
        None => Ok(()),
    }
}

/// meta's programs are the workspace's bin targets, which
/// `scripts/programs.py` reads from `cargo metadata`: the files that build,
/// package and install them name none of them. `meta` is the exception, as
/// the one program the installers require, and so is install.sh's
/// `OLD_PLUGIN_COPIES` line, the one list of the copies old make targets
/// left in the plugin folders (the Makefile reads it).
fn programs_once(repo: &Repo) -> Result<(), String> {
    let out = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(repo.root.join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let programs: Vec<String> = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|package| package["targets"].as_array().unwrap())
        .filter(|target| {
            target["kind"]
                .as_array()
                .unwrap()
                .iter()
                .any(|kind| kind == "bin")
        })
        .map(|target| target["name"].as_str().unwrap().to_string())
        .filter(|name| name != "meta")
        .collect();
    assert!(programs.len() >= 4, "only {programs:?}");
    let word =
        |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    let mut problems = Vec::new();
    for file in [
        "install.sh",
        "install.ps1",
        ".github/workflows/release.yml",
        "Makefile",
    ] {
        let text = std::fs::read_to_string(repo.root.join(file)).unwrap();
        for (at, line) in text.lines().enumerate() {
            if file == "install.sh" && line.starts_with("OLD_PLUGIN_COPIES=") {
                continue;
            }
            for program in &programs {
                let named = line.match_indices(program.as_str()).any(|(i, _)| {
                    !word(line[..i].chars().next_back())
                        && !word(line[i + program.len()..].chars().next())
                });
                if named {
                    problems.push(format!("{file}:{}: names {program}", at + 1));
                }
            }
        }
    }
    match problems.is_empty() {
        true => Ok(()),
        false => Err(problems.join("; ")),
    }
}

/// The version is `VERSION`'s, written only where release-please keeps it
/// in step: no other file names it, and the Claude plugin has none.
fn version(repo: &Repo) -> Result<(), String> {
    let version = std::fs::read_to_string(repo.root.join("VERSION")).unwrap();
    let version = version.trim();
    assert!(version.split('.').count() == 3, "VERSION holds {version:?}");
    let kept = [
        "VERSION",
        "Cargo.toml",
        "Cargo.lock",
        ".release-please-manifest.json",
    ];
    let mut problems: Vec<String> = repo
        .texts()
        .filter(|(file, _)| !kept.contains(file))
        .flat_map(|(file, text)| {
            text.lines()
                .enumerate()
                .filter(|(_, line)| {
                    line.match_indices(version).any(|(at, _)| {
                        let before = line[..at].chars().next_back();
                        let after = line[at + version.len()..].chars().next();
                        let part =
                            |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit() || c == '.');
                        !part(before) && !part(after)
                    })
                })
                .map(|(at, _)| format!("{file}:{}: names version {version}", at + 1))
                .collect::<Vec<_>>()
        })
        .collect();
    let plugin = "claude-plugin/.claude-plugin/plugin.json";
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo.root.join(plugin)).unwrap()).unwrap();
    if manifest.get("version").is_some() {
        problems.push(format!("{plugin} has a version of its own"));
    }
    match problems.is_empty() {
        true => Ok(()),
        false => Err(problems.join("\n")),
    }
}

/// One file of notes for agents (`CLAUDE.md`; `AGENTS.md` sends agents to
/// it) and one for contributors (`CONTRIBUTING.md`).
fn notes(repo: &Repo) -> Result<(), String> {
    let mut problems = Vec::new();
    let agents = std::fs::read_to_string(repo.root.join("AGENTS.md")).unwrap_or_default();
    if agents.trim() != "READ CLAUDE.md" {
        problems.push("AGENTS.md says more than READ CLAUDE.md".to_string());
    }
    for file in &repo.files {
        let name = file.rsplit('/').next().unwrap_or(file);
        if name.eq_ignore_ascii_case("contributing.md") && file != "CONTRIBUTING.md" {
            problems.push(format!("{file} is a second CONTRIBUTING"));
        }
    }
    if !repo.tracked("CLAUDE.md") || !repo.tracked("CONTRIBUTING.md") {
        problems.push("CLAUDE.md and CONTRIBUTING.md are the notes".to_string());
    }
    match problems.is_empty() {
        true => Ok(()),
        false => Err(problems.join("\n")),
    }
}

/// This file, which names what the rules look for.
const THIS: &str = "meta_cli/tests/single_source.rs";

/// Which commands find their repos themselves (and so take no
/// `--include`/`--exclude`/`--tag`) is decided once, by the host's scope
/// of each command: no plugin refuses the filters itself.
fn scope_once(repo: &Repo) -> Result<(), String> {
    let refusal = "it does not take --include/--exclude/--tag";
    let found: Vec<&str> = repo
        .texts()
        .filter(|(file, _)| *file != THIS)
        .filter(|(file, text)| file.ends_with(".rs") && text.contains(refusal))
        .map(|(file, _)| file)
        .collect();
    match found.as_slice() {
        ["meta_cli/src/main.rs"] => Ok(()),
        found => Err(format!(
            "the filter refusal is in {found:?}, not the host alone"
        )),
    }
}

/// meta project's commands are declared once, by the host's clap
/// subcommands: no plugin serves them, and no code lists or routes them by
/// hand or writes their help.
fn project_once(repo: &Repo) -> Result<(), String> {
    let copies = [r#""project", "list""#, "const SUBCOMMANDS", "PROJECT_HELP"];
    let mut problems: Vec<String> = repo
        .files
        .iter()
        .filter(|file| file.starts_with("meta_project_cli/"))
        .map(|file| format!("{file}: a plugin serves meta project"))
        .collect();
    for (file, text) in repo.texts() {
        // docs_lint.rs: a module of meta's built only for its tests
        if !file.ends_with(".rs")
            || file == THIS
            || file.contains("/tests/")
            || file == "meta_cli/src/docs_lint.rs"
        {
            continue;
        }
        let code = text.split("#[cfg(test)]").next().unwrap_or_default();
        for copy in copies.iter().filter(|copy| code.contains(**copy)) {
            problems.push(format!("{file} has `{copy}`"));
        }
    }
    match problems.is_empty() {
        true => Ok(()),
        false => Err(problems.join("\n")),
    }
}

/// Every `--json` output is the SDK's `Envelope`: no other JSON output type
/// is defined, and each JSON example the docs show after a `meta … --json`
/// line parses as one.
fn one_envelope(repo: &Repo) -> Result<(), String> {
    let mut problems = Vec::new();
    for (file, text) in repo.texts() {
        if file.ends_with(".rs") && file != THIS && text.contains("struct JsonOutput") {
            problems.push(format!("{file} defines a JSON output of its own"));
        }
    }
    let mut examples = 0;
    for (file, text) in repo.texts().filter(|(file, _)| file.ends_with(".md")) {
        let text = text.replace("\r\n", "\n");
        let lines: Vec<&str> = text.lines().collect();
        for (at, line) in lines.iter().enumerate() {
            if line.trim() != "```json" {
                continue;
            }
            // the example is meta's output when a line just above names a
            // `meta … --json` command
            let above = lines.get(at.saturating_sub(4)..at).unwrap_or_default();
            if !above
                .iter()
                .any(|l| l.contains("meta ") && l.contains("--json") || l.contains("envelope"))
            {
                continue;
            }
            let block: Vec<&str> = lines
                .iter()
                .skip(at + 1)
                .take_while(|l| l.trim() != "```")
                .copied()
                .collect();
            examples += 1;
            if let Err(e) = serde_json::from_str::<meta_plugin::Envelope>(&block.join("\n")) {
                problems.push(format!("{file}:{}: not an envelope: {e}", at + 1));
            }
        }
    }
    // a renamed folder can't make it pass: the docs show some
    if examples < 3 {
        problems.push(format!("only {examples} envelope examples in the docs"));
    }
    match problems.is_empty() {
        true => Ok(()),
        false => Err(problems.join("\n")),
    }
}

/// The protocol's documentation is the SDK's rustdoc: no Markdown file
/// restates how a call is made.
fn protocol_docs(repo: &Repo) -> Result<(), String> {
    let restated = ["--meta-plugin-exec", "PlanResponse", "pre_commands"];
    markdown_without(repo, &restated)
}

/// A request's fields are documented by `meta_plugin::Request`'s rustdoc
/// alone: no Markdown file lists them.
fn request_fields(repo: &Repo) -> Result<(), String> {
    let fields = [
        "include_filters",
        "tag_filters",
        "`listed_root`",
        "`data_dir`",
    ];
    markdown_without(repo, &fields)
}

fn markdown_without(repo: &Repo, words: &[&str]) -> Result<(), String> {
    let found: Vec<String> = repo
        .texts()
        // the old task notes are history, as the changelog is
        .filter(|(file, _)| file.ends_with(".md") && !file.starts_with(".context/"))
        .flat_map(|(file, text)| {
            words
                .iter()
                .filter(|word| text.contains(**word))
                .map(|word| format!("{file} has `{word}`"))
                .collect::<Vec<_>>()
        })
        .collect();
    match found.is_empty() {
        true => Ok(()),
        false => Err(found.join("\n")),
    }
}

/// No plugin holds a list of meta's option spellings: selection reaches a
/// plugin as its projects, the rest of meta's options as `request.meta`.
fn no_option_spellings(repo: &Repo) -> Result<(), String> {
    let found: Vec<&str> = repo
        .texts()
        .filter(|(file, _)| file.ends_with(".rs") && *file != THIS)
        .filter(|(file, _)| !file.starts_with("meta_cli/") && !file.starts_with("meta_plugin/"))
        .filter(|(_, text)| {
            let code = text.split("#[cfg(test)]").next().unwrap_or_default();
            code.contains(r#""--include", "--exclude""#) || code.contains("const FILTERS")
        })
        .map(|(file, _)| file)
        .collect();
    match found.is_empty() {
        true => Ok(()),
        false => Err(format!("meta's option spellings listed in {found:?}")),
    }
}
