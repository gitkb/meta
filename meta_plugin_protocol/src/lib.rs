//! Shared plugin protocol types for meta subprocess plugins.
//!
//! This crate defines the communication protocol between the meta CLI host
//! and its subprocess plugins (meta-git, meta-project, meta-cargo, etc.).
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
//! 2. Host invokes the plugin via `--meta-plugin-exec` (sends `PluginRequest`
//!    JSON on stdin)
//! 3. Plugin responds with either a `PlanResponse` JSON (commands to execute) or direct output

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginHelp {
    /// Usage string (e.g., "meta git <command> [args...]")
    pub usage: String,
    /// Command descriptions (command name -> description)
    /// Uses IndexMap to preserve insertion order for consistent help output
    #[serde(default)]
    pub commands: IndexMap<String, String>,
    /// Categorized commands (section title -> commands map)
    /// When present and non-empty, commands are displayed by section instead of flat list
    #[serde(default)]
    pub command_sections: IndexMap<String, IndexMap<String, String>>,
    /// Example usage strings
    #[serde(default)]
    pub examples: Vec<String>,
    /// Additional note (e.g., how to run raw commands)
    #[serde(default)]
    pub note: Option<String>,
}

// ============================================================================
// Host-to-Plugin Communication
// ============================================================================

/// A request from the meta CLI host to a plugin, sent as JSON on stdin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRequest {
    /// The plugin command the user named, e.g. `git status`: the claimed
    /// words only. Everything the user typed after them is in `args`.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub projects: Vec<String>,
    /// Where the command runs; a plugin given none uses its own working
    /// directory ([`run_plugin`] fills it in).
    #[serde(default)]
    pub cwd: PathBuf,
    #[serde(default)]
    pub options: PluginRequestOptions,
}

/// Options passed from the host to the plugin as part of the request.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PluginRequestOptions {
    #[serde(default)]
    pub json_output: bool,
    #[serde(default)]
    pub verbose: bool,
    #[serde(default)]
    pub parallel: bool,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub silent: bool,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub depth: Option<usize>,
    /// `--include` as the user typed it. The host has already applied it
    /// (and `exclude_filters` and `tag_filters`) to `PluginRequest::projects`;
    /// a plugin that acts on repos it finds itself must refuse filters it
    /// cannot honor.
    #[serde(default)]
    pub include_filters: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_filters: Option<Vec<String>>,
    /// `--tag`'s tags, applied like `include_filters`. A host older than
    /// this field never sends it.
    #[serde(default)]
    pub tag_filters: Option<Vec<String>>,
    /// Convert warnings to errors for all-or-nothing behavior (CI/automation)
    #[serde(default)]
    pub strict: bool,
}

impl PluginRequestOptions {
    /// Whether the user chose repos with `--include`, `--exclude` or
    /// `--tag`: `PluginRequest::projects` holds only those. A plugin that
    /// finds its repos itself acts on those alone, or refuses the filters.
    pub fn filtered(&self) -> bool {
        self.include_filters.is_some()
            || self.exclude_filters.is_some()
            || self.tag_filters.is_some()
    }

    /// `dry_run` as the mode a command runs in: read it here, once, and
    /// hand the mode to everything that changes something.
    pub fn mode(&self) -> meta_core::ExecutionMode {
        meta_core::ExecutionMode::from_dry_run(self.dry_run)
    }
}

// ============================================================================
// Plugin-to-Host Response
// ============================================================================

