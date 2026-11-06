//! The SDK for meta plugins: the plugin protocol, and the runtime every
//! plugin (and meta itself) shares.
//!
//! - [`exit`], [`out`] and [`display`]: how a program ends (its exit codes,
//!   one error reported once), what it prints (a closed stdout ends it with
//!   141, colour as meta's), and escaping text that came from a repo or a
//!   user (`escape`, `shell_quote`, `display_command`).
//! - [`RepoUrl`]: a `.meta.yaml` `repo:` URL.
//! - [`process`] (feature `process`, on by default): the one owner of the
//!   processes a program starts, and what happens to them when it is told to
//!   stop.
//! - [`request`], [`response`] and [`envelope`]: the wire, one type each:
//!   what the host sends, what a plugin answers, what `meta --json` prints.
//! - [`run_plugin`] and [`Ctx`]: a Rust plugin's side of a call.
//! - [`args`] and [`parse_args`] (feature `parse`): a plugin's own clap
//!   parsing, with the tip for one of meta's options typed after the command.
//! - `manifest` (feature `manifest`), `parser` (feature `parser`) and
//!   `build` (feature `build`): what a plugin declares in `meta-plugin.toml`,
//!   those options as a command line, and Rust types generated from them.
//! - `testing` (feature `testing`; git fixtures with `testing-git`): a
//!   sandbox with its own home and config, a workspace builder, and requests
//!   to send a plugin through the real transport.
//!
//! # The protocol
//!
//! A plugin is an executable named `meta-<word>` (`meta-<word>.exe` on
//! Windows), and it serves `meta <word> ...`: its file name, not anything it
//! answers, decides which command reaches it. To serve two top-level words,
//! ship two files. The host looks in `.meta/plugins/` of the workspace's meta
//! directories (nearest first), then `~/.meta/plugins/`, then PATH; the first
//! file with the name wins.
//!
//! The protocol works as follows:
//! 1. For `meta <word> ...` the host starts only `meta-<word>`, first with
//!    `--meta-plugin-info` (plugin responds with `PluginInfo` JSON within
//!    5 seconds). `meta`, `meta help`, `meta --help` and `meta plugin list`
//!    ask every plugin.
//! 2. The host resolves the invocation (the workspace, the selection, the
//!    set it runs in) and starts the plugin with `--meta-plugin-exec`, the
//!    [`Request`] in the file `META_REQUEST` names.
//! 3. The plugin writes one [`Response`] (a [`Plan`] for the host to run,
//!    [`Results`], a message, a delegate or an error, each with its
//!    warnings and skipped projects) to the file `META_RESPONSE` names;
//!    [`run_plugin`] does both for a Rust plugin. The plugin's stdin,
//!    stdout and stderr are meta's, but under `--json` its stdout is meta's
//!    stderr.
//! 4. The host prints the outcome: under `--json`, one [`Envelope`] for
//!    every command, which plugins never print themselves.
//!
//! A plugin ends with one of meta's exit codes ([`exit::CODES`]); the
//! response decides meta's, and without one, meta ends with the plugin's.
//!
//! Every program meta starts, a plugin included, gets `META_OWNER_DEPTH`: 1
//! for what meta starts, one more for each program in between. On Unix a
//! program that owns its children's signals ([`process::own`]) gives the
//! ones it starts half the time to stop that its own owner gives it (2
//! seconds in meta, 1 in a plugin, at least 250 ms), so a plugin has stopped
//! what it started before meta stops the plugin. [`process`] sets the
//! variable for every program it starts (its own depth plus one); a program
//! that starts others some other way sets it the same way.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[cfg(feature = "parse")]
pub mod args;
#[cfg(feature = "build")]
pub mod build;
#[cfg(feature = "client")]
pub mod client;
pub mod display;
pub mod duration;
pub mod envelope;
pub mod event;
pub mod exit;
#[cfg(feature = "manifest")]
pub mod manifest;
mod mode;
pub mod out;
#[cfg(feature = "parser")]
pub mod parser;
pub mod path;
#[cfg(feature = "process")]
pub mod process;
mod repo_url;
pub mod request;
pub mod response;
mod serve;
#[cfg(feature = "sets")]
pub mod sets;
#[cfg(feature = "testing")]
pub mod testing;

