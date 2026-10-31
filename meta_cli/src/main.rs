use anyhow::Result;
use clap::{Args, CommandFactory, Parser, Subcommand};
use colored::*;
use loop_lib::run;
use meta_core::config::{self, find_meta_config, MetaTreeNode, ProjectInfo, Workspace};
use meta_core::display::{escape, escape_lines, escape_path};
use meta_core::ExecutionMode;
use std::io::Write;
use std::path::PathBuf;

mod init;
mod registry;
mod subprocess_plugins;
use meta_cli::{repo_filter, worktree};
use meta_plugin_protocol::PluginRequestOptions;
use subprocess_plugins::SubprocessPluginManager;

// === CLI Structs ===

const VERSION: &str = include_str!("../../VERSION");

#[derive(Parser)]
#[command(
    author,
    version = VERSION.trim(),
    about,
    long_about = None,
    disable_help_flag = true,
    disable_help_subcommand = true
)]
struct Cli {
    /// Print help
    #[arg(short = 'h', long, global = true, action = clap::ArgAction::SetTrue)]
    help: bool,

    #[arg(
        long,
        global = true,
        help = "Add shell aliases to the global .looprc file"
    )]
    add_aliases_to_global_looprc: bool,

    #[arg(short, long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    #[arg(
        short,
        long,
        global = true,
        value_delimiter = ',',
        help = "Skip these repos: each value is a repo's .meta.yaml name or its whole path from the workspace root (. for the root), never part of one"
    )]
    exclude: Option<Vec<String>>,

    #[arg(
        short,
        long,
        global = true,
        value_delimiter = ',',
        help = "Only run in these repos: each value is a repo's .meta.yaml name or its whole path from the workspace root (. for the root), never part of one"
    )]
    include: Option<Vec<String>>,

    #[arg(long, global = true, help = "Output results in JSON format")]
    json: bool,

    #[arg(short, long, global = true, help = "Enable silent mode")]
    silent: bool,

    #[arg(short, long, action, global = true, help = "Enable verbose output")]
    verbose: bool,

    #[arg(
        long,
        short = 't',
        global = true,
        value_name = "TAGS",
        help = "Filter projects by tag(s), comma-separated"
    )]
    tag: Option<String>,

    #[arg(
        long,
        short = 'r',
        global = true,
        help = "Recursively process nested meta repos"
    )]
    recursive: bool,

    #[arg(
        long,
        global = true,
        value_name = "N",
        help = "Maximum depth for recursive processing (default: unlimited)"
    )]
    depth: Option<usize>,

    #[arg(
        long,
        global = true,
        help = "Show what commands would be run without executing them"
    )]
    dry_run: bool,

    #[arg(long, global = true, help = "Run commands in parallel")]
    parallel: bool,

    #[arg(
        long,
        global = true,
        help = "Run commands sequentially (overrides config default)"
    )]
    sequential: bool,

    #[arg(
        long,
        global = true,
        help = "Use primary checkout paths, overriding worktree context detection"
    )]
    primary: bool,

    #[arg(
        long,
        global = true,
        help = "Convert warnings to errors for all-or-nothing behavior (CI/automation)"
    )]
    strict: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

impl Cli {
    /// `--dry-run`, resolved once for every command
    fn mode(&self) -> ExecutionMode {
        ExecutionMode::from_dry_run(self.dry_run)
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Show workspace context summary
    Context(ContextArgs),
    /// Execute a command across all repos
    Exec(ExecArgs),
    /// Print this message or the help of the given subcommand(s)
    Help,
    /// Initialize meta integrations
    Init(InitArgs),
    /// Manage plugins
    Plugin(PluginArgs),
    #[command(external_subcommand)]
    External(Vec<String>),
}

/// Arguments for `meta context`
#[derive(Args)]
struct ContextArgs {
    /// Skip git status queries (structure only, faster)
    #[arg(long)]
    no_status: bool,