/// An execution plan returned by a plugin, containing commands for the host to execute.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    /// Commands to run BEFORE the main parallel execution (sequential, must complete).
    /// Use for setup tasks like establishing SSH ControlMaster connections.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pre_commands: Vec<PlannedCommand>,

    /// Main commands to execute (may run in parallel based on `parallel` flag)
    pub commands: Vec<PlannedCommand>,

    /// Commands to run AFTER main execution completes (sequential).
    /// Use for cleanup tasks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_commands: Vec<PlannedCommand>,

    /// Whether to run main commands in parallel (overrides CLI --parallel if set)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel: Option<bool>,

    /// Maximum number of commands to run in parallel (pool size limit).
    /// Use to limit concurrency for operations with shared resources (e.g., SSH
    /// ControlMaster has a default session limit of 10). When set, loop_lib will
    /// run at most this many commands simultaneously.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<usize>,

    /// Milliseconds to wait between spawning parallel commands.
    /// Use to spread out initial connection bursts and prevent SSH ControlMaster
    /// socket saturation. A value of 25ms with 13 repos spreads spawns over ~325ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_stagger_ms: Option<u64>,
}

/// A single command to be executed by the host via loop_lib.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedCommand {
    /// Directory to execute in (relative to meta root or absolute)
    pub dir: String,
    /// The program and its arguments. The host runs them as given, with no
    /// shell in between, so no quoting is needed.
    pub argv: Vec<String>,
    /// Environment variables to set for this command's subprocess
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
}

/// Wrapper for the execution plan response (the JSON envelope plugins emit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanResponse {
    pub plan: ExecutionPlan,
}

// ============================================================================
// Command Result
// ============================================================================

/// The result of a plugin command execution.
pub enum CommandResult {
    /// A plan of commands to execute via loop_lib (simple form, no pre/post commands)
    Plan(Vec<PlannedCommand>, Option<bool>),
    /// A full execution plan with pre/post commands
    FullPlan(ExecutionPlan),
    /// A message to display (no commands to execute)
    Message(String),
    /// An error occurred: the plugin exits 1 (the host escapes the message)
    Error(String),
    /// The command line was wrong: the plugin exits 2
    Usage(String),
    /// The plugin already reported how it ended (a summary, a JSON report):
    /// it exits with this code and prints nothing more
    Exit(i32),
    /// Show help text (optionally with an error message prefix)
    ShowHelp(Option<String>),
}