pub mod wire_path;

pub use envelope::{Envelope, ErrorCode, ErrorInfo, ProjectResult, Skipped, Warning};
pub use event::Event;
pub use mode::ExecutionMode;
pub use repo_url::RepoUrl;
pub use request::{Meta, Project, Request, Scope, Set, Workspace, Worktree};
pub use response::{Answer, Plan, Response, Results, Step, Text};
pub use serve::{run_plugin, usage, Ctx, PluginDefinition, REQUEST_VAR, RESPONSE_VAR};
pub use wire_path::Word;

/// The types [`build::generate`] wrote for this crate's manifest.
#[macro_export]
macro_rules! include_commands {
    () => {
        include!(concat!(env!("OUT_DIR"), "/commands.rs"));
    };
}

/// The variable that names the lease a program's caller holds (see
/// `sets::lease`): meta reads it and passes it on as `Request::lease`.
pub const LEASE_VAR: &str = "META_LEASE";

/// The variable a caller would name an exact selection in. Nothing reads it
/// yet; meta strips it, with [`LEASE_VAR`], from every program it starts.
pub const SELECTION_VAR: &str = "META_SELECTION";

/// What a plugin's handler returns: an error carries its [`ErrorCode`] in
/// an [`ErrorInfo`] (any other error is `failed`).
pub type Result<T, E = anyhow::Error> = std::result::Result<T, E>;

// ============================================================================
// Plugin Discovery Types
// ============================================================================

/// Metadata about a plugin, returned in response to `--meta-plugin-info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    /// The plugin's name for itself. The host names a plugin by its file's
    /// word instead, and routes by that.
    pub name: String,
    pub version: String,
    /// Commands the plugin handles, as words starting with its file's word
    /// (`"git worktree create"` for `meta-git`). For `meta <word> ...` the
    /// longest one that matches the typed words whole becomes the request's
    /// `command`, and the rest its `args`; with none matching, `command` is
    /// the word alone. A bare `meta <word>` prints the plugin's help. A
    /// command under another word never reaches the plugin.
    pub commands: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub help: Option<PluginHelp>,
}

/// Help information for a plugin's commands.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PluginHelp {
    /// Usage string (e.g., "meta git <command> [args...]")
    pub usage: String,
    /// The plugin's own commands (for meta-git, the git commands it
    /// adapts), each with a one-line description, in the order help shows
    /// them. Every help surface reads them from here: `meta --help`,
    /// `meta <word> --help` and `meta <word> <command> --help`.
    #[serde(default)]
    pub commands: IndexMap<String, String>,
    /// The tool that every other command runs as typed, in each selected
    /// repo: with `git`, `meta git log -3` runs `git log -3` there.
    #[serde(default)]
    pub runs_as_typed: Option<String>,
    /// Example usage strings
    #[serde(default)]
    pub examples: Vec<String>,
    /// Additional note (e.g., how to run raw commands)
    #[serde(default)]
    pub note: Option<String>,
}

/// Who answers `meta <word> <verb>`, as a plugin's help declares it.
#[derive(Debug, PartialEq)]
pub enum Verb<'a> {
    /// One of the plugin's own commands
    Own,
    /// Not one of them: `<tool> <verb> ...` runs as typed in each repo
    AsTyped(&'a str),
    /// Neither: the plugin decides
    Unknown,
}

impl PluginInfo {
    /// Who answers `meta <word> <verb>` (`verb`: the word after the
    /// plugin's word).
    pub fn verb(&self, verb: &str) -> Verb<'_> {
        let Some(help) = &self.help else {
            return Verb::Unknown;
        };
        match &help.runs_as_typed {
            _ if help.commands.contains_key(verb) => Verb::Own,
            Some(tool) => Verb::AsTyped(tool),
            None => Verb::Unknown,
        }
    }
}

