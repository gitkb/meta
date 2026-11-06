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
    clippy::string_slice,
    clippy::unwrap_used,
    reason = "test helpers: a panic is a failed test, and tests start the programs they test"
)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The groups whose facts have one source now.
const ENABLED: &[u8] = &[3, 4, 5, 7, 8, 9, 10, 11, 12, 18, 19, 20, 21, 22, 23, 24, 25];

/// How a group is checked.
enum Check {
    /// The rule the group keeps
    Rule(fn(&Repo) -> Result<(), String>),
    /// Copies found at f44f0a4: a file, and a line of the copy in it. The
    /// group passes once none is left; enabling it replaces these with its
    /// rule where one is needed.
    Copies(&'static [(&'static str, &'static str)]),
}

const GROUPS: [(u8, &str, Check); 25] = [
    (
        1,
        "meta-git's commands: declared once, in its manifest",
        Check::Copies(&[("meta_git_cli/src/lib.rs", "help: Some(PluginHelp {")]),
    ),
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
        5,
        "meta's own commands: listed once, by the index",
        Check::Copies(&[("meta_cli/tests/docs.rs", "const OWNED")]),
    ),
    (
        6,
        "help and usage: rendered from the declarations",
        Check::Copies(&[
            (
                "meta_git_cli/src/lib.rs",
                "usage: \"Usage: meta git <COMMAND> [OPTIONS]\"",
            ),
            (
                "meta_rust_cli/src/main.rs",
                "usage: \"Usage: meta cargo <CARGO_ARGS>...\"",
            ),
            ("meta_cli/src/main.rs", "fn as_typed_help"),
            ("meta_cli/src/main.rs", "fn help_with_plugins"),
            ("meta_plugin/src/lib.rs", "pub fn help_text"),
        ]),
    ),
    (
        7,
        "meta's options: listed once (MetaOptions)",
        Check::Copies(&[
            ("meta_cli/src/main.rs", "const SHOWN"),
            ("meta_cli/src/main.rs", "pub struct Cli"),
            ("meta_cli/tests/docs.rs", "const VALUED"),
            ("meta_cli/tests/docs.rs", "const META_OPTIONS"),
        ]),
    ),
    (
        8,
        "meta's option spellings: never in a plugin",
        Check::Rule(no_option_spellings),
    ),
    (
        9,
        "loop's options: loop's own, not a copy in meta",
        Check::Copies(&[("meta_cli/src/main.rs", "add_aliases_to_global_looprc")]),
    ),
    (
        10,
        "a plugin command's options in prose: checked against its declaration",
        Check::Rule(options_in_prose),
    ),
    (11, "exit codes: meta_plugin::exit", Check::Rule(exit_codes)),
    (12, "--json output: one envelope", Check::Rule(one_envelope)),
    (
        13,
        "the build systems meta knows: one list",
        Check::Copies(&[("meta_core/src/query.rs", r#"("meson.build", "meson")"#)]),
    ),
    (
        14,
        "MCP tools' arguments: from the commands they run",
        Check::Copies(&[("meta_mcp/src/tools.rs", "fn has_any_tag")]),
    ),
    (
        15,
        "the MCP tool list: derived, never counted in prose",
        Check::Copies(&[("README.md", "26 tools")]),
    ),
    (
        16,
        "command examples: checked in every tracked file",
        Check::Copies(&[(".claude/agents/meta-worker.md", "project list")]),
    ),
    (
        17,
        "the installed binaries: listed once, by cargo",
        Check::Copies(&[("install.sh", "meta-git")]),
    ),
    (
        18,
        "the Homebrew formula: release.yml's",
        Check::Rule(homebrew),
    ),
    (19, "the version: VERSION", Check::Rule(version)),
    (
        20,
        ".meta.yaml's schema: one type",
        Check::Rule(config_schema),
    ),
    (
        21,
        "worktree hook event names: their serde names",
        Check::Rule(hook_events),
    ),
    (
        22,
        "the plugin protocol's docs: the SDK's rustdoc",
        Check::Rule(protocol_docs),
    ),
    (
        23,
        "the shipped skills: the skills folder",
        Check::Rule(skills_folder),
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
        if !file.ends_with(".rs") || file == THIS || file.contains("/tests/") {
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
        "`plugin_dir`",
        "`data_dir`",
        "\"plugin_dir\"",
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

/// The lines of a Markdown text outside its generated sections, numbered.
fn hand_written(text: &str) -> Vec<(usize, &str)> {
    let mut generated = false;
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            generated = match line {
                l if l.starts_with("<!-- generated:") => true,
                l if l.starts_with("<!-- /generated") => false,
                _ => generated,
            };
            !generated
        })
        .map(|(at, line)| (at + 1, line))
        .collect()
}

fn all_ok(problems: Vec<String>) -> Result<(), String> {
    match problems.is_empty() {
        true => Ok(()),
        false => Err(problems.join("\n")),
    }
}

/// `.meta.yaml` is read by one serde type, `MetaConfig` and the sections it
/// holds. The field tables are generated from it (the keys are serde's, and
/// a meta_core test holds their meanings to them), and no other table lists
/// a project's fields. A key meta doesn't know is an error, but in a project
/// entry, where it is another tool's.
fn config_schema(repo: &Repo) -> Result<(), String> {
    use meta_core::config;
    let table = config::fields_table();
    let mut problems = Vec::new();
    for file in ["README.md", "claude-plugin/skills/meta-workspace/SKILL.md"] {
        problems.extend(repo.generated(file, "config-fields", &table).err());
    }
    if table.contains("|  |") {
        problems.push(format!("a field has no meaning:\n{table}"));
    }
    let rows: Vec<String> = table
        .lines()
        .filter_map(|row| row.split("`projects.<name>.").nth(1)?.split('`').next())
        .map(|field| format!("| `{field}` |"))
        .collect();
    assert!(rows.len() > 1, "project fields: {table}");
    for (file, text) in repo.texts().filter(|(file, _)| file.ends_with(".md")) {
        for (at, line) in hand_written(&text) {
            if rows.iter().any(|row| line.starts_with(row.as_str())) {
                problems.push(format!("{file}:{at}: a config field's table row"));
            }
        }
    }
    for (file, text) in repo.texts() {
        if file.ends_with(".rs") && file != THIS && text.contains("struct ExtendedEntry") {
            problems.push(format!("{file} has a second type for project entries"));
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join(".meta.yaml");
    let load = |yaml: &str| {
        std::fs::write(&config, yaml).unwrap();
        config::Workspace::load(&config)
    };
    if let Err(e) = load("projects:\n  a:\n    repo: git@x:o/a.git\n    branch: main\n") {
        problems.push(format!("another tool's key in a project entry: {e:#}"));
    }
    if load("project: {}\n").is_ok() {
        problems.push("a misspelled top-level key was accepted".to_string());
    }
    all_ok(problems)
}

/// A hook event's name is its serde name: no code spells one as a string
/// (`.to_string()` gives it), and the docs name one only as an example or
/// in a section generated from the enum.
fn hook_events(repo: &Repo) -> Result<(), String> {
    let events = meta_core::config::hook_events();
    let names: Vec<&str> = events.split('`').skip(1).step_by(2).collect();
    assert!(names.len() > 1, "hook events: {events}");
    let mut problems = Vec::new();
    let skill = "claude-plugin/skills/meta-worktree/SKILL.md";
    let render = format!("{events}\n");
    problems.extend(repo.generated(skill, "hook-events", &render).err());
    for (file, text) in repo.texts() {
        if file.ends_with(".rs") && !file.contains("/tests/") {
            let code = text.split("#[cfg(test)]").next().unwrap_or_default();
            for name in names.iter().filter(|n| code.contains(&format!("\"{n}\""))) {
                problems.push(format!("{file} spells the event {name}"));
            }
        }
        if file.ends_with(".md") {
            for (at, line) in hand_written(&text) {
                if names.iter().filter(|n| line.contains(**n)).count() > 1 {
                    problems.push(format!("{file}:{at}: a list of hook events"));
                }
            }
        }
    }
    all_ok(problems)
}

/// The skills are the folders of `claude-plugin/skills/`: `meta init claude`
/// installs exactly those, the docs' table is generated from their
/// frontmatter, and nothing else lists them: no line names three (a test's
/// fixture may use two), no code in `init.rs` names one, no table has a row
/// for one. (`shipped-skills.txt`
/// is the versions meta ever wrote, not this list: `--update` needs it to
/// tell them from a user's file, and a unit test holds it to these folders.)
fn skills_folder(repo: &Repo) -> Result<(), String> {
    let mut skills: Vec<&str> = repo
        .files
        .iter()
        .filter_map(|f| {
            f.strip_prefix("claude-plugin/skills/")?
                .strip_suffix("/SKILL.md")
        })
        .collect();
    skills.sort_unstable();
    assert!(skills.len() > 1, "skills: {skills:?}");
    let mut problems = Vec::new();

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".meta.yaml"), "projects: {}\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_meta"))
        .args(["init", "claude"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "meta init claude: {out:?}");
    let mut installed: Vec<String> = std::fs::read_dir(dir.path().join(".meta/skills"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    installed.sort_unstable();
    if installed != skills {
        problems.push(format!(
            "meta installs {installed:?}, the folder has {skills:?}"
        ));
    }

    let mut rows = String::from("| Skill | What it teaches |\n|-------|-----------------|\n");
    for name in &skills {
        let file = format!("claude-plugin/skills/{name}/SKILL.md");
        let text = std::fs::read_to_string(repo.root.join(&file)).unwrap();
        let description = text.lines().find_map(|l| l.strip_prefix("description: "));
        match description {
            Some(description) => rows.push_str(&format!("| `{name}` | {description} |\n")),
            None => problems.push(format!("{file} has no description")),
        }
    }
    problems.extend(
        repo.generated("docs/claude_code_skills.md", "skills", &rows)
            .err(),
    );

    let init = "meta_cli/src/init.rs";
    for (file, text) in repo.texts().filter(|(file, _)| *file != THIS) {
        let lines = match file.ends_with(".md") {
            true => hand_written(&text),
            false => text
                .lines()
                .enumerate()
                .map(|(at, l)| (at + 1, l))
                .collect(),
        };
        for (at, line) in lines {
            if skills.iter().filter(|name| line.contains(**name)).count() > 2 {
                problems.push(format!("{file}:{at}: a list of skills"));
            }
            if skills
                .iter()
                .any(|name| line.starts_with(&format!("| `{name}`")))
            {
                problems.push(format!("{file}:{at}: a skill's table row"));
            }
        }
        if file == init {
            let code = text.split("#[cfg(test)]").next().unwrap_or_default();
            for name in skills.iter().filter(|n| code.contains(&format!("\"{n}\""))) {
                problems.push(format!("{init} names the skill {name}"));
            }
        }
    }
    all_ok(problems)
}

/// The long options a document gives a command are ones its declaration has:
/// a command line that names a declared command (not one that runs as
/// typed) names only that command's options, or meta's.
fn options_in_prose(repo: &Repo) -> Result<(), String> {
    let out = Command::new(env!("CARGO_BIN_EXE_meta"))
        .args(["--json", "help"])
        .output()
        .unwrap();
    let envelope: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let index = &envelope["results"][0]["data"];
    let longs = |options: &serde_json::Value| -> Vec<String> {
        let options = options.as_array().into_iter().flatten();
        options
            .filter_map(|o| Some(format!("--{}", o["long"].as_str()?)))
            .collect()
    };
    let meta = longs(&index["meta_options"]);
    let valued: Vec<String> = index["meta_options"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["value"].is_string())
        .flat_map(|o| {
            let long = o["long"].as_str().map(|long| format!("--{long}"));
            long.into_iter()
                .chain(o["short"].as_str().map(|short| format!("-{short}")))
        })
        .collect();
    // a command whose words go on to another program, as typed or after a
    // positional list, gives them no options of its own
    let as_typed = |c: &serde_json::Value| {
        let options = c["options"].as_array().into_iter().flatten();
        c["raw"] == true || options.into_iter().any(|o| o["rest"] == true)
    };
    let commands: Vec<(Vec<&str>, bool, Vec<String>)> = index["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let name = c["name"].as_str().unwrap().split(' ').collect();
            (name, as_typed(c), longs(&c["options"]))
        })
        .collect();
    let (mut problems, mut checked) = (Vec::new(), 0);
    // every command has these, from clap; the old task notes are history
    let always = ["--help".to_string(), "--version".to_string()];
    let documents = repo
        .texts()
        .filter(|(file, _)| file.ends_with(".md") && !file.starts_with(".context/"));
    for (file, text) in documents {
        let mut fenced = false;
        for (at, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
                continue;
            }
            let spans: Vec<&str> = match fenced {
                true => vec![line],
                false => line.split('`').skip(1).step_by(2).collect(),
            };
            for span in spans {
                let span = span.trim_start().trim_start_matches("$ ");
                let Some(rest) = span.strip_prefix("meta ") else {
                    continue;
                };
                // up to where the shell takes over
                let rest = rest.split(['|', ';', '#', '>', '&']).next().unwrap_or(rest);
                let mut words = rest.split_whitespace().peekable();
                while let Some(word) = words.next_if(|word| word.starts_with('-')) {
                    if valued.iter().any(|v| v == word) {
                        words.next();
                    }
                }
                let words: Vec<&str> = words.collect();
                let found = commands
                    .iter()
                    .filter(|(name, ..)| {
                        name.len() <= words.len() && name.iter().zip(&words).all(|(n, w)| n == w)
                    })
                    .max_by_key(|(name, ..)| name.len());
                let Some((name, false, options)) = found else {
                    continue;
                };
                checked += 1;
                let given = words.iter().skip(name.len()).take_while(|w| **w != "--");
                for word in given.filter(|word| word.starts_with("--")) {
                    let word = word.split('=').next().unwrap_or(word);
                    if !options
                        .iter()
                        .chain(&meta)
                        .chain(&always)
                        .any(|k| k == word)
                    {
                        let name = name.join(" ");
                        problems.push(format!("{file}:{}: meta {name} has no {word}", at + 1));
                    }
                }
            }
        }
    }
    // a renamed folder can't make it pass: the docs show many
    if checked < 30 {
        problems.push(format!("only {checked} command lines in the docs"));
    }
    match problems.is_empty() {
        true => Ok(()),
        false => Err(problems.join("\n")),
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