/// An error as the plugin's result: with its causes (`{e:#}`), or as the
/// ending it carries ([`meta_core::exit::ending`]).
impl From<anyhow::Error> for CommandResult {
    fn from(e: anyhow::Error) -> Self {
        match meta_core::exit::ending(&e) {
            Some(code) => CommandResult::Exit(code),
            None if e.chain().any(|cause| cause.is::<meta_core::exit::Usage>()) => {
                CommandResult::Usage(format!("{e:#}"))
            }
            None => CommandResult::Error(format!("{e:#}")),
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Serialize and print an execution plan to stdout.
pub fn output_execution_plan(commands: Vec<PlannedCommand>, parallel: Option<bool>) {
    end(emit_json(&simple_plan(commands, parallel)));
}

/// The plan for `commands`, with no setup or cleanup.
fn simple_plan(commands: Vec<PlannedCommand>, parallel: Option<bool>) -> PlanResponse {
    PlanResponse {
        plan: ExecutionPlan {
            pre_commands: vec![],
            commands,
            post_commands: vec![],
            parallel,
            max_parallel: None,
            spawn_stagger_ms: None,
        },
    }
}

/// End the plugin as `result` says when it failed (see
/// [`meta_core::exit`]); return when it succeeded.
fn end(result: anyhow::Result<()>) {
    let code = meta_core::exit::code(meta_core::exit::flushed(result));
    if code != 0 {
        meta_core::exit::now(code);
    }
}

/// Serialize and print a full execution plan with pre/post commands to
/// stdout. A plugin whose stdout meta no longer reads ends here, quietly.
pub fn output_execution_plan_full(
    pre_commands: Vec<PlannedCommand>,
    commands: Vec<PlannedCommand>,
    post_commands: Vec<PlannedCommand>,
    parallel: Option<bool>,
    max_parallel: Option<usize>,
    spawn_stagger_ms: Option<u64>,
) {
    let plan = ExecutionPlan {
        pre_commands,
        commands,
        post_commands,
        parallel,
        max_parallel,
        spawn_stagger_ms,
    };
    end(emit_json(&PlanResponse { plan }));
}

/// Write `text` to stdout, flushed: meta reads a plugin's stdout until the
/// plugin ends.
fn emit(text: &str) -> anyhow::Result<()> {
    meta_core::out::stdout_bytes(text.as_bytes())?;
    Ok(meta_core::out::flush()?)
}

/// Write `value` to stdout as one line of JSON.
fn emit_json(value: &impl Serialize) -> anyhow::Result<()> {
    emit(&format!("{}\n", serde_json::to_string(value)?))
}

/// Read and parse a `PluginRequest` from stdin.
pub fn read_request_from_stdin() -> anyhow::Result<PluginRequest> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let request: PluginRequest = serde_json::from_str(&input)?;
    Ok(request)
}

/// A plugin's help, from its info. Every value comes from the plugin, so
/// each is escaped onto its own line.
pub fn help_text(info: &PluginInfo) -> String {
    use meta_core::display::escape;
    let mut out = String::new();
    let mut line = |text: &str| {
        out.push_str(text);
        out.push('\n');
    };
    let Some(help) = &info.help else {
        line(&format!(
            "meta {} v{}",
            escape(&info.name),
            escape(&info.version)
        ));
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
    let flat = "Commands".to_string();
    let sections: Vec<_> = if help.command_sections.is_empty() {
        vec![(&flat, &help.commands)]
    } else {
        help.command_sections.iter().collect()
    };
    for (title, commands) in sections.into_iter().filter(|(_, c)| !c.is_empty()) {
        line(&format!("{}:", escape(title)));
        for (cmd, desc) in commands {
            line(&format!("  {:<20} {}", escape(cmd), escape(desc)));
        }
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

/// Parse the arguments of `meta <command>` with `P`: its help (or version)
/// as a message, its error, escaped, as an error.
pub fn parse_args<P: clap::Parser>(command: &str, args: &[String]) -> Result<P, CommandResult> {
    let name = format!("meta {command}");
    let argv: Vec<String> = std::iter::once(name.clone())
        .chain(args.iter().cloned())
        .collect();
    P::try_parse_from(&argv).map_err(|e| {
        use clap::error::ErrorKind;
        match e.kind() {
            ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                CommandResult::Message(e.render().to_string().trim_end().to_string())
            }
            _ => {
                let message = meta_core::args::escaped_error::<P>(&argv).to_string();
                let message = message.strip_prefix("error: ").unwrap_or(&message);
                let mut message = message.trim_end().to_string();
                // a command that isn't one: the ones there are
                if e.kind() == ErrorKind::InvalidSubcommand {
                    let help = P::command().bin_name(&name).render_help().to_string();
                    let error = message.split("\n\nUsage:").next().unwrap_or_default();
                    message = format!("{error}\n\n{}", help.trim_end());
                }
                CommandResult::Usage(message)
            }
        }
    })
}

// ============================================================================
// Plugin Harness
// ============================================================================

/// Definition of a plugin, used by `run_plugin()` to eliminate main.rs boilerplate.
pub struct PluginDefinition {
    pub info: PluginInfo,
    /// The execute function: receives the parsed request and returns a CommandResult.
    pub execute: fn(PluginRequest) -> CommandResult,
}

/// Run a plugin's main loop. Handles `--meta-plugin-info` and `--meta-plugin-exec` flags.
///
/// This replaces the boilerplate main() function in each plugin binary.
/// Plugins only need to define their `PluginInfo` and an execute function.
///
/// Returns when the plugin succeeded; otherwise the plugin ends here, its
/// error reported once and escaped (see [`meta_core::exit`]): 1 for an
/// error, 2 for a wrong command line, 141 when meta stopped reading.
///
/// Initializes `env_logger` so plugins can use `log` macros with RUST_LOG.
pub fn run_plugin(plugin: PluginDefinition) {
    // Initialize logger for subprocess - inherits RUST_LOG from parent
    env_logger::init();
    meta_core::out::color();
    end(serve(&plugin));
}

fn serve(plugin: &PluginDefinition) -> anyhow::Result<()> {
    use meta_core::display::escape;
    let usage = |problem: String| {
        anyhow::Error::new(meta_core::exit::Usage(format!(
            "{problem}This binary is a meta plugin. Use via: meta {}",
            escape(&plugin.info.name)
        )))
    };
    let flag = std::env::args_os()
        .nth(1)
        .ok_or_else(|| usage(String::new()))?;
    match flag.to_str() {
        Some("--meta-plugin-info") => emit_json(&plugin.info),
        Some("--meta-plugin-exec") => execute(plugin),
        Some("--help" | "-h") => emit(&help_text(&plugin.info)),
        _ => Err(usage(format!(
            "Unknown flag: {}\n",
            escape(&flag.to_string_lossy())
        ))),
    }
}

/// `--meta-plugin-exec`: answer the request on stdin.
fn execute(plugin: &PluginDefinition) -> anyhow::Result<()> {
    use anyhow::Context;
    if let Err(e) = meta_core::children::own() {
        meta_core::out::warn(format!(
            "cannot pass signals on to the programs this plugin starts: {e}"
        ));
    }
    let mut request = read_request_from_stdin().context("Failed to parse plugin request")?;
    meta_core::out::set_json(request.options.json_output);
    if request.cwd.as_os_str().is_empty() {
        request.cwd = std::env::current_dir().context("Failed to get working directory")?;
    }
    match (plugin.execute)(request) {
        CommandResult::Plan(commands, parallel) => emit_json(&simple_plan(commands, parallel)),
        CommandResult::FullPlan(plan) => emit_json(&PlanResponse { plan }),
        CommandResult::Message(msg) if msg.is_empty() => Ok(()),
        CommandResult::Message(msg) => emit(&format!("{msg}\n")),
        CommandResult::Error(e) => Err(anyhow::anyhow!(e)),
        CommandResult::Usage(e) => Err(meta_core::exit::Usage(e).into()),
        CommandResult::Exit(code) => Err(meta_core::exit::ExitWith(code).into()),
        CommandResult::ShowHelp(None) => emit(&help_text(&plugin.info)),
        // on stderr, so it's seen even though meta reads stdout
        CommandResult::ShowHelp(Some(err)) => Err(meta_core::exit::Usage(format!(
            "{err}\n\n{}",
            help_text(&plugin.info).trim_end()
        ))
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_becomes_the_result_its_ending_says() {
        use meta_core::exit::{ExitWith, Usage};
        let result = |e: anyhow::Error| CommandResult::from(e.context("on the way up"));
        assert!(
            matches!(result(Usage("bad".into()).into()), CommandResult::Usage(m) if m.contains("bad"))
        );
        assert!(matches!(result(ExitWith(3).into()), CommandResult::Exit(3)));
        assert!(
            matches!(result(anyhow::anyhow!("failed")), CommandResult::Error(m) if m.contains("failed"))
        );
    }

    fn parse<T: serde::de::DeserializeOwned>(json: &str) -> serde_json::Result<T> {
        serde_json::from_str(json)
    }

    /// `value` as JSON and back
    fn round_trip<T: Serialize + serde::de::DeserializeOwned>(value: &T) -> T {
        serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
    }

    #[test]
    fn each_required_field_is_required() {
        assert!(parse::<PluginInfo>(r#"{"version":"1","commands":[]}"#).is_err());
        assert!(parse::<PluginInfo>(r#"{"name":"x","commands":[]}"#).is_err());
        assert!(parse::<PluginInfo>(r#"{"name":"x","version":"1"}"#).is_err());
        assert!(parse::<PluginHelp>("{}").is_err());
        assert!(parse::<PluginRequest>(r#"{"args":[]}"#).is_err());
        assert!(parse::<PlanResponse>(r#"{"commands":[]}"#).is_err());
        assert!(parse::<PlanResponse>(r#"{"plan":[]}"#).is_err());
        assert!(parse::<ExecutionPlan>(r#"{"parallel":true}"#).is_err());
        assert!(parse::<PlannedCommand>(r#"{"argv":["x"]}"#).is_err());
        assert!(parse::<PlannedCommand>(r#"{"dir":"x"}"#).is_err());
    }

    #[test]
    fn a_request_with_only_a_command_gets_the_defaults() {
        let request: PluginRequest = parse(r#"{"command":"git status"}"#).unwrap();
        assert_eq!(request.command, "git status");
        assert!(request.args.is_empty() && request.projects.is_empty());
        assert_eq!(request.cwd, PathBuf::new());
        let options = request.options;
        assert!(!options.filtered() && options.depth.is_none());
        assert!(![
            options.json_output,
            options.verbose,
            options.parallel,
            options.dry_run,
            options.silent,
            options.recursive,
            options.strict
        ]
        .contains(&true));
    }

    #[test]
    fn a_plan_with_only_commands_has_no_controls_and_sends_none() {
        let response: PlanResponse =
            parse(r#"{"plan":{"commands":[{"dir":"a","argv":["git","status"]}]}}"#).unwrap();
        let plan = &response.plan;
        assert!(plan.pre_commands.is_empty() && plan.post_commands.is_empty());
        assert_eq!(
            (plan.parallel, plan.max_parallel, plan.spawn_stagger_ms),
            (None, None, None)
        );
        assert!(plan.commands.iter().all(|c| c.env.is_none()));
        let sent = serde_json::to_value(&response).unwrap();
        let fields: Vec<&String> = sent["plan"].as_object().unwrap().keys().collect();
        assert_eq!(fields, ["commands"]);
        let command: Vec<&String> = sent["plan"]["commands"][0]
            .as_object()
            .unwrap()
            .keys()
            .collect();
        assert_eq!(command, ["argv", "dir"]);
    }

    #[test]
    fn an_empty_plan_is_a_plan_of_no_commands() {
        let written = serde_json::json!({"plan": {"commands": []}});
        let response: PlanResponse = serde_json::from_value(written.clone()).unwrap();
        assert!(response.plan.commands.is_empty());
        assert_eq!(serde_json::to_value(&response).unwrap(), written);
    }

    #[test]
    fn dir_and_argv_arrive_exactly_as_sent() {
        let argv: Vec<String> = [
            "git",
            "commit",
            "-m",
            "it's \"quoted\" $HOME `x` ; rm -rf /",
            "",
            "two  spaces",
            "ünïcödé ✓",
            "line\nbreak",
        ]
        .map(String::from)
        .to_vec();
        for dir in ["a b/c", "/abs/path", "../up", "."] {
            let sent = PlannedCommand {
                dir: dir.to_string(),
                argv: argv.clone(),
                env: None,
            };
            let got = round_trip(&sent);
            assert_eq!((got.dir.as_str(), &got.argv), (dir, &argv));
        }
    }

    #[test]
    fn every_plan_control_arrives_as_a_plugin_writes_it() {
        let written = serde_json::json!({"plan": {
            "pre_commands": [{"dir": "pre", "argv": ["true"]}],
            "commands": [
                {"dir": "a", "argv": ["true"]},
                {"dir": "b", "argv": ["true"], "env": {"GIT_PAGER": "cat"}},
            ],
            "post_commands": [{"dir": "post", "argv": ["true"]}],
            "parallel": false,
            "max_parallel": 3,
            "spawn_stagger_ms": 25,
        }});
        let plan = serde_json::from_value::<PlanResponse>(written.clone())
            .unwrap()
            .plan;
        let dirs = |c: &[PlannedCommand]| c.iter().map(|c| c.dir.clone()).collect::<Vec<_>>();
        assert_eq!(dirs(&plan.pre_commands), ["pre"]);
        assert_eq!(dirs(&plan.commands), ["a", "b"]);
        assert_eq!(dirs(&plan.post_commands), ["post"]);
        assert_eq!(
            (plan.parallel, plan.max_parallel, plan.spawn_stagger_ms),
            (Some(false), Some(3), Some(25))
        );
        let env = plan.commands[1].env.as_ref().unwrap();
        assert_eq!(env.get("GIT_PAGER").map(String::as_str), Some("cat"));
        // and a plugin built on this crate writes the same keys
        let rewritten = serde_json::to_value(PlanResponse { plan }).unwrap();
        assert_eq!(rewritten, written);
    }

    #[test]
    fn every_request_option_arrives_as_sent() {
        let sent = PluginRequest {
            command: "git status".to_string(),
            args: vec!["--short".to_string(), "".to_string()],
            projects: vec!["a".to_string(), "b c".to_string()],
            cwd: PathBuf::from("/work space"),
            options: PluginRequestOptions {
                json_output: true,
                verbose: true,
                parallel: true,
                dry_run: true,
                silent: true,
                recursive: true,
                depth: Some(2),
                include_filters: Some(vec!["a".to_string()]),
                exclude_filters: Some(vec!["b".to_string()]),
                tag_filters: Some(vec!["t".to_string()]),
                strict: true,
            },
        };
        // the keys as a plugin reads them
        let wire = serde_json::to_value(&sent).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "command": "git status",
                "args": ["--short", ""],
                "projects": ["a", "b c"],
                "cwd": "/work space",
                "options": {
                    "json_output": true, "verbose": true, "parallel": true,
                    "dry_run": true, "silent": true, "recursive": true, "depth": 2,
                    "include_filters": ["a"], "exclude_filters": ["b"],
                    "tag_filters": ["t"], "strict": true,
                },
            })
        );
        let got = round_trip(&sent);
        assert_eq!(
            (got.command, got.args, got.projects),
            (sent.command, sent.args, sent.projects)
        );
        assert_eq!(got.cwd, sent.cwd);
        let (o, s) = (got.options, sent.options);
        assert_eq!(
            [
                o.json_output,
                o.verbose,
                o.parallel,
                o.dry_run,
                o.silent,
                o.recursive,
                o.strict
            ],
            [true; 7]
        );
        assert_eq!(o.depth, s.depth);
        assert!(o.mode().is_preview());
        assert_eq!(
            (o.include_filters, o.exclude_filters, o.tag_filters),
            (s.include_filters, s.exclude_filters, s.tag_filters)
        );
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
        assert!(help.commands.is_empty() && help.command_sections.is_empty());
        assert!(help.examples.is_empty() && help.note.is_none());
        let written = serde_json::json!({
            "name": "git", "version": "1.0", "commands": ["git status"],
            "description": "Git across repos",
            "help": {
                "usage": "meta git <command>",
                "commands": {"status": "Show status"},
                "command_sections": {"Worktrees": {"worktree create": "Make a set"}},
                "examples": ["meta git status"],
                "note": "Raw git: meta exec -- git",
            },
        });
        let info: PluginInfo = serde_json::from_value(written.clone()).unwrap();
        let help = info.help.as_ref().unwrap();
        assert_eq!(info.description.as_deref(), Some("Git across repos"));
        assert_eq!(help.commands["status"], "Show status");
        assert_eq!(
            help.command_sections["Worktrees"]["worktree create"],
            "Make a set"
        );
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
            command_sections: IndexMap::new(),
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
        let plain = help_text(&info);
        info.help = Some(PluginHelp {
            usage: forged.clone(),
            commands,
            command_sections: IndexMap::new(),
            examples: vec![forged.clone()],
            note: Some(forged),
        });
        for text in [plain, help_text(&info)] {
            assert!(!text.contains('\u{1b}'), "{text}");
            assert!(!text.lines().any(|l| l.starts_with("FORGED")), "{text}");
            assert!(text.contains("x\\u{1b}[2J\\nFORGED"), "{text}");
        }
    }
}