impl PluginHelp {
    /// What becomes of the commands that aren't the plugin's own, if it
    /// runs them as typed: "any other git command runs as typed in each
    /// selected repo". Not escaped.
    pub fn as_typed_rule(&self) -> Option<String> {
        let tool = self.runs_as_typed.as_ref()?;
        let which = if self.commands.is_empty() {
            "every"
        } else {
            "any other"
        };
        Some(format!(
            "{which} {tool} command runs as typed in each selected repo"
        ))
    }
}

/// The help of the plugin serving `meta <word>`, from its info. Every value
/// comes from the plugin, so each is escaped onto its own line.
pub fn help_text(word: &str, info: &PluginInfo) -> String {
    use crate::display::escape;
    let mut out = String::new();
    let mut line = |text: &str| {
        out.push_str(text);
        out.push('\n');
    };
    let Some(help) = &info.help else {
        line(&format!("meta {} v{}", escape(word), escape(&info.version)));
        if let Some(desc) = &info.description {
            line(&escape(desc));
        }
        if !info.commands.is_empty() {
            line("");
            line("Commands:");
            for cmd in &info.commands {
                line(&format!("  {}", escape(cmd)));
            }
        }
        return out;
    };
    line(&escape(&help.usage));
    line("");
    if !help.commands.is_empty() {
        match help.runs_as_typed {
            Some(_) => line(&format!("Commands meta-{} adapts:", escape(word))),
            None => line("Commands:"),
        }
        for (cmd, desc) in &help.commands {
            line(&format!("  {:<20} {}", escape(cmd), escape(desc)));
        }
        line("");
    }
    if let (Some(tool), Some(rule)) = (&help.runs_as_typed, help.as_typed_rule()) {
        let (tool, word) = (escape(tool), escape(word));
        line(&format!(
            "Pass-through: {}, {tool}'s own extensions on PATH included \
             ({tool}-<name>: meta {word} <name>).",
            escape(&rule)
        ));
        line("");
    }
    if !help.examples.is_empty() {
        line("Examples:");
        for ex in &help.examples {
            line(&format!("  {}", escape(ex)));
        }
        line("");
    }
    if let Some(note) = &help.note {
        line(&escape(note));
    }
    out
}

/// Whether `--help`/`-h` comes before `--` among the command's words and
/// its arguments; after `--`, they are arguments for the command.
pub fn has_help_token(command: &str, args: &[String]) -> bool {
    command
        .split_whitespace()
        .chain(
            args.iter()
                .map(String::as_str)
                .take_while(|arg| *arg != "--"),
        )
        .any(|arg| arg == "--help" || arg == "-h")
}