    /// Bypass cache and force fresh context generation
    #[arg(long)]
    no_cache: bool,
}

/// Arguments for `meta exec`
#[derive(Args)]
struct ExecArgs {
    /// Command and arguments to execute (use -- to separate from meta flags)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<String>,
}

/// Arguments for `meta init`
#[derive(Args)]
struct InitArgs {
    #[command(subcommand)]
    command: Option<InitCommands>,
}

#[derive(Subcommand)]
enum InitCommands {
    /// Install Claude Code skills, rules, and hooks for this meta repo
    Claude {
        /// Refresh skills and rules, merge meta hooks into settings
        #[arg(short, long)]
        force: bool,

        /// Update skills and rules only, preserve settings
        #[arg(short, long)]
        update: bool,
    },
}

/// Arguments for `meta plugin`
#[derive(Args)]
struct PluginArgs {
    #[command(subcommand)]
    command: Option<PluginCommands>,
}

#[derive(Subcommand)]
enum PluginCommands {
    /// Search for plugins in the registry
    Search {
        /// Search query
        query: Option<String>,
    },
    /// Install a plugin from the registry
    Install {
        /// Plugin name
        name: Option<String>,
        /// Install plugin locally to project (.meta/plugins/) instead of globally
        #[arg(long)]
        local: bool,
    },
    /// List installed plugins
    List {
        /// List only project-local plugins
        #[arg(long)]
        local: bool,
    },
    /// Uninstall a plugin
    Uninstall {
        /// Plugin name
        name: Option<String>,
        /// Uninstall from project-local plugins
        #[arg(long)]
        local: bool,
    },
    /// Update plugins to latest versions
    Update {
        /// Plugin name (updates all if not specified)
        name: Option<String>,
        /// Update from project-local plugins
        #[arg(long)]
        local: bool,
        /// Check for updates without installing
        #[arg(long)]
        check: bool,
    },
}

// === Help Utilities ===

/// Print help text with integrated plugin commands to stdout or stderr.
/// Use `to_stderr: true` for error cases where help is shown due to an invalid command.
fn print_help_with_plugins(plugins: &SubprocessPluginManager, to_stderr: bool) {
    let mut output: Box<dyn Write> = if to_stderr {
        Box::new(std::io::stderr())
    } else {
        Box::new(std::io::stdout())
    };
    let _ = write_help_with_plugin_commands(plugins, &mut output);
}

/// Write help text with plugin commands integrated into the command list.
fn write_help_with_plugin_commands(
    plugins: &SubprocessPluginManager,
    w: &mut dyn Write,
) -> std::io::Result<()> {
    let cmd = Cli::command();

    // Header
    if let Some(about) = cmd.get_about() {
        writeln!(w, "{}", about)?;
    }
    writeln!(w)?;

    // Usage
    writeln!(w, "Usage: meta [OPTIONS] [COMMAND]")?;
    writeln!(w)?;

    // Collect all commands: (name, description, plugin_source)
    let mut commands: Vec<(String, String, Option<String>)> = Vec::new();

    // Built-in commands from clap (skip "help" - we'll add it at the end)
    for subcommand in cmd.get_subcommands() {
        let name = subcommand.get_name();
        if name == "help" {
            continue;
        }
        let about = subcommand
            .get_about()
            .map(|s| s.to_string())
            .unwrap_or_default();
        commands.push((name.to_string(), about, None));
    }

    // Promoted plugin commands (top-level commands from plugins)
    for (name, desc, plugin) in plugins.get_promoted_commands() {
        commands.push((name, desc, Some(plugin)));
    }

    // Plugin namespaces must be claimed commands, not merely plugin names.
    // These are invoked as `meta <plugin> <subcommand>`
    // Mark them with plugin name and version suffix
    for (name, version, description) in plugins.list_plugins() {
        if !plugins.available_commands().iter().any(|(command, owner)| {
            *owner == name && command.split_whitespace().next() == Some(name)
        }) {
            continue;
        }
        commands.push((
            name.to_string(),
            description.to_string(),
            Some(format!("plugin: {name} v{version}")),
        ));
    }

    // Sort alphabetically
    commands.sort_by(|a, b| a.0.cmp(&b.0));

    // Add help at the end (standard convention)
    commands.push((
        "help".to_string(),
        "Print this message or the help of the given subcommand(s)".to_string(),
        None,
    ));

    // Render commands section; plugins name and describe their own commands,
    // so each value stays on its line
    writeln!(w, "Commands:")?;
    let commands: Vec<_> = commands
        .iter()
        .map(|(name, desc, plugin)| (escape(name), escape(desc), plugin.as_deref().map(escape)))
        .collect();
    let max_name_len = commands.iter().map(|c| c.0.len()).max().unwrap_or(10);
    for (name, desc, plugin_opt) in &commands {
        let suffix = plugin_opt
            .as_ref()
            .map(|p| format!(" [{}]", p))
            .unwrap_or_default();
        writeln!(
            w,
            "  {:<width$}  {}{}",
            name,
            desc,
            suffix,
            width = max_name_len
        )?;
    }
    writeln!(w)?;

    // Options - extract from clap
    writeln!(w, "Options:")?;
    for arg in cmd.get_arguments() {
        let name = arg.get_id().as_str();
        // Skip hidden or help/version (clap adds them automatically)
        if name == "help" || name == "version" {
            continue;
        }

        let mut flag_str = String::new();
        if let Some(short) = arg.get_short() {
            flag_str.push_str(&format!("-{}", short));
            if arg.get_long().is_some() {
                flag_str.push_str(", ");
            }
        }
        if let Some(long) = arg.get_long() {
            flag_str.push_str(&format!("--{}", long));
        }

        // Add value name if this arg takes a value (not a flag)
        // A flag has action SetTrue/SetFalse/Count - check num_args or action
        let is_flag = matches!(
            arg.get_action(),
            clap::ArgAction::SetTrue | clap::ArgAction::SetFalse | clap::ArgAction::Count
        );
        if !is_flag {
            if let Some(value_names) = arg.get_value_names() {
                if !value_names.is_empty() {
                    flag_str.push_str(&format!(" <{}>", value_names[0]));
                }
            }
        }

        let help = arg.get_help().map(|s| s.to_string()).unwrap_or_default();

        if !flag_str.is_empty() {
            writeln!(w, "  {:<38}  {}", flag_str, help)?;
        }
    }

    // Standard help/version
    writeln!(w, "  {:<38}  Print help", "-h, --help")?;
    writeln!(w, "  {:<38}  Print version", "-V, --version")?;

    Ok(())
}

/// Print help for `meta context` without generating workspace context.
fn print_context_help() {
    println!("meta context - Show workspace context summary");
    println!();
    println!("Usage: meta context [OPTIONS]");
    println!();
    println!("Options:");
    println!("      --no-status   Skip git status queries (structure only, faster)");
    println!("      --no-cache    Bypass cache and force fresh context generation");
    println!("  -h, --help        Print help");
    println!();
    println!("Shows the current meta workspace, repos, branches, status, key commands, and dependencies.");
}

/// Print help for the built-in plugin management command family.
fn print_plugin_help() {
    println!("meta plugin - Manage subprocess plugins");
    println!();
    println!("Usage: meta plugin <COMMAND>");
    println!();
    println!("Commands:");
    println!("  search <query>        Search for plugins in the registry");
    println!("  install <name>        Install a plugin (add --local for project-local)");
    println!("  list                  List installed plugins (add --local for project-local only)");
    println!("  uninstall <name>      Uninstall a plugin (add --local for project-local)");
    println!("  update [name]         Update plugins to latest versions");
}

/// Print help for `meta plugin search` without requiring a query.
fn print_plugin_search_help() {
    println!("meta plugin search - Search for plugins in the registry");
    println!();
    println!("Usage: meta plugin search <QUERY>");
    println!();
    println!("Arguments:");
    println!("  <QUERY>   Search query");
}

/// Print help for `meta plugin install` without requiring a plugin name.
fn print_plugin_install_help() {
    println!("meta plugin install - Install a plugin from the registry");
    println!();
    println!("Usage: meta plugin install <NAME> [--local]");
    println!();
    println!("Options:");
    println!("      --local   Install to .meta/plugins instead of ~/.meta/plugins");
}

/// Print help for `meta plugin list` without listing installed plugins.
fn print_plugin_list_help() {
    println!("meta plugin list - List installed plugins");
    println!();
    println!("Usage: meta plugin list [--local]");
    println!();
    println!("Options:");
    println!("      --local   List only project-local plugins");
}

/// Print help for `meta plugin uninstall` without requiring a plugin name.
fn print_plugin_uninstall_help() {
    println!("meta plugin uninstall - Uninstall a plugin");
    println!();
    println!("Usage: meta plugin uninstall <NAME> [--local]");
    println!();
    println!("Options:");
    println!("      --local   Uninstall from .meta/plugins instead of ~/.meta/plugins");
}

/// Print help for `meta plugin update` without checking the registry.
fn print_plugin_update_help() {
    println!("meta plugin update - Update plugins to latest versions");
    println!();
    println!("Usage: meta plugin update [NAME] [--local] [--check]");
    println!();
    println!("Options:");
    println!("      --local   Update project-local plugins");
    println!("      --check   Check for updates without installing");
}

// === Main Entry Point ===

/// Exit status for output cut off by a closed stdout, as a shell reports a
/// command SIGPIPE stopped (128 + 13): a failure it hid isn't success.
const CLOSED_STDOUT: u8 = 141;

/// `println!` panics when stdout is a closed pipe (`meta … | head`). End
/// quietly instead, as a fallible write to stdout does; other panics are
/// reported as usual.
fn exit_quietly_on_closed_stdout() {
    let report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("");
        if is_closed_stdout_panic(message) {
            std::process::exit(CLOSED_STDOUT.into());
        }
        report(info)
    }));
}

/// Whether `message` is std's panic for printing to a closed stdout: a
/// broken pipe, which Windows reports as os error 109 or 232.
fn is_closed_stdout_panic(message: &str) -> bool {
    message.starts_with("failed printing to stdout")
        && ["Broken pipe", "(os error 109)", "(os error 232)"]
            .iter()
            .any(|closed| message.contains(closed))
}

fn main() -> std::process::ExitCode {
    exit_quietly_on_closed_stdout();
    match run_cli() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e)
            if e.downcast_ref::<loop_lib::StdoutError>()
                .is_some_and(|e| e.0.kind() == std::io::ErrorKind::BrokenPipe) =>
        {
            std::process::ExitCode::from(CLOSED_STDOUT)
        }
        Err(e) => {
            // as Rust prints an error returned from main, but escaped: it can
            // carry paths and names from git and the filesystem
            eprintln!("Error: {}", escape_lines(&format!("{e:?}")));
            std::process::ExitCode::FAILURE
        }
    }
}

