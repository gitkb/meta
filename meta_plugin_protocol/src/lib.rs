//! Shared plugin protocol types for meta subprocess plugins.
//!
//! This crate defines the communication protocol between the meta CLI host
//! and its subprocess plugins (meta-git, meta-project, meta-rust, etc.).
//!
//! The protocol works as follows:
//! 1. Host discovers plugins via `--meta-plugin-info` (plugin responds with `PluginInfo` JSON)
//! 2. Host invokes plugins via `--meta-plugin-exec` (sends `PluginRequest` JSON on stdin)
//! 3. Plugin responds with either a `PlanResponse` JSON (commands to execute) or direct output

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;

// ============================================================================
// Plugin Discovery Types
// ============================================================================

/// Metadata about a plugin, returned in response to `--meta-plugin-info`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
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
    #[serde(default)]
    pub cwd: String,
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

/// Write plugin help text to a writer.
fn write_plugin_help(info: &PluginInfo, w: &mut dyn std::io::Write) {
    if let Some(help) = &info.help {
        let _ = writeln!(w, "{}", help.usage);
        let _ = writeln!(w);

        // Use command_sections if present, otherwise fall back to flat commands
        if !help.command_sections.is_empty() {
            for (section_title, commands) in &help.command_sections {
                let _ = writeln!(w, "{section_title}:");
                for (cmd, desc) in commands {
                    let _ = writeln!(w, "  {cmd:<20} {desc}");
                }
                let _ = writeln!(w);
            }
        } else if !help.commands.is_empty() {
            let _ = writeln!(w, "Commands:");
            for (cmd, desc) in &help.commands {
                let _ = writeln!(w, "  {cmd:<20} {desc}");
            }
            let _ = writeln!(w);
        }

        if !help.examples.is_empty() {
            let _ = writeln!(w, "Examples:");
            for ex in &help.examples {
                let _ = writeln!(w, "  {ex}");
            }
            let _ = writeln!(w);
        }
        if let Some(note) = &help.note {
            let _ = writeln!(w, "{note}");
        }
    } else {
        let _ = writeln!(w, "meta {} v{}", info.name, info.version);
        if let Some(desc) = &info.description {
            let _ = writeln!(w, "{desc}");
        }
    }
}

/// Print plugin help text to stdout.
fn print_plugin_help(info: &PluginInfo) {
    write_plugin_help(info, &mut std::io::stdout());
}

/// Print plugin help text to stderr (for error cases where meta captures stdout).
fn eprint_plugin_help(info: &PluginInfo) {
    write_plugin_help(info, &mut std::io::stderr());
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
            let request = match read_request_from_stdin() {
                Ok(req) => req,
                Err(e) => {
                    eprintln!("Failed to parse plugin request: {e}");
                    std::process::exit(1);
                }
            };

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
                        eprint_plugin_help(&plugin.info);
                        std::process::exit(1);
                    } else {
                        print_plugin_help(&plugin.info);
                        std::process::exit(0);
                    }
                }
            }
        }
        "--help" | "-h" => {
            print_plugin_help(&plugin.info);
        }
        _ => {
            eprintln!("Unknown flag: {}. This binary is a meta plugin.", args[1]);
            eprintln!("Use via: meta {}", plugin.info.name);
            std::process::exit(1);
        }
    }
}
