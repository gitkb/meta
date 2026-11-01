use anyhow::Result;
use clap::{Args, CommandFactory, Parser, Subcommand};
use colored::*;
use loop_lib::run;
use meta_cli::repo_filter::{Filters, Scope};
use meta_core::config::{self, find_meta_config, Workspace};
use meta_core::display::{escape, escape_lines, escape_path};
use meta_core::ExecutionMode;
use std::path::{Path, PathBuf};

mod init;
mod registry;
mod subprocess_plugins;
use meta_cli::worktree;
use meta_plugin_protocol::PluginRequestOptions;
use subprocess_plugins::{SubprocessPlugin, SubprocessPluginManager};

// === CLI Structs ===

const VERSION: &str = include_str!("../../VERSION");

/// meta's options go before the command; every word after the command
/// belongs to it, so `meta git pull --tag v1` gives `--tag v1` to git.
#[derive(Parser)]
#[command(
    // named meta however it was started (meta.exe, a link): its help and
    // errors say `meta`, and meta's own help is told apart by its text
    name = "meta",
    bin_name = "meta",
    author,
    version = VERSION.trim(),
    about,
    long_about = None,
    disable_help_subcommand = true,
    after_help = "meta's options go before the command (meta --dry-run git update); \
                  what follows the command belongs to it."
)]
struct Cli {
    #[arg(long, help = "Add shell aliases to the global .looprc file")]
    add_aliases_to_global_looprc: bool,

    #[arg(
        short,
        long,
        value_name = "FILE",
        help = "Read FILE instead of .meta.yaml, looked for in the current directory and the ones above it"
    )]
    config: Option<PathBuf>,

    #[arg(
        short,
        long,
        value_delimiter = ',',
        help = "Skip these repos: each value is a repo's .meta.yaml name or its whole path from the workspace root (. for the root), never part of one"
    )]
    exclude: Option<Vec<String>>,

    #[arg(
        short,
        long,
        value_delimiter = ',',
        help = "Run in these repos (and in those --tag selects): each value is a repo's .meta.yaml name or its whole path from the workspace root (. for the root), never part of one"
    )]
    include: Option<Vec<String>>,

    #[arg(long, help = "Output results in JSON format")]
    json: bool,

    #[arg(short, long, help = "Enable silent mode")]
    silent: bool,

    #[arg(short, long, action, help = "Enable verbose output")]
    verbose: bool,

    #[arg(
        long,
        short = 't',
        value_name = "TAGS",
        help = "Run in the repos with any of these tags, comma-separated (the root has none: add it with -i .)"
    )]
    tag: Option<String>,

    #[arg(long, short = 'r', help = "Recursively process nested meta repos")]
    recursive: bool,

    #[arg(
        long,
        value_name = "N",
        help = "Maximum depth for recursive processing (default: unlimited)"
    )]
    depth: Option<usize>,

    #[arg(
        long,
        help = "Show what would change, and change nothing (commands that can't preview refuse it)"
    )]
    dry_run: bool,

    #[arg(long, help = "Run commands in parallel")]
    parallel: bool,

    #[arg(long, help = "Run commands sequentially (overrides config default)")]
    sequential: bool,

    #[arg(
        long,
        help = "Inside a worktree set, act on the workspace it was made from instead"
    )]
    primary: bool,

    #[arg(
        long,
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

    /// `--include`, `--exclude` and `--tag`
    fn filters(&self) -> Filters {
        Filters::new(
            self.include.clone(),
            self.exclude.clone(),
            self.tag.as_deref(),
        )
    }

    /// What a plugin is told of meta's options; `parallel` is resolved
    /// against the config the command runs from
    fn request_options(&self, parallel: bool) -> PluginRequestOptions {
        let tags = self.filters().tags;
        PluginRequestOptions {
            json_output: self.json,
            verbose: self.verbose,
            parallel,
            dry_run: self.dry_run,
            silent: self.silent,
            recursive: self.recursive,
            depth: self.depth,
            include_filters: self.include.clone(),
            exclude_filters: self.exclude.clone(),
            tag_filters: (!tags.is_empty()).then_some(tags),
            strict: self.strict,
        }
    }

    /// Where the command runs from: the current directory, or with
    /// `--primary` inside a worktree set, its place in the workspace the set
    /// was made from
    fn start_dir(&self) -> Result<PathBuf> {
        let cwd = std::env::current_dir()?;
        if !self.primary {
            return Ok(cwd);
        }
        Ok(worktree::primary_dir(&cwd)?.unwrap_or(cwd))
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Show workspace context summary
    Context(ContextArgs),
    /// Run a command in each selected repo, as typed (no shell, no plugin)
    #[command(
        after_help = "No shell parses the command: each argument reaches the program as \
                            given, so quotes, $(), ;, | and globs stay literal. For a shell, ask \
                            for one: meta exec -- sh -c 'npm ci && npm test'.\n\n\
                            Examples:\n  meta exec -- cargo fmt --check\n  \
                            meta --include api,web exec -- docker-compose up -d"
    )]
    Exec(ExecArgs),
    /// Print help for meta or for a command (meta help git worktree create)
    Help {
        /// The command, as you would type it after `meta`
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
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

    /// Output the context as JSON
    #[arg(long)]
    json: bool,
}