fn run_cli() -> Result<()> {
    // Log records carry names, versions, paths and URLs from config, the
    // filesystem and plugin registries: escape them like other output
    env_logger::Builder::from_default_env()
        .format(|buf, record| {
            use std::io::Write;
            writeln!(
                buf,
                "[{} {} {}] {}",
                buf.timestamp(),
                record.level(),
                record.target(),
                // Newlines too: one from data could forge a log line
                escape(&record.args().to_string())
            )
        })
        .init();

    let mut cli: Cli = meta_core::args::parse();

    log::debug!("cli.json = {}", cli.json);

    // Check for orphaned nested meta repo and warn the user
    check_and_warn_orphan();

    // Discover plugins early to handle --help requests and plugin listing
    let mut subprocess_plugins = SubprocessPluginManager::new();
    subprocess_plugins.discover_plugins(cli.verbose)?;

    // Handle --help flag at top level
    if cli.help && cli.command.is_none() {
        print_help_with_plugins(&subprocess_plugins, false);
        std::process::exit(0);
    }

    // Take command out so we can move subcommand args while still borrowing cli
    let command = cli.command.take();
    match command {
        None => {
            print_help_with_plugins(&subprocess_plugins, false);
            std::process::exit(0);
        }
        Some(Commands::Help) => {
            print_help_with_plugins(&subprocess_plugins, false);
            std::process::exit(0);
        }
        Some(Commands::Context(args)) => {
            if cli.help {
                print_context_help();
                return Ok(());
            }
            meta_cli::context::handle_context(
                cli.json,
                args.no_status,
                args.no_cache,
                cli.mode(),
                cli.verbose,
            )
        }
        Some(Commands::Init(args)) => {
            let cmd = match args.command {
                // `--help` must only print help, never install
                _ if cli.help => init::InitCommand::None,
                None => init::InitCommand::None,
                Some(InitCommands::Claude { force, update }) => {
                    init::InitCommand::Claude { force, update }
                }
            };
            init::handle_init_command(cmd, cli.mode(), cli.verbose)
        }
        Some(Commands::Plugin(args)) => handle_plugin_command(
            args.command,
            cli.mode(),
            cli.json,
            cli.help,
            &subprocess_plugins,
        ),
        Some(Commands::Exec(args)) => {
            // Handle help flag for exec command specifically
            if cli.help {
                println!("meta exec - Run any command across all repos");
                println!();
                println!("Usage: meta exec [OPTIONS] -- <COMMAND>...");
                println!();
                println!("Arguments:");
                println!("  <COMMAND>...    Command and arguments to run");
                println!();
                println!(
                    "The command runs as typed in each project directory defined in .meta.yaml,"
                );
                println!("never through a plugin (use `meta git ...` for the git plugin).");
                println!("No shell parses it: each argument reaches the program as given, so");
                println!("quotes, $(), ;, | and globs stay literal. For a shell, ask for one.");
                println!();
                println!("--include/--exclude take repos by .meta.yaml name or path from the root");
                println!("(. for the root), never part of a path.");
                println!();
                println!("Examples:");
                println!("  meta exec -- npm install");
                println!("  meta exec -- cargo fmt --check");
                println!("  meta exec -- make clean");
                println!("  meta --include api,web exec -- docker-compose up -d");
                println!("  meta exec -- sh -c 'npm ci && npm test'");
                std::process::exit(0);
            }
            handle_command_dispatch(args.command, &cli, &subprocess_plugins, true)
        }
        Some(Commands::External(args)) => {
            // clap doesn't capture global flags that appear after an external
            // subcommand name. Extract long-form global flags here so they
            // work in both positions (before and after the subcommand).
            let mut args = args;
            extract_global_flags(&mut args, &mut cli);

            // Keep root plugin help fast and plugin-aware, but let nested help
            // requests reach the matched plugin command implementation.
            if let Some(first) = args.first() {
                // A --help after `--` is an argument for the command
                let wants_help = args
                    .iter()
                    .take_while(|a| *a != "--")
                    .any(|a| a == "--help" || a == "-h");
                let is_bare = args.len() == 1;
                let is_root_help = wants_help
                    && args.len() == 2
                    && matches!(args.get(1).map(String::as_str), Some("--help" | "-h"));

                let promoted_commands: Vec<String> = subprocess_plugins
                    .get_promoted_commands()
                    .iter()
                    .map(|(name, _, _)| name.clone())
                    .collect();
                let is_promoted = promoted_commands.contains(&first.to_string());

                if is_root_help || (is_bare && !is_promoted) {
                    if let Some(help_text) = subprocess_plugins.get_plugin_help(first) {
                        subprocess_plugins::print_output(&format!("{help_text}\n"))?;
                        return Ok(());
                    }
                }

                if wants_help {
                    let options = PluginRequestOptions {
                        json_output: cli.json,
                        verbose: cli.verbose,
                        parallel: cli.parallel,
                        dry_run: cli.dry_run,
                        silent: cli.silent,
                        recursive: cli.recursive,
                        depth: cli.depth,
                        include_filters: cli.include.clone(),
                        exclude_filters: cli.exclude.clone(),
                        strict: cli.strict,
                    };
                    if subprocess_plugins.execute(&args, &[], None, options)? {
                        return Ok(());
                    }
                }
            }
            handle_command_dispatch(args, &cli, &subprocess_plugins, false)
        }
    }
}

// === Command Dispatch (shared by exec and external) ===

