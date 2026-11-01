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
    /// (and `exclude_filters`) to `PluginRequest::projects`; a plugin that acts
    /// on repos it finds itself must refuse filters it cannot honor.
    #[serde(default)]
    pub include_filters: Option<Vec<String>>,
    #[serde(default)]
    pub exclude_filters: Option<Vec<String>>,
    /// Convert warnings to errors for all-or-nothing behavior (CI/automation)
    #[serde(default)]
    pub strict: bool,
}

impl PluginRequestOptions {
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
    /// An error occurred
    Error(String),
    /// Show help text (optionally with an error message prefix)
    ShowHelp(Option<String>),
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Serialize and print an execution plan to stdout.
pub fn output_execution_plan(commands: Vec<PlannedCommand>, parallel: Option<bool>) {
    output_execution_plan_full(vec![], commands, vec![], parallel, None, None);
}

/// Serialize and print a full execution plan with pre/post commands to stdout.
pub fn output_execution_plan_full(
    pre_commands: Vec<PlannedCommand>,
    commands: Vec<PlannedCommand>,
    post_commands: Vec<PlannedCommand>,
    parallel: Option<bool>,
    max_parallel: Option<usize>,
    spawn_stagger_ms: Option<u64>,
) {
    let response = PlanResponse {
        plan: ExecutionPlan {
            pre_commands,
            commands,
            post_commands,
            parallel,
            max_parallel,
            spawn_stagger_ms,
        },
    };
    println!("{}", serde_json::to_string(&response).unwrap());
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
    let argv: Vec<String> = std::iter::once(format!("meta {command}"))
        .chain(args.iter().cloned())
        .collect();
    P::try_parse_from(&argv).map_err(|e| {
        use clap::error::ErrorKind;
        match e.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                CommandResult::Message(e.render().to_string().trim_end().to_string())
            }
            _ => {
                let message = meta_core::args::escaped_error::<P>(&argv).to_string();
                let message = message.strip_prefix("error: ").unwrap_or(&message);
                CommandResult::Error(message.trim_end().to_string())
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
/// Initializes `env_logger` so plugins can use `log` macros with RUST_LOG.
pub fn run_plugin(plugin: PluginDefinition) {
    // Initialize logger for subprocess - inherits RUST_LOG from parent
    env_logger::init();

    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!(
            "This binary is a meta plugin. Use via: meta {}",
            plugin.info.name
        );
        std::process::exit(1);
    }

    match args[1].as_str() {
        "--meta-plugin-info" => {
            let json = serde_json::to_string_pretty(&plugin.info).unwrap();
            println!("{json}");
        }
        "--meta-plugin-exec" => {
            let mut request = match read_request_from_stdin() {
                Ok(req) => req,
                Err(e) => {
                    eprintln!("Failed to parse plugin request: {e}");
                    std::process::exit(1);
                }
            };
            if request.cwd.as_os_str().is_empty() {
                match std::env::current_dir() {
                    Ok(cwd) => request.cwd = cwd,
                    Err(e) => {
                        eprintln!("Error: Failed to get working directory: {e}");
                        std::process::exit(1);
                    }
                }
            }

            match (plugin.execute)(request) {
                CommandResult::Plan(commands, parallel) => {
                    output_execution_plan(commands, parallel);
                }
                CommandResult::FullPlan(plan) => {
                    let response = PlanResponse { plan };
                    println!("{}", serde_json::to_string(&response).unwrap());
                }
                CommandResult::Message(msg) => {
                    if !msg.is_empty() {
                        println!("{msg}");
                    }
                }
                CommandResult::Error(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
                CommandResult::ShowHelp(maybe_error) => {
                    if let Some(ref err) = maybe_error {
                        eprintln!("error: {err}");
                        eprintln!();
                        // Print help to stderr when there's an error (so it's visible even if meta captures stdout)
                        eprint!("{}", help_text(&plugin.info));
                        std::process::exit(1);
                    } else {
                        print!("{}", help_text(&plugin.info));
                        std::process::exit(0);
                    }
                }
            }
        }
        "--help" | "-h" => {
            print!("{}", help_text(&plugin.info));
        }
        _ => {
            eprintln!("Unknown flag: {}. This binary is a meta plugin.", args[1]);
            eprintln!("Use via: meta {}", plugin.info.name);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