/// Arguments for `meta exec`
#[derive(Args)]
struct ExecArgs {
    /// The program and its arguments (after `--` when the program starts with `-`)
    #[arg(required = true, trailing_var_arg = true)]
    command: Vec<String>,
}

/// Arguments for `meta init`
#[derive(Args)]
#[command(subcommand_required = true, arg_required_else_help = true)]
struct InitArgs {
    #[command(subcommand)]
    command: InitCommands,
}

#[derive(Subcommand)]
enum InitCommands {
    /// Install Claude Code skills, rules, and hooks for this meta repo
    #[command(
        after_help = "Examples:\n  meta init claude\n  meta init claude --update\n  \
                            meta --dry-run init claude --force"
    )]
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
#[command(subcommand_required = true, arg_required_else_help = true)]
struct PluginArgs {
    #[command(subcommand)]
    command: PluginCommands,
}

#[derive(Subcommand)]
enum PluginCommands {
    /// Search for plugins in the registry
    Search {
        /// Search query
        query: String,
        /// Output the matches as JSON
        #[arg(long)]
        json: bool,
    },
    /// Install a plugin (by registry name, owner/repo[@version] or URL)
    Install {
        /// Plugin name, owner/repo[@version] or archive URL
        name: String,
        /// Install plugin locally to project (.meta/plugins/) instead of globally
        #[arg(long)]
        local: bool,
    },
    /// List installed plugins
    List {
        /// List only project-local plugins
        #[arg(long)]
        local: bool,
        /// Output name, version, description and path of each as JSON
        #[arg(long)]
        json: bool,
    },
    /// Uninstall a plugin
    Uninstall {
        /// Plugin name
        name: String,
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

/// meta's help, with a line for each plugin's command word (each plugin is
/// started for its description and version) unless a built-in command has
/// that name. Plugins name and describe themselves, so their values are
/// escaped.
fn help_with_plugins(plugins: &SubprocessPluginManager) -> String {
    let mut cmd = Cli::command();
    for plugin in plugins.all() {
        // shown escaped; a word whose escaped form is taken (by a built-in
        // or another plugin's) is shown once
        let shown = escape(&plugin.word).into_owned();
        if cmd.find_subcommand(&shown).is_some() {
            continue;
        }
        let description = plugin
            .info
            .description
            .as_deref()
            .unwrap_or("No description available");
        let about = format!(
            "{} [plugin: {} v{}]",
            escape(description),
            escape(&plugin.word),
            escape(&plugin.info.version)
        );
        cmd = cmd.subcommand(clap::Command::new(shown).about(about));
    }
    cmd.render_help().to_string()
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

    // meta's own --help also lists the commands plugins add, so it is
    // answered here, before anything else runs; every other result of
    // parsing is reported as usual
    let cli: Cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if is_root_help(&e) => {
            check_and_warn_orphan();
            let plugins = SubprocessPluginManager::new(false)?;
            return subprocess_plugins::print_output(&help_with_plugins(&plugins));
        }
        Err(_) => meta_core::args::parse(),
    };

    // Check for orphaned nested meta repo and warn the user
    check_and_warn_orphan();

    // Where plugins are; nothing is started until a command needs one
    let subprocess_plugins = SubprocessPluginManager::new(cli.verbose)?;

    // `--tag "$UNSET"` is a slip, not "every repo"
    anyhow::ensure!(
        cli.tag.is_none() || !cli.filters().tags.is_empty(),
        "--tag '{}' names no tag",
        escape(cli.tag.as_deref().unwrap_or_default())
    );

    if cli.add_aliases_to_global_looprc {
        anyhow::ensure!(
            cli.command.is_none(),
            "--add-aliases-to-global-looprc imports your shell's aliases and runs no command"
        );
        return loop_lib::import_aliases(cli.dry_run);
    }

    match &cli.command {
        None => subprocess_plugins::print_output(&help_with_plugins(&subprocess_plugins)),
        Some(Commands::Help { command }) => help(&cli, command, &subprocess_plugins),
        // it reads the nearest workspace only, so -r would go unheard
        Some(Commands::Context(_)) if cli.recursive || cli.depth.is_some() => anyhow::bail!(
            "meta context shows the nearest workspace only; it does not take --recursive or --depth"
        ),
        Some(Commands::Context(args)) => meta_cli::context::handle_context(
            cli.json || args.json,
            args.no_status,
            args.no_cache,
            cli.mode(),
            cli.verbose,
            &cli.start_dir()?,
            &cli.filters(),
        ),
        Some(Commands::Init(InitArgs {
            command: InitCommands::Claude { force, update },
        })) => init::init_claude(*force, *update, cli.mode(), cli.verbose),
        Some(Commands::Plugin(args)) => {
            handle_plugin_command(&args.command, cli.mode(), cli.json, &subprocess_plugins)
        }
        Some(Commands::Exec(args)) => dispatch(&cli, &args.command, None),
        Some(Commands::External(args)) => {
            // Only the plugin serving this word is started
            let plugin = match args.first() {
                Some(word) => subprocess_plugins.find(word)?,
                None => None,
            };
            let Some(plugin) = plugin else {
                unrecognized_command_error(args);
            };
            plugin_command(&cli, args, &plugin)
        }
    }
}

/// Whether `error` is clap showing meta's own help (`meta --help`, also
/// before a command: `meta --help exec`), not a command's
fn is_root_help(error: &clap::Error) -> bool {
    let mut root = Cli::command();
    root.build();
    let shown = error.render().to_string();
    error.kind() == clap::error::ErrorKind::DisplayHelp
        && (shown == root.render_help().to_string() || shown == root.render_long_help().to_string())
}

/// `meta help [COMMAND...]`: meta's help, a built-in command's, or the
/// plugin command's, as `meta COMMAND... --help` prints it.
fn help(cli: &Cli, words: &[String], plugins: &SubprocessPluginManager) -> Result<()> {
    let Some(first) = words.first() else {
        return subprocess_plugins::print_output(&help_with_plugins(plugins));
    };
    let mut cmd = Cli::command();
    // built, so the subcommands carry their names and meta's help settings
    cmd.build();
    if cmd.find_subcommand(first).is_some() {
        let mut found = &mut cmd;
        for (i, word) in words.iter().enumerate() {
            found = match found.find_subcommand_mut(word) {
                Some(sub) => sub,
                None => anyhow::bail!(
                    "meta {} has no command '{}'",
                    escape(&words[..i].join(" ")),
                    escape(word)
                ),
            };
        }
        return subprocess_plugins::print_output(&found.render_help().to_string());
    }
    let Some(plugin) = plugins.find(first)? else {
        unrecognized_command_error(words);
    };
    // The command's words only: what follows `--` is its arguments, and
    // help never runs the command
    let args: Vec<String> = words
        .iter()
        .take_while(|word| *word != "--")
        .cloned()
        .chain(["--help".to_string()])
        .collect();
    plugin_command(cli, &args, &plugin)
}

/// `meta <word> ...`, served by `plugin`: its help, or the command in the
/// selected repos.
fn plugin_command(cli: &Cli, args: &[String], plugin: &SubprocessPlugin) -> Result<()> {
    // A bare `meta <word>` and `meta <word> --help` print the plugin's
    // help; nested help requests reach the plugin's command, without its
    // repos. A --help after `--` is an argument for the command.
    let wants_help = args
        .iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--help" || a == "-h");
    let namespace_help =
        args.len() == 2 && matches!(args.get(1).map(String::as_str), Some("--help" | "-h"));
    if args.len() == 1 || namespace_help {
        let help = meta_plugin_protocol::help_text(&plugin.info);
        return subprocess_plugins::print_output(&format!("{help}\n"));
    }
    if wants_help {
        let options = cli.request_options(cli.parallel);
        return plugin.execute(args, &[], None, &cli.start_dir()?, options);
    }
    dispatch(cli, args, Some(plugin))
}

// === Command Dispatch (shared by exec and external) ===

/// The repos a command runs in, chosen once
struct Target {
    /// The workspace or worktree set they belong to, shown as `.`
    root: PathBuf,
    dirs: Vec<PathBuf>,
    /// `--parallel` or `--sequential`, else the config's default
    parallel: bool,
}

/// Run a command across the selected repos.
///
/// `meta exec` (no plugin) runs the command as typed and never offers it to
/// a plugin, so a plugin's word (e.g. meta-cargo's `cargo`) cannot shadow a
/// raw command. Other commands (e.g. `meta git status`) go to the plugin
/// serving their first word.
fn dispatch(cli: &Cli, command_args: &[String], plugin: Option<&SubprocessPlugin>) -> Result<()> {
    let start = cli.start_dir()?;
    // `git clone` makes the workspace, so it needs none
    let is_git_clone = command_args.first().is_some_and(|s| s == "git")
        && command_args.get(1).is_some_and(|s| s == "clone");
    if let Some(plugin) = plugin.filter(|_| is_git_clone) {
        return plugin.execute(command_args, &[], None, &start, cli.request_options(false));
    }

    let target = target(cli, &start)?;
    if cli.verbose {
        println!(
            "Executing command: {}",
            escape(&loop_lib::display_command(command_args))
        );
    }
    match plugin {
        None => {
            let config = loop_lib::LoopConfig {
                directories: target.dirs,
                verbose: cli.verbose,
                silent: cli.silent,
                parallel: target.parallel,
                dry_run: cli.dry_run,
                json_output: cli.json,
                aliases: loop_lib::aliases()?,
                root_dir: Some(target.root),
                ..Default::default()
            };
            run(&config, command_args)
        }
        Some(plugin) => plugin.execute(
            command_args,
            &target.dirs,
            Some(&target.root),
            &start,
            cli.request_options(target.parallel),
        ),
    }
}

/// The repos `cli`'s filters select from `start`: the checkouts of the
/// worktree set it is in (unless `--primary`), else the repos of the
/// workspace whose config is nearest at or above it (with `--recursive`, of
/// the nested meta repos too).
fn target(cli: &Cli, start: &Path) -> Result<Target> {
    let filters = cli.filters();
    // --parallel wins, then --sequential, then the default of the config
    // the command runs from (parallel without one)
    let parallel = |workspace: Option<&Workspace>| {
        cli.parallel || !cli.sequential && workspace.is_none_or(|w| w.defaults.parallel)
    };

    // Inside a set, the command runs in the set's checkouts
    if let Some((name, set_dir)) =
        worktree::detect_worktree_location(start).filter(|_| !cli.primary)
    {
        let members: Vec<_> = worktree::discover_worktree_members(&set_dir)?
            .into_iter()
            .map(|member| member.path)
            .collect();
        // Inside a set, a command never falls back to the workspace's own
        // checkouts: they aren't the ones the caller is in. A plain clone
        // that is merely under a `.worktrees` directory is no set.
        if members.is_empty() && worktree::holds_meta_checkout(&set_dir) {
            anyhow::bail!(
                "No checkouts of worktree set '{}' found in {}. Run from a set meta made, or pass --primary to run in the workspace.",
                escape(&name),
                escape_path(&set_dir)
            );
        }
        if !members.is_empty() {
            if cli.verbose {
                eprintln!(
                    "Detected worktree context: '{}' ({} repos)",
                    escape(&name),
                    members.len()
                );
            }
            let workspace = worktree::set_config(&set_dir, &members, cli.config.as_ref())?
                .map(|config| Workspace::load(&config))
                .transpose()?;
            if cli.verbose {
                match &workspace {
                    Some(w) => eprintln!("Using config {}", escape_path(&w.config_path)),
                    None => eprintln!(
                        "{} No .meta.yaml found for worktree '{}'. Tags, plugins, and dependency features unavailable.",
                        "warning:".yellow().bold(),
                        escape(&name)
                    ),
                }
            }
            let dirs = worktree::select_in_set(&set_dir, &members, workspace.as_ref(), &filters)?;
            return Ok(Target {
                parallel: parallel(workspace.as_ref()),
                root: set_dir,
                dirs,
            });
        }
    }

    let Some(workspace) = Workspace::find(start, cli.config.as_ref())? else {
        let config_name = cli
            .config
            .as_ref()
            .map(|p| escape_path(p))
            .unwrap_or_else(|| config::META_CONFIG_NAME.to_string());
        anyhow::bail!(
            "Could not find meta config file '{config_name}'\nSearched from {} up to root",
            escape_path(start)
        );
    };
    if cli.verbose {
        eprintln!(
            "Resolved config file path: {}",
            escape_path(&workspace.config_path)
        );
    }
    // From the config already loaded (the default or --config's), into the
    // nested meta repos
    let tree = if cli.recursive {
        config::walk_projects(&workspace.dir, &workspace.projects, cli.depth)?
    } else {
        Vec::new()
    };
    let scope = Scope::workspace(
        &workspace.dir,
        Some(&workspace.config_path),
        &workspace.projects,
        &tree,
    );
    Ok(Target {
        dirs: scope.select(&filters)?,
        parallel: parallel(Some(&workspace)),
        root: workspace.dir,
    })
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
    command: &PluginCommands,
    mode: ExecutionMode,
    json: bool,
    subprocess_plugins: &SubprocessPluginManager,
) -> Result<()> {
    use registry::{PluginInstaller, RegistryClient, PLUGIN_PREFIX};

    // These change installed plugins and have no preview
    if mode.is_preview() {
        let changing = match command {
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
        PluginCommands::Search { query, json: own } => {
            let client = RegistryClient::new()?;
            let results = client.search(query)?;

            if json || *own {
                println!("{}", serde_json::to_string_pretty(&results)?);
            } else if results.is_empty() {
                println!("No plugins found matching '{}'", escape(query));
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
            use registry::GitHubShorthand;
            let installer = create_installer(*local)?;
            let location = format_plugin_location(*local);

            // Detect input type and route accordingly
            if name.starts_with("http://") || name.starts_with("https://") {
                // Direct URL install
                let plugin_name = installer.install_from_url(name)?;
                if !json {
                    println!(
                        "Successfully installed {} to {location}",
                        escape(&plugin_name)
                    );
                }
            } else if let Some(shorthand) = GitHubShorthand::parse(name) {
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
                match client.resolve_plugin_source(name) {
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
                        let metadata = client.fetch_plugin_metadata(name)?;
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
        PluginCommands::List { local, json: own } => {
            let json = json || *own;
            if *local {
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
                let plugins = subprocess_plugins.all();

                if json {
                    let json_plugins: Vec<_> = plugins
                        .iter()
                        .map(|plugin| {
                            serde_json::json!({
                                "name": plugin.word,
                                "version": plugin.info.version,
                                "description": plugin.info.description.as_deref()
                                    .unwrap_or("No description available"),
                                "path": plugin.path.display().to_string()
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
                    for plugin in &plugins {
                        println!(
                            "{:<12} {:<12} {}",
                            escape(&plugin.word),
                            escape(&plugin.info.version),
                            escape_path(&plugin.path)
                        );
                    }
                }
            }
        }
        PluginCommands::Uninstall { name, local } => {
            let installer = create_installer(*local)?;
            let location = format_plugin_location(*local);
            installer.uninstall(name)?;

            if !json {
                println!("Successfully uninstalled {} from {location}", escape(name));
            }
        }
        PluginCommands::Update { name, local, check } => {
            let (check, local) = (*check, *local);
            let installer = create_installer(local)?;
            let location = format_plugin_location(local);

            if let Some(plugin_name) = name {
                // Update specific plugin
                match installer.check_update(plugin_name)? {
                    Some((current, latest)) => {
                        if check {
                            if !json {
                                println!(
                                    "Update available for {}: {} → {}",
                                    escape(plugin_name),
                                    escape(&current),
                                    escape(&latest)
                                );
                            }
                        } else {
                            let updated = installer.update_plugin(plugin_name)?;
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
                            println!("{} is already up to date", escape(plugin_name));
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

/// Print unrecognized command error with suggestion and help, then exit.
fn unrecognized_command_error(command_args: &[String]) -> ! {
    let first_cmd = command_args.first().map(|s| s.as_str()).unwrap_or("");
    eprintln!(
        "{}: unrecognized command '{}'",
        "error".red().bold(),
        escape(first_cmd)
    );
    let command_str = escape(&loop_lib::display_command(command_args)).into_owned();
    eprintln!();
    eprintln!("To run '{command_str}' across all repos:");
    eprintln!("    meta exec -- {command_str}");
    eprintln!();
    // Listing the plugins' commands would start every plugin; a typo
    // starts none
    eprintln!("Plugins add more commands: meta --help lists them.");
    eprintln!();
    eprint!("{}", Cli::command().render_help());
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
        assert_eq!(repo1.repo, Some("git@github.com:org/repo1.git".into()));
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
        assert_eq!(api.repo, Some("git@github.com:org/api.git".into()));
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
        assert_eq!(web_app.repo, Some("git@github.com:org/web-app.git".into()));

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