/// Run a command across the selected repos.
///
/// `meta exec` (is_explicit_exec=true) runs the command as typed and never
/// offers it to plugins, so a plugin's claimed command (e.g. meta-rust's
/// `cargo`) cannot shadow a raw one. External subcommands
/// (is_explicit_exec=false, e.g. `meta git status`) go to the plugin that owns
/// them, or fail as unrecognized.
fn handle_command_dispatch(
    command_args: Vec<String>,
    cli: &Cli,
    plugins: &SubprocessPluginManager,
    is_explicit_exec: bool,
) -> Result<()> {
    if command_args.is_empty() {
        if is_explicit_exec {
            // Show usage error (help is handled at the caller level via cli.help)
            eprintln!("Usage: meta exec -- <command> [args...]");
        } else {
            print_help_with_plugins(plugins, true);
        }
        std::process::exit(1);
    }

    // All meta flags come from clap globals (before the command).
    // Command args pass through untouched to avoid collisions with
    // identically-named flags (e.g., grep --include, git clone --depth).
    let recursive = cli.recursive;
    let dry_run = cli.dry_run;
    let depth = cli.depth;
    // --parallel wins, then --sequential, then the default of the config
    // the command runs from (parallel without one)
    let parallel_for = |defaults: Option<&config::MetaDefaults>| {
        cli.parallel || !cli.sequential && defaults.is_none_or(|d| d.parallel)
    };

    let command_str = loop_lib::display_command(&command_args);

    // Check if this is `git clone` - it doesn't require a .meta.yaml file because
    // its purpose is to clone the repo that contains the .meta.yaml file
    let is_git_clone = command_args.first().map(|s| s == "git").unwrap_or(false)
        && command_args.get(1).map(|s| s == "clone").unwrap_or(false);

    if is_git_clone && !is_explicit_exec {
        if cli.include.is_some() || cli.exclude.is_some() {
            anyhow::bail!("meta git clone clones every repo in the meta repo; it does not take --include/--exclude");
        }
        // Handle git clone directly via plugin without requiring .meta.yaml file
        let subprocess_options = PluginRequestOptions {
            json_output: cli.json,
            verbose: cli.verbose,
            parallel: false,
            dry_run,
            silent: cli.silent,
            recursive,
            depth,
            include_filters: None,
            exclude_filters: None,
            strict: cli.strict,
        };

        if plugins.execute(&command_args, &[], None, subprocess_options)? {
            if cli.verbose {
                println!("{}", "Git clone handled by subprocess plugin.".green());
            }
            return Ok(());
        } else {
            eprintln!("Error: No plugin available to handle 'git clone'");
            eprintln!("Make sure meta-git plugin is installed.");
            std::process::exit(1);
        }
    }

    let current_dir = std::env::current_dir()?;

    // Context detection: if cwd is inside a worktree, auto-scope to its repos
    if !cli.primary {
        let context = if let Some((name, root)) = worktree::detect_worktree_location(&current_dir) {
            let paths: Vec<_> = worktree::discover_worktree_members(&root)?
                .into_iter()
                .map(|member| member.path)
                .collect();
            // Inside a set, a command never falls back to the workspace's
            // own checkouts: they aren't the ones the caller is in. A plain
            // clone that is merely under a `.worktrees` directory is no set.
            if paths.is_empty() && worktree::holds_meta_checkout(&root) {
                anyhow::bail!(
                    "No checkouts of worktree set '{}' found in {}. Run from a set meta made, or pass --primary to run in the workspace.",
                    escape(&name),
                    escape_path(&root)
                );
            }
            (!paths.is_empty()).then_some((name, root, paths))
        } else {
            None
        };
        if let Some((task_name, task_dir, wt_paths)) = context {
            if cli.verbose {
                eprintln!(
                    "Detected worktree context: '{}' ({} repos)",
                    escape(&task_name),
                    wt_paths.len()
                );
            }

            if let Some(config_path) =
                worktree::set_config(&task_dir, &wt_paths, cli.config.as_ref())?
            {
                let workspace = Workspace::load(&config_path)?;
                let parallel = parallel_for(Some(&workspace.defaults));
                let meta_projects = workspace.projects;

                // A set lays its repos out as the workspace: a checkout's path
                // in the set is its project's path, nested meta repos included.
                // Only a filter reads the nested configs; the set's own copies
                // win over the workspace's (see set_tree).
                let filtered = cli.tag.is_some() || cli.include.is_some() || cli.exclude.is_some();
                let tree = if filtered {
                    worktree::set_tree(&task_dir, &wt_paths, &meta_projects)?
                } else {
                    Vec::new()
                };
                // Compared as directories: a project at a link in the set is
                // the checkout where the link leads
                let real = |path: &std::path::Path| {
                    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
                };
                let project_map: std::collections::HashMap<PathBuf, ProjectInfo> =
                    config::build_project_map(&tree, &task_dir, "")
                        .into_iter()
                        .map(|(place, (_, info))| (real(&task_dir.join(place)), info))
                        .collect();

                // Filter the checkouts by their projects' tags
                let wt_directories: Vec<String> = wt_paths
                    .iter()
                    .filter(|path| {
                        if let Some(ref tag_filter) = cli.tag {
                            if let Some(info) = project_map.get(&real(path)) {
                                matches_tag_filter(&info.tags, tag_filter)
                            } else {
                                // As in the workspace, only the root runs without a project
                                real(path) == real(&task_dir)
                            }
                        } else {
                            true
                        }
                    })
                    .map(|p| p.display().to_string())
                    .collect();

                // Selected by project name or by its path in the set
                let wt_directories = select_repos(cli, wt_directories, &task_dir, || {
                    repo_filter::repo_names(&task_dir, &meta_projects, &tree)
                })?;

                if wt_directories.is_empty() {
                    eprintln!(
                        "{}: no projects match tag filter '{}' in worktree '{}'",
                        "warning".yellow().bold(),
                        escape(cli.tag.as_deref().unwrap_or("")),
                        escape(&task_name)
                    );
                    return Ok(());
                }

                // Check if the config is from the worktree itself or the primary checkout
                let config_in_worktree = config_path.starts_with(&task_dir);
                if cli.verbose && !config_in_worktree {
                    eprintln!(
                        "Using config from primary checkout: {}",
                        escape_path(&config_path)
                    );
                }

                let config = loop_lib::LoopConfig {
                    directories: wt_directories.clone(),
                    ignore: vec![],
                    verbose: cli.verbose,
                    silent: cli.silent,
                    parallel,
                    dry_run,
                    json_output: cli.json,
                    add_aliases_to_global_looprc: false,
                    spawn_stagger_ms: 0,
                    env: None,
                    max_parallel: None,
                    root_dir: Some(task_dir.clone()),
                };

                // Plugin dispatch (full feature parity with normal path)
                let subprocess_options = PluginRequestOptions {
                    json_output: cli.json,
                    verbose: cli.verbose,
                    parallel,
                    dry_run,
                    silent: cli.silent,
                    recursive,
                    depth,
                    include_filters: cli.include.clone(),
                    exclude_filters: cli.exclude.clone(),
                    strict: cli.strict,
                };

                if is_explicit_exec {
                    run(&config, &command_args)?;
                } else if plugins.execute(
                    &command_args,
                    &wt_directories,
                    Some(&task_dir),
                    subprocess_options,
                )? {
                    if cli.verbose {
                        println!(
                            "{}",
                            "Command handled by subprocess plugin (worktree).".green()
                        );
                    }
                } else {
                    unrecognized_command_error(&command_args, &command_str, plugins);
                }
                return Ok(());
            }

            // No config found — degraded legacy path with warning
            if let Some(tag) = &cli.tag {
                anyhow::bail!(
                    "No meta config found for worktree '{}', so --tag '{}' cannot select its repos",
                    escape(&task_name),
                    escape(tag)
                );
            }
            if cli.verbose {
                eprintln!(
                    "{} No .meta.yaml found for worktree '{}'. Tags, plugins, and dependency features unavailable.",
                    "warning:".yellow().bold(),
                    escape(&task_name)
                );
            }

            let directories: Vec<String> =
                wt_paths.iter().map(|p| p.display().to_string()).collect();

            let directories = select_repos(cli, directories, &task_dir, Vec::new)?;
            let parallel = parallel_for(None);

            let config = loop_lib::LoopConfig {
                directories,
                ignore: vec![],
                verbose: cli.verbose,
                silent: cli.silent,
                parallel, // Use the determined parallel mode, not hardcoded false
                dry_run,
                json_output: cli.json,
                add_aliases_to_global_looprc: false,
                spawn_stagger_ms: 0,
                env: None,
                max_parallel: None,
                root_dir: Some(task_dir.clone()),
            };

            run(&config, &command_args)?;
            return Ok(());
        }
    }

    let Some(workspace) = Workspace::find(&current_dir, cli.config.as_ref())? else {
        let config_name = cli
            .config
            .as_ref()
            .map(|p| escape_path(p))
            .unwrap_or_else(|| config::META_CONFIG_NAME.to_string());
        eprintln!("Error: Could not find meta config file '{config_name}'");
        eprintln!("Searched from {} up to root", escape_path(&current_dir));
        std::process::exit(1);
    };
    let meta_dir = workspace.dir.as_path();
    let parallel = parallel_for(Some(&workspace.defaults));

    if cli.verbose {
        println!("{}", "Verbose mode enabled".green());
        println!(
            "Resolved config file path: {}",
            escape_path(&workspace.config_path)
        );
        println!("Executing command: {}", escape(&command_str));
    }

    let meta_projects = &workspace.projects;

    // Filter projects by tags if --tag is specified
    let filtered_projects: Vec<&ProjectInfo> = if let Some(ref tag_filter) = cli.tag {
        if cli.verbose {
            println!(
                "Filtering projects by tags: {:?}",
                tag_filter.split(',').map(|s| s.trim()).collect::<Vec<_>>()
            );
        }
        meta_projects
            .iter()
            .filter(|p| matches_tag_filter(&p.tags, tag_filter))
            .collect()
    } else {
        meta_projects.iter().collect()
    };

    let meta_dir_str = meta_dir.to_string_lossy().to_string();
    let mut project_paths = vec![meta_dir_str.clone()];
    project_paths.extend(
        filtered_projects
            .iter()
            .map(|p| meta_dir.join(&p.path).to_string_lossy().to_string()),
    );

    // If recursive mode is enabled, discover nested meta repos
    let mut tree = vec![];
    if recursive {
        if cli.verbose {
            let depth_str = depth.map_or("unlimited".to_string(), |d| d.to_string());
            println!("Recursive mode enabled, max depth: {depth_str}");
        }
        // From the config already loaded (the default or --config's), into
        // the nested meta repos
        tree = config::walk_projects(meta_dir, meta_projects, depth)?;
        project_paths = vec![meta_dir_str.clone()];
        let flat = flatten_with_tag_filter(&tree, &cli.tag);
        project_paths.extend(
            flat.iter()
                .map(|p| meta_dir.join(p).to_string_lossy().to_string()),
        );
    }

    // Commands and plugins get only the selected repos
    let project_paths = select_repos(cli, project_paths, meta_dir, || {
        repo_filter::repo_names(meta_dir, meta_projects, &tree)
    })?;

    let config = loop_lib::LoopConfig {
        add_aliases_to_global_looprc: cli.add_aliases_to_global_looprc,
        directories: project_paths.clone(),
        ignore: vec![],
        verbose: cli.verbose,
        silent: cli.silent,
        parallel,
        dry_run,
        json_output: cli.json,
        spawn_stagger_ms: 0,
        env: None,
        max_parallel: None,
        root_dir: Some(meta_dir.to_path_buf()),
    };

    // Plugin dispatch for external subcommands
    let subprocess_options = PluginRequestOptions {
        json_output: cli.json,
        verbose: cli.verbose,
        parallel,
        dry_run,
        silent: cli.silent,
        recursive,
        depth,
        include_filters: cli.include.clone(),
        exclude_filters: cli.exclude.clone(),
        strict: cli.strict,
    };

    if is_explicit_exec {
        // User explicitly requested exec, run the command in all repos
        log::info!("Explicit exec requested, running command via loop");
        if cli.verbose {
            println!("{}", "Running command via loop (explicit exec).".green());
        }
        run(&config, &command_args)?;
    } else if plugins.execute(
        &command_args,
        &project_paths,
        Some(meta_dir),
        subprocess_options,
    )? {
        log::info!("Command was handled by subprocess plugin");
        if cli.verbose {
            println!("{}", "Command handled by subprocess plugin.".green());
        }
    } else {
        unrecognized_command_error(&command_args, &command_str, plugins);
    }

    Ok(())
}