/// Parse the arguments of `meta <command>` with `P`, or the response to give
/// instead: its help (or version) as a message, its error, escaped, as a
/// usage error. An option of meta's typed
/// after the command that `P` rejects gets the tip with the line to run
/// (`meta` sends its options in the request: `&request.meta_options`;
/// [`crate::args::MetaOptions::refusing`] the ones the command never takes).
#[cfg(feature = "parse")]
pub fn parse_args<P: clap::Parser>(
    command: &str,
    args: &[String],
    meta: &crate::args::MetaOptions,
) -> Result<P, Box<Response>> {
    let name = format!("meta {command}");
    let argv: Vec<String> = std::iter::once(name.clone())
        .chain(args.iter().cloned())
        .collect();
    P::try_parse_from(&argv).map_err(|e| {
        use clap::error::{ContextKind, ContextValue, ErrorKind};
        match e.kind() {
            ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => Box::new(Response::message(
                e.render().to_string().trim_end().to_string(),
            )),
            _ => {
                let mut escaped = crate::args::escaped_error::<P>(&argv);
                let typed: Vec<String> = command
                    .split_whitespace()
                    .map(str::to_string)
                    .chain(args.iter().cloned())
                    .collect();
                let words = command.split_whitespace().count();
                let check = |_: &[String], rest: &[String]| {
                    let args = rest.iter().skip(words).map(String::as_str);
                    crate::args::verdict(P::try_parse_from(
                        std::iter::once(name.as_str()).chain(args),
                    ))
                };
                let tip = meta.rejected(&e, &typed, &check);
                if let Some(tip) = &tip {
                    escaped.insert(
                        ContextKind::Suggested,
                        ContextValue::StyledStrs(vec![tip.clone().into()]),
                    );
                }
                let mut message = crate::args::message(&escaped);
                // a command that isn't one: the ones there are
                if e.kind() == ErrorKind::InvalidSubcommand {
                    let help = P::command().bin_name(&name).render_help().to_string();
                    let error = message.split("\n\nUsage:").next().unwrap_or_default();
                    message = format!("{error}\n\n{}", help.trim_end());
                }
                let mut error = ErrorInfo::new(ErrorCode::Usage, message);
                error.hint = tip;
                Box::new(Response::error(error))
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    fn parse<T: serde::de::DeserializeOwned>(json: &str) -> serde_json::Result<T> {
        serde_json::from_str(json)
    }

    /// `value` as JSON and back
    fn round_trip<T: Serialize + serde::de::DeserializeOwned>(value: &T) -> T {
        serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
    }

    #[test]
    fn a_request_arrives_as_the_host_resolved_it() {
        let project = Project {
            name: "api".to_string(),
            path: "platform/api".to_string(),
            abs: PathBuf::from("/w/platform/api"),
            repo: Some("git@x:api.git".to_string()),
            tags: vec!["rust".to_string()],
            depends_on: vec!["core".to_string()],
            dependency_problems: vec![],
            meta: false,
            level: None,
            present: true,
            nested: vec![],
        };
        let mut sent = Request::new("git status", "/w/platform");
        sent.args = vec!["--short".to_string(), "it's \"q\" $HOME".to_string()];
        sent.scope = Scope::Selection;
        sent.workspace = Some(Workspace {
            root: PathBuf::from("/w"),
            config: PathBuf::from("/w/other.yaml"),
            listed_root: PathBuf::from("/w"),
            members: vec![project.clone()],
            worktree: Worktree {
                dir: Some(PathBuf::from("/w/.worktrees")),
                hooks: [("post-create".to_string(), "make".to_string())].into(),
                home: None,
                refused: None,
            },
        });
        sent.projects = vec![project.clone()];
        sent.meta.strict = true;
        let got = round_trip(&sent);
        assert_eq!(got.projects, [project]);
        assert_eq!(got.args, sent.args);
        let workspace = got.workspace.unwrap();
        assert_eq!(workspace.config, PathBuf::from("/w/other.yaml"));
        assert_eq!(workspace.worktree.hooks["post-create"], "make");
        assert!(got.meta.strict && !got.meta.dry_run);
        // a key the protocol doesn't have is refused, not ignored
        let mut wire = serde_json::to_value(&sent).unwrap();
        wire["meta"]["recursive"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Request>(wire).is_err());
    }

    #[test]
    fn an_empty_selection_is_sent_as_no_projects() {
        let mut sent = Request::new("git status", "/w");
        sent.scope = Scope::Selection;
        let wire = serde_json::to_value(&sent).unwrap();
        assert_eq!(wire["projects"], serde_json::json!([]));
        assert!(parse::<Request>(&wire.to_string())
            .unwrap()
            .projects
            .is_empty());
    }

    #[test]
    fn plugin_info_and_help_default_what_is_left_out() {
        let info: PluginInfo =
            parse(r#"{"name":"git","version":"1.0","commands":["git status"],"help":{"usage":"meta git"}}"#)
                .unwrap();
        assert_eq!((info.name.as_str(), info.version.as_str()), ("git", "1.0"));
        assert_eq!(info.commands, ["git status"]);
        assert!(info.description.is_none());
        let help = info.help.unwrap();
        assert_eq!(help.usage, "meta git");
        assert!(help.commands.is_empty() && help.runs_as_typed.is_none());
        assert!(help.examples.is_empty() && help.note.is_none());
        let written = serde_json::json!({
            "name": "git", "version": "1.0", "commands": ["git status"],
            "description": "Git across repos",
            "help": {
                "usage": "meta git <command>",
                "commands": {"status": "Show status"},
                "runs_as_typed": "git",
                "examples": ["meta git status"],
                "note": "Raw git: meta exec -- git",
            },
        });
        let info: PluginInfo = serde_json::from_value(written.clone()).unwrap();
        let help = info.help.as_ref().unwrap();
        assert_eq!(info.description.as_deref(), Some("Git across repos"));
        assert_eq!(help.commands["status"], "Show status");
        assert_eq!(help.runs_as_typed.as_deref(), Some("git"));
        assert_eq!((help.examples.len(), help.note.is_some()), (1, true));
        assert_eq!(serde_json::to_value(&info).unwrap(), written);
        // the order of the help's commands is kept
        let mut commands = IndexMap::new();
        for name in ["z", "a", "m"] {
            commands.insert(name.to_string(), format!("does {name}"));
        }
        let help = round_trip(&PluginHelp {
            usage: "u".to_string(),
            commands,
            runs_as_typed: None,
            examples: vec![],
            note: None,
        });
        assert_eq!(help.commands.keys().collect::<Vec<_>>(), ["z", "a", "m"]);
    }

    #[test]
    fn help_keeps_each_plugin_value_on_its_line() {
        let forged = "x\u{1b}[2J\nFORGED".to_string();
        let mut commands = IndexMap::new();
        commands.insert(forged.clone(), forged.clone());
        let mut info = PluginInfo {
            name: forged.clone(),
            version: forged.clone(),
            commands: vec![forged.clone()],
            description: Some(forged.clone()),
            help: None,
        };
        let plain = help_text(&forged, &info);
        info.help = Some(PluginHelp {
            usage: forged.clone(),
            commands,
            runs_as_typed: Some(forged.clone()),
            examples: vec![forged.clone()],
            note: Some(forged.clone()),
        });
        for text in [plain, help_text(&forged, &info)] {
            assert!(!text.contains('\u{1b}'), "{text}");
            assert!(!text.lines().any(|l| l.starts_with("FORGED")), "{text}");
            assert!(text.contains("x\\u{1b}[2J\\nFORGED"), "{text}");
        }
    }

    #[test]
    fn a_verb_is_the_plugins_own_or_runs_as_typed_as_its_help_says() {
        let mut commands = IndexMap::new();
        commands.insert("clone".to_string(), "Clone a workspace".to_string());
        let mut info = PluginInfo {
            name: "git".to_string(),
            version: "1".to_string(),
            commands: vec!["git clone".to_string()],
            description: None,
            help: Some(PluginHelp {
                usage: "meta git".to_string(),
                commands,
                runs_as_typed: Some("git".to_string()),
                ..PluginHelp::default()
            }),
        };
        assert_eq!(info.verb("clone"), Verb::Own);
        assert_eq!(info.verb("log"), Verb::AsTyped("git"));
        let text = help_text("git", &info);
        assert!(
            text.contains("Commands meta-git adapts:\n  clone"),
            "{text}"
        );
        assert!(
            text.contains("any other git command runs as typed in each selected repo"),
            "{text}"
        );
        // a plugin that runs nothing as typed decides the rest itself
        if let Some(help) = info.help.as_mut() {
            help.runs_as_typed = None;
        }
        assert_eq!(info.verb("log"), Verb::Unknown);
        assert!(!help_text("git", &info).contains("as typed"));
    }
}