// === Plugin Management ===

/// Create a plugin installer for the specified scope (local or global)
fn create_installer(local: bool) -> Result<registry::PluginInstaller> {
    if local {
        registry::PluginInstaller::new_local()
    } else {
        registry::PluginInstaller::new()
    }
}

/// Format the plugin location string for user-facing messages
fn format_plugin_location(local: bool) -> &'static str {
    if local {
        ".meta/plugins"
    } else {
        "~/.meta/plugins"
    }
}

/// Handle plugin management subcommands with typed args.
fn handle_plugin_command(
    command: Option<PluginCommands>,
    mode: ExecutionMode,
    json: bool,
    help: bool,
    subprocess_plugins: &SubprocessPluginManager,
) -> Result<()> {
    use registry::{PluginInstaller, RegistryClient, PLUGIN_PREFIX};

    let command = match command {
        Some(cmd) => cmd,
        None => {
            print_plugin_help();
            return Ok(());
        }
    };

    // These change installed plugins and have no preview
    if mode.is_preview() && !help {
        let changing = match &command {
            PluginCommands::Install { .. } => Some("install"),
            PluginCommands::Uninstall { .. } => Some("uninstall"),
            PluginCommands::Update { check: false, .. } => Some("update"),
            _ => None,
        };
        if let Some(name) = changing {
            anyhow::bail!(
                "meta plugin {name} has no preview, so it refuses --dry-run; nothing was changed"
            );
        }
    }

    match command {
        PluginCommands::Search { query } => {
            if help {
                print_plugin_search_help();
                return Ok(());
            }
            let Some(query) = query else {
                anyhow::bail!("Usage: meta plugin search <QUERY>");
            };
            let client = RegistryClient::new()?;
            let results = client.search(&query)?;

            if json {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else if results.is_empty() {
                println!("No plugins found matching '{}'", escape(&query));
            } else {
                println!("Found {} plugin(s):", results.len());
                for plugin in results {
                    println!(
                        "  {} v{} - {}",
                        escape(&plugin.name),
                        escape(&plugin.version),
                        escape(&plugin.description)
                    );
                    println!("    by {}", escape(&plugin.author));
                }
            }
        }
        PluginCommands::Install { name, local } => {
            if help {
                print_plugin_install_help();
                return Ok(());
            }
            let Some(name) = name else {
                anyhow::bail!("Usage: meta plugin install <NAME> [--local]");
            };
            use registry::GitHubShorthand;
            let installer = create_installer(local)?;
            let location = format_plugin_location(local);

            // Detect input type and route accordingly
            if name.starts_with("http://") || name.starts_with("https://") {
                // Direct URL install
                let plugin_name = installer.install_from_url(&name)?;
                if !json {
                    println!(
                        "Successfully installed {} to {location}",
                        escape(&plugin_name)
                    );
                }
            } else if let Some(shorthand) = GitHubShorthand::parse(&name) {
                // GitHub shorthand install (user/repo[@version])
                let plugin_name = installer.install_from_github(&shorthand)?;
                if !json {
                    println!(
                        "Successfully installed {} to {location}",
                        escape(&plugin_name)
                    );
                }
            } else {
                // Registry-based install
                let client = RegistryClient::new()?;

                // Try simple registry format first (M6: plugins/{name} contains GitHub shorthand)
                match client.resolve_plugin_source(&name) {
                    Ok(source) => {
                        // Got GitHub shorthand from registry, use GitHub install flow
                        if let Some(shorthand) = GitHubShorthand::parse(&source) {
                            let plugin_name = installer.install_from_github(&shorthand)?;
                            if !json {
                                println!(
                                    "Successfully installed {} from {} to {location}",
                                    escape(&plugin_name),
                                    escape(&source)
                                );
                            }
                        } else {
                            anyhow::bail!("Invalid plugin source in registry: {}", escape(&source));
                        }
                    }
                    Err(_) => {
                        // Fall back to complex registry format (plugins/{name}/plugin.json)
                        let metadata = client.fetch_plugin_metadata(&name)?;
                        let installed = installer.install(&metadata)?;

                        if !json {
                            println!(
                                "Successfully installed {} v{} ({}) to {location}",
                                escape(&metadata.name),
                                escape(&metadata.version),
                                escape(&installed.join(", "))
                            );
                        }
                    }
                }
            }
        }
        PluginCommands::List { local } => {
            if help {
                print_plugin_list_help();
                return Ok(());
            }
            if local {
                let installer = PluginInstaller::new_local()?;
                let plugins = installer.list_plugins_detailed()?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&plugins)?);
                } else if plugins.is_empty() {
                    println!("No project-local plugins installed");
                } else {
                    println!("Project-local plugins ({}):", plugins.len());
                    println!();
                    println!("{:<12} {:<12} PATH", "NAME", "VERSION");
                    println!("{}", "-".repeat(70));
                    for plugin in plugins {
                        let name = plugin
                            .name
                            .strip_prefix(PLUGIN_PREFIX)
                            .unwrap_or(&plugin.name);
                        let version = plugin.version.as_deref().unwrap_or("-");
                        let path = installer.plugins_dir().join(&plugin.name);
                        println!(
                            "{:<12} {:<12} {}",
                            escape(name),
                            escape(version),
                            escape_path(&path)
                        );
                    }
                }
            } else {
                // Use discovered plugins from subprocess plugin manager
                let plugins = subprocess_plugins.list_plugins_with_paths();

                if json {
                    let json_plugins: Vec<_> = plugins
                        .iter()
                        .map(|(name, version, desc, path)| {
                            serde_json::json!({
                                "name": name,
                                "version": version,
                                "description": desc,
                                "path": path.display().to_string()
                            })
                        })
                        .collect();
                    println!("{}", serde_json::to_string_pretty(&json_plugins)?);
                } else if plugins.is_empty() {
                    println!("No plugins installed");
                } else {
                    println!("Installed plugins ({}):", plugins.len());
                    println!();
                    println!("{:<12} {:<12} PATH", "NAME", "VERSION");
                    println!("{}", "-".repeat(70));
                    for (name, version, _, path) in &plugins {
                        println!(
                            "{:<12} {:<12} {}",
                            escape(name),
                            escape(version),
                            escape_path(path)
                        );
                    }
                }
            }
        }
        PluginCommands::Uninstall { name, local } => {
            if help {
                print_plugin_uninstall_help();
                return Ok(());
            }
            let Some(name) = name else {
                anyhow::bail!("Usage: meta plugin uninstall <NAME> [--local]");
            };
            let installer = create_installer(local)?;
            let location = format_plugin_location(local);
            installer.uninstall(&name)?;

            if !json {
                println!("Successfully uninstalled {} from {location}", escape(&name));
            }
        }
        PluginCommands::Update { name, local, check } => {
            if help {
                print_plugin_update_help();
                return Ok(());
            }
            let installer = create_installer(local)?;
            let location = format_plugin_location(local);

            if let Some(plugin_name) = name {
                // Update specific plugin
                match installer.check_update(&plugin_name)? {
                    Some((current, latest)) => {
                        if check {
                            if !json {
                                println!(
                                    "Update available for {}: {} → {}",
                                    escape(&plugin_name),
                                    escape(&current),
                                    escape(&latest)
                                );
                            }
                        } else {
                            let updated = installer.update_plugin(&plugin_name)?;
                            if !json {
                                println!(
                                    "Successfully updated {} from {} to {} in {}",
                                    escape(&updated),
                                    escape(&current),
                                    escape(&latest),
                                    escape(location)
                                );
                            }
                        }
                    }
                    None => {
                        if !json {
                            println!("{} is already up to date", escape(&plugin_name));
                        }
                    }
                }
            } else {
                // Update all plugins
                let plugins = installer.list_plugins_detailed()?;
                let mut updates_available = Vec::new();
                let mut updated_count = 0;

                for plugin in plugins {
                    let name = plugin
                        .name
                        .strip_prefix(PLUGIN_PREFIX)
                        .unwrap_or(&plugin.name);
                    if let Ok(Some((current, latest))) = installer.check_update(name) {
                        updates_available.push((name.to_string(), current, latest));
                    }
                }

                if updates_available.is_empty() {
                    if !json {
                        println!("All plugins are up to date");
                    }
                } else if check {
                    if !json {
                        println!("Updates available:");
                        for (name, current, latest) in &updates_available {
                            println!(
                                "  {} {} → {}",
                                escape(name),
                                escape(current),
                                escape(latest)
                            );
                        }
                    }
                } else {
                    for (name, current, latest) in &updates_available {
                        match installer.update_plugin(name) {
                            Ok(_) => {
                                if !json {
                                    println!(
                                        "Updated {} from {} to {}",
                                        escape(name),
                                        escape(current),
                                        escape(latest)
                                    );
                                }
                                updated_count += 1;
                            }
                            Err(e) => {
                                if !json {
                                    eprintln!(
                                        "Failed to update {}: {}",
                                        escape(name),
                                        escape_lines(&format!("{e:#}"))
                                    );
                                }
                            }
                        }
                    }
                    if !json {
                        println!(
                            "\nUpdated {} plugin(s) in {}",
                            updated_count,
                            escape(location)
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

// === Helpers ===

/// Extract meta-only global flags from external subcommand args.
///
/// clap's `external_subcommand` captures all tokens after the first unrecognized
/// subcommand, including global flags like `--json`. This function pulls them
/// out and applies them to the CLI struct so they work regardless of position.
///
/// Only extracts flags that are meta-global and NOT reused by plugin subcommands.
/// Flags like `--dry-run` and `--parallel` are left in args because plugin
/// subcommands (e.g. `worktree prune --dry-run`, `worktree exec --parallel`)
/// define their own versions and need to see them.
fn extract_global_flags(args: &mut Vec<String>, cli: &mut Cli) {
    // Everything from `--` on belongs to the command, untouched
    let rest = args.split_off(args.iter().position(|a| a == "--").unwrap_or(args.len()));
    args.retain(|arg| {
        match arg.as_str() {
            "--json" => {
                cli.json = true;
                false
            }
            "--verbose" => {
                cli.verbose = true;
                false
            }
            "--silent" => {
                cli.silent = true;
                false
            }
            "--primary" => {
                cli.primary = true;
                false
            }
            "--recursive" => {
                cli.recursive = true;
                false
            }
            "--strict" => {
                cli.strict = true;
                false
            }
            _ => true, // keep in args
        }
    });
    args.extend(rest);
}

/// Check whether a project's tags match a comma-separated tag filter string.
fn matches_tag_filter(tags: &[String], filter: &str) -> bool {
    let requested: Vec<&str> = filter.split(',').map(|s| s.trim()).collect();
    tags.iter().any(|t| requested.contains(&t.as_str()))
}

/// The repos among `dirs` that `--include`/`--exclude` select (see
/// [`repo_filter::select`]); `names` lists the `.meta.yaml` names they may use.
fn select_repos(
    cli: &Cli,
    dirs: Vec<String>,
    root: &std::path::Path,
    names: impl FnOnce() -> Vec<(String, String)>,
) -> Result<Vec<String>> {
    let include = cli.include.clone().unwrap_or_default();
    let exclude = cli.exclude.clone().unwrap_or_default();
    if include.is_empty() && exclude.is_empty() {
        return Ok(dirs);
    }
    repo_filter::select(&include, &exclude, &dirs, root, &names())
}

/// Print unrecognized command error with suggestion and help, then exit.
fn unrecognized_command_error(
    command_args: &[String],
    command_str: &str,
    plugins: &SubprocessPluginManager,
) -> ! {
    let first_cmd = command_args.first().map(|s| s.as_str()).unwrap_or("");
    eprintln!(
        "{}: unrecognized command '{}'",
        "error".red().bold(),
        escape(first_cmd)
    );
    let command_str = escape(command_str);
    eprintln!();
    eprintln!("To run '{command_str}' across all repos:");
    eprintln!("    meta exec -- {command_str}");
    eprintln!();
    print_help_with_plugins(plugins, true);
    std::process::exit(1);
}

/// Check if current directory is in an orphaned nested meta repo and warn the user.
///
/// An "orphan" is a nested meta repo that isn't tracked by its parent meta config.
/// This helps users understand why recursive operations might not find their nested repos.
fn check_and_warn_orphan() {
    let current_dir = match std::env::current_dir() {
        Ok(d) => d,
        Err(_) => return,
    };

    if worktree::detect_worktree_context(&current_dir).is_some() {
        return;
    }

    // Find the nearest .meta.yaml
    let Ok(Some(config_path)) = find_meta_config(&current_dir, None) else {
        return;
    };

    let meta_dir = match config_path.parent() {
        Some(p) => p.to_path_buf(),
        None => return,
    };

    // Check if this meta repo is orphaned (not tracked by parent)
    // A set's checkout is meta's own, not a repo its parent forgot to list
    if config::set_source(&meta_dir).is_some() {
        return;
    }
    if let Some(warning) = config::check_orphan_status(&meta_dir) {
        eprintln!(
            "{}: This meta repo is not tracked by its parent.",
            "warning".yellow().bold()
        );
        eprintln!("  Current: {}", escape_path(&warning.current));
        eprintln!("  Parent:  {}", escape_path(&warning.parent));
        eprintln!();

        eprintln!(
            "  To include it, add to {}:",
            escape_path(&warning.parent_config)
        );
        eprintln!("    {}:", escape(&warning.suggested_key));
        eprintln!("      repo: <git-url>");
        eprintln!("      meta: true");
        eprintln!();
    }
}

/// Flatten a meta tree into path strings, optionally filtering by tag.
/// If tag_filter is Some, only includes nodes whose tags match (and recurses into them).
fn flatten_with_tag_filter(nodes: &[MetaTreeNode], tag_filter: &Option<String>) -> Vec<String> {
    let mut paths = Vec::new();
    flatten_filtered_inner(nodes, tag_filter, "", &mut paths);
    paths
}

fn flatten_filtered_inner(
    nodes: &[MetaTreeNode],
    tag_filter: &Option<String>,
    prefix: &str,
    paths: &mut Vec<String>,
) {
    for node in nodes {
        let matches = match tag_filter {
            Some(ref tag_str) => matches_tag_filter(&node.info.tags, tag_str),
            None => true,
        };

        if matches {
            let full_path = if prefix.is_empty() {
                node.info.path.clone()
            } else {
                format!("{}/{}", prefix, node.info.path)
            };
            paths.push(full_path.clone());
            flatten_filtered_inner(&node.children, tag_filter, &full_path, paths);
        }
    }
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_stdout_panics_are_recognised_on_every_platform() {
        for message in [
            "failed printing to stdout: Broken pipe (os error 32)",
            "failed printing to stdout: The pipe has been ended. (os error 109)",
            "failed printing to stdout: The pipe is being closed. (os error 232)",
        ] {
            assert!(is_closed_stdout_panic(message), "{message}");
        }
        assert!(!is_closed_stdout_panic(
            "failed printing to stdout: No space left on device (os error 28)"
        ));
        assert!(!is_closed_stdout_panic("index out of bounds: Broken pipe"));
    }
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_parse_meta_config_valid_simple_format() {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            r#"{{
                "projects": {{
                    "repo1": "git@github.com:org/repo1.git",
                    "repo2": "git@github.com:org/repo2.git"
                }}
            }}"#
        )
        .unwrap();

        let Workspace { projects, .. } = Workspace::load(file.path()).unwrap();
        assert_eq!(projects.len(), 2);

        let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"repo1"));
        assert!(names.contains(&"repo2"));

        // Simple format: path defaults to project name
        let repo1 = projects.iter().find(|p| p.name == "repo1").unwrap();
        assert_eq!(repo1.path, "repo1");
        assert_eq!(repo1.repo.as_ref().unwrap(), "git@github.com:org/repo1.git");
        assert!(repo1.tags.is_empty());
    }

    #[test]
    fn test_parse_meta_config_extended_format() {
        let mut file = NamedTempFile::new().unwrap();
        write!(
            file,
            r#"{{
                "projects": {{
                    "api": {{
                        "repo": "git@github.com:org/api.git",
                        "path": "./services/api",
                        "tags": ["backend", "rust"]
                    }}
                }}
            }}"#
        )
        .unwrap();

        let projects = meta_core::config::Workspace::load(file.path())
            .unwrap()
            .projects;
        assert_eq!(projects.len(), 1);

        let api = &projects[0];
        assert_eq!(api.name, "api");
        assert_eq!(api.path, "services/api"); // normalized
        assert_eq!(api.repo.as_ref().unwrap(), "git@github.com:org/api.git");
        assert_eq!(api.tags, vec!["backend", "rust"]);
    }

    #[test]
    fn test_parse_meta_config_invalid_json() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "invalid json").unwrap();

        let result = meta_core::config::Workspace::load(file.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_meta_config_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("test.yaml");
        std::fs::write(
            &yaml_path,
            r#"
projects:
  web-app: git@github.com:org/web-app.git
  api:
    repo: git@github.com:org/api.git
    path: ./backend/api
    tags:
      - backend
      - python
"#,
        )
        .unwrap();

        let Workspace { projects, .. } = Workspace::load(&yaml_path).unwrap();
        assert_eq!(projects.len(), 2);

        let web_app = projects.iter().find(|p| p.name == "web-app").unwrap();
        assert_eq!(web_app.path, "web-app");
        assert_eq!(
            web_app.repo.as_ref().unwrap(),
            "git@github.com:org/web-app.git"
        );

        let api = projects.iter().find(|p| p.name == "api").unwrap();
        assert_eq!(api.path, "backend/api"); // normalized
        assert_eq!(api.tags, vec!["backend", "python"]);
    }

    #[test]
    fn test_find_meta_config() {
        let dir = tempfile::tempdir().unwrap();
        let meta_path = dir.path().join(".meta.yaml");
        std::fs::write(&meta_path, r#"{"projects": {}}"#).unwrap();

        let result = find_meta_config(dir.path(), None).unwrap();
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(path, meta_path);
    }

    #[test]
    fn test_parse_meta_config_with_tags() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("test.yaml");
        std::fs::write(
            &yaml_path,
            r#"
projects:
  frontend:
    repo: git@github.com:org/frontend.git
    tags:
      - ui
      - typescript
  backend:
    repo: git@github.com:org/backend.git
    tags:
      - api
      - rust
  shared:
    repo: git@github.com:org/shared.git
    tags:
      - ui
      - api
"#,
        )
        .unwrap();

        let projects = meta_core::config::Workspace::load(&yaml_path)
            .unwrap()
            .projects;
        assert_eq!(projects.len(), 3);

        let frontend = projects.iter().find(|p| p.name == "frontend").unwrap();
        assert_eq!(frontend.tags, vec!["ui", "typescript"]);

        let backend = projects.iter().find(|p| p.name == "backend").unwrap();
        assert_eq!(backend.tags, vec!["api", "rust"]);

        let shared = projects.iter().find(|p| p.name == "shared").unwrap();
        assert_eq!(shared.tags, vec!["ui", "api"]);

        // Filter projects by tag "ui"
        let ui_projects: Vec<_> = projects
            .iter()
            .filter(|p| p.tags.contains(&"ui".to_string()))
            .collect();
        assert_eq!(ui_projects.len(), 2);

        // Filter projects by tag "rust"
        let rust_projects: Vec<_> = projects
            .iter()
            .filter(|p| p.tags.contains(&"rust".to_string()))
            .collect();
        assert_eq!(rust_projects.len(), 1);
        assert_eq!(rust_projects[0].name, "backend");
    }

    #[test]
    fn test_discover_nested_projects_no_nested() {
        let dir = tempfile::tempdir().unwrap();
        // Create a .meta.yaml with a project that has no nested .meta.yaml
        let project_dir = dir.path().join("project1");
        std::fs::create_dir(&project_dir).unwrap();
        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"project1": "git@github.com:org/project1.git"}}"#,
        )
        .unwrap();

        let tree = config::walk_meta_tree(dir.path(), None).unwrap();
        let paths = config::flatten_meta_tree(&tree);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], "project1");
    }

    #[test]
    fn test_discover_nested_projects_with_nested() {
        let dir = tempfile::tempdir().unwrap();

        // Create nested structure:
        // root/
        //   .meta.yaml (contains project1)
        //   project1/
        //     .meta.yaml (contains subproject)
        //     subproject/

        let project1_dir = dir.path().join("project1");
        let subproject_dir = project1_dir.join("subproject");
        std::fs::create_dir_all(&subproject_dir).unwrap();

        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"project1": "git@github.com:org/project1.git"}}"#,
        )
        .unwrap();
        std::fs::write(
            project1_dir.join(".meta.yaml"),
            r#"{"projects": {"subproject": "git@github.com:org/subproject.git"}}"#,
        )
        .unwrap();

        let tree = config::walk_meta_tree(dir.path(), None).unwrap();
        let paths = config::flatten_meta_tree(&tree);

        assert_eq!(paths.len(), 2);
        assert!(paths.contains(&"project1".to_string()));
        assert!(paths.contains(&"project1/subproject".to_string()));
    }

    #[test]
    fn test_discover_nested_projects_with_depth_limit() {
        let dir = tempfile::tempdir().unwrap();

        let level1 = dir.path().join("level1");
        let level2 = level1.join("level2");
        std::fs::create_dir_all(&level2).unwrap();

        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{"projects": {"level1": "git@github.com:org/level1.git"}}"#,
        )
        .unwrap();
        std::fs::write(
            level1.join(".meta.yaml"),
            r#"{"projects": {"level2": "git@github.com:org/level2.git"}}"#,
        )
        .unwrap();

        // depth 0: no recursion into nested .meta.yaml
        let tree_0 = config::walk_meta_tree(dir.path(), Some(0)).unwrap();
        let paths_0 = config::flatten_meta_tree(&tree_0);
        assert_eq!(paths_0, vec!["level1"]);

        // depth 1: recurse one level
        let tree_1 = config::walk_meta_tree(dir.path(), Some(1)).unwrap();
        let paths_1 = config::flatten_meta_tree(&tree_1);
        assert_eq!(paths_1.len(), 2);
        assert!(paths_1.contains(&"level1/level2".to_string()));
    }

    #[test]
    fn test_discover_nested_projects_with_tag_filter() {
        let dir = tempfile::tempdir().unwrap();

        let frontend_dir = dir.path().join("frontend");
        let backend_dir = dir.path().join("backend");
        std::fs::create_dir_all(&frontend_dir).unwrap();
        std::fs::create_dir_all(&backend_dir).unwrap();

        std::fs::write(
            dir.path().join(".meta.yaml"),
            r#"{
                "projects": {
                    "frontend": {
                        "repo": "git@github.com:org/frontend.git",
                        "tags": ["ui"]
                    },
                    "backend": {
                        "repo": "git@github.com:org/backend.git",
                        "tags": ["api"]
                    }
                }
            }"#,
        )
        .unwrap();

        let tree = config::walk_meta_tree(dir.path(), None).unwrap();
        let result = flatten_with_tag_filter(&tree, &Some("ui".to_string()));

        let has_frontend = result.iter().any(|p| p.contains("frontend"));
        let has_backend = result.iter().any(|p| p.contains("backend"));
        assert!(has_frontend, "Frontend should be included (has 'ui' tag)");
        assert!(!has_backend, "Backend should be excluded (no 'ui' tag)");
    }

    #[test]
    fn test_extended_format_with_optional_path() {
        let dir = tempfile::tempdir().unwrap();
        let meta_path = dir.path().join(".meta.yaml");

        // Extended format without explicit path (should default to project name)
        std::fs::write(
            &meta_path,
            r#"{
                "projects": {
                    "myproject": {
                        "repo": "git@github.com:org/myproject.git",
                        "tags": ["test"]
                    }
                }
            }"#,
        )
        .unwrap();

        let projects = meta_core::config::Workspace::load(&meta_path)
            .unwrap()
            .projects;
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "myproject");
        assert_eq!(projects[0].path, "myproject"); // Path defaults to name
        assert_eq!(projects[0].tags, vec!["test"]);
    }

    #[test]
    fn test_find_meta_config_walks_up_directories() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).unwrap();

        // Create .meta.yaml at root
        let meta_path = dir.path().join(".meta.yaml");
        std::fs::write(&meta_path, r#"{"projects": {}}"#).unwrap();

        // Search from nested directory should find root .meta.yaml
        let result = find_meta_config(&nested, None).unwrap();
        assert!(result.is_some());
        let found_path = result.unwrap();
        assert_eq!(found_path, meta_path);
    }
}
