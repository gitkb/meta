use anyhow::Result;
use clap::{Args, CommandFactory, Parser, Subcommand};
use meta_cli::repo_filter::{Filters, Reach};
use meta_core::config::{self, find_meta_config, Workspace};
use meta_core::display::{escape, escape_lines, escape_path};
use meta_core::ExecutionMode;
use meta_core::{errln, out, outln};
use std::path::{Path, PathBuf};

mod init;
mod registry;
mod subprocess_plugins;
use meta_cli::{run_list, worktree};
use meta_plugin_protocol::PluginRequestOptions;
use subprocess_plugins::{SubprocessPlugin, SubprocessPluginManager};

// === CLI Structs ===

const VERSION: &str = include_str!("../../VERSION");

/// The version, and for a build from a git checkout its commit and date:
/// `0.2.22 (git 1c96c7e 2026-11-03)`
fn version() -> String {
    format!("{}{}", VERSION.trim(), env!("META_BUILD_COMMIT"))
}

/// meta's options go before the command; every word after the command
/// belongs to it, so `meta git pull --tag v1` gives `--tag v1` to git.
#[derive(Parser)]
#[command(
    // named meta however it was started (meta.exe, a link): its help and
    // errors say `meta`, and meta's own help is told apart by its text
    name = "meta",
    bin_name = "meta",
    author,
    version = version(),
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

    #[arg(
        long,
        short = 'r',
        help = "Also the meta repos nested under the nearest .meta.yaml (at the workspace root, the same as -A)"
    )]
    recursive: bool,

    #[arg(
        long,
        short = 'A',
        help = "The whole workspace, from its outermost .meta.yaml, wherever you are (in a worktree set: the whole set)"
    )]
    all: bool,

    #[arg(
        long,
        value_name = "N",
        help = "How many levels of nested meta repos -r or -A goes down, from where it starts (default: unlimited)"
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
            // an older plugin, which knows no `all`, reaches at least -r's
            recursive: self.recursive || self.all,
            all: self.all,
            depth: self.depth,
            include_filters: self.include.clone(),
            exclude_filters: self.exclude.clone(),
            tag_filters: (!tags.is_empty()).then_some(tags),
            strict: self.strict,
            meta_options: meta_options(),
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
    /// Show workspace context summary (meta -r context: nested meta repos' projects too)
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

/// The two kinds of extension meta runs, told apart
const PLUGIN_KINDS: &str =
    "A meta plugin (meta-<word>) adds or adapts `meta <word> ...`. A tool's \
     own extension (git-<name>) is just part of that tool, and runs through meta as typed: \
     `meta git <name>` runs `git <name>` in each repo. `meta plugin list` shows both.";

/// Arguments for `meta plugin`
#[derive(Args)]
#[command(
    subcommand_required = true,
    arg_required_else_help = true,
    after_help = PLUGIN_KINDS
)]
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
    },
    /// Install a plugin (by registry name, owner/repo[@version] or URL)
    Install {
        /// Plugin name, owner/repo[@version] or archive URL
        name: String,
        /// Install plugin locally to project (.meta/plugins/) instead of globally
        #[arg(long)]
        local: bool,
    },
    /// List the plugins meta finds, and the tool extensions on PATH they run
    #[command(after_help = PLUGIN_KINDS)]
    List {
        /// List only project-local plugins (meta --json: name, version,
        /// description and path of each)
        #[arg(long)]
        local: bool,
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

/// meta's help, with an entry for each plugin's command word (each plugin is
/// started for its info) unless a built-in command has that name: its
/// description and version, its own commands, and what becomes of the
/// rest. Plugins name and describe themselves, so their values are escaped.
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
        let mut about = format!(
            "{} [plugin: {} v{}]",
            escape(description),
            escape(&plugin.word),
            escape(&plugin.info.version)
        );
        if let Some(help) = &plugin.info.help {
            if !help.commands.is_empty() {
                let names: Vec<_> = help.commands.keys().map(|name| escape(name)).collect();
                about.push_str(&format!("\n{}", names.join(", ")));
            }
            if let Some(rule) = help.as_typed_rule() {
                about.push_str(&format!("\n{}", escape(&rule)));
            }
        }
        cmd = cmd.subcommand(clap::Command::new(shown).about(about));
    }
    cmd.render_help().to_string()
}

// === Main Entry Point ===

fn main() -> std::process::ExitCode {
    meta_core::exit::run(|| {
        if let Err(e) = meta_core::children::own() {
            meta_core::out::warn(format!(
                "cannot pass signals on to the programs meta starts: {e}"
            ));
        }
        run_cli()
    })
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
            return Ok(out!("{}", help_with_plugins(&plugins))?);
        }
        Err(_) => meta_core::args::parse()?,
    };
    meta_core::out::set_json(cli.json);

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
        None => Ok(out!("{}", help_with_plugins(&subprocess_plugins))?),
        Some(Commands::Help { command }) => help(&cli, command, &subprocess_plugins),
        Some(Commands::Context(args)) => {
            let workspace = Workspace::find(&cli.start_dir()?, None)?
                .ok_or_else(|| anyhow::anyhow!("Not a meta workspace (no .meta.yaml found)"))?;
            meta_cli::context::handle_context(
                cli.json,
                args.no_status,
                cli.verbose,
                Reach::of(workspace, cli.recursive, cli.all, cli.depth)?,
                &cli.filters(),
                with_all,
            )
        }
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
                return Err(unrecognized_command_error(args, &subprocess_plugins, true));
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
/// plugin command's, as `meta <command> --help` prints it.
fn help(cli: &Cli, words: &[String], plugins: &SubprocessPluginManager) -> Result<()> {
    let Some(first) = words.first() else {
        return Ok(out!("{}", help_with_plugins(plugins))?);
    };
    let mut cmd = Cli::command();
    // built, so the subcommands carry their names and meta's help settings
    cmd.build();
    if cmd.find_subcommand(first).is_some() {
        let mut found = &mut cmd;
        for (i, word) in words.iter().enumerate() {
            found = match found.find_subcommand_mut(word) {
                Some(sub) => sub,
                None => {
                    return Err(meta_core::exit::Usage(format!(
                        "meta {} has no command '{}'",
                        escape(&words.get(..i).unwrap_or_default().join(" ")),
                        escape(word)
                    ))
                    .into())
                }
            };
        }
        return Ok(out!("{}", found.render_help())?);
    }
    let Some(plugin) = plugins.find(first)? else {
        return Err(unrecognized_command_error(words, plugins, false));
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
        let help = meta_plugin_protocol::help_text(&plugin.word, &plugin.info);
        return Ok(outln!("{}", help.trim_end())?);
    }
    if wants_help {
        return command_help(cli, args, plugin);
    }
    dispatch(cli, args, Some(plugin))
}

/// `meta <word> <verb> ... --help`: a line saying who answers the command,
/// as the plugin's info declares it, then the command's help: the plugin's
/// for one of its own, meta's for one it runs as typed.
fn command_help(cli: &Cli, args: &[String], plugin: &SubprocessPlugin) -> Result<()> {
    use meta_plugin_protocol::Verb;
    let verb = args.get(1).map(String::as_str).unwrap_or_default();
    let named = loop_lib::display_command(&[plugin.word.clone(), verb.to_string()]);
    let named = escape(&named);
    match plugin.info.verb(verb) {
        Verb::Own => {
            let tool = plugin
                .info
                .help
                .as_ref()
                .and_then(|h| h.runs_as_typed.as_deref());
            // --json keeps stdout the plugin's JSON
            if !cli.json {
                let owner = match tool {
                    Some(tool) => format!(
                        "`meta {named}` is adapted by meta-{}: meta's own command, not {} run as typed.",
                        escape(&plugin.word),
                        escape(tool)
                    ),
                    None => format!("`meta {named}` is a command of meta-{}.", escape(&plugin.word)),
                };
                outln!("{owner}\n")?;
            }
            let options = cli.request_options(cli.parallel);
            plugin.execute(args, &[], None, &cli.start_dir()?, options)
        }
        Verb::AsTyped(tool) => {
            let help = as_typed_help(cli, args, plugin, tool);
            match cli.json {
                true => Ok(outln!(
                    "{}",
                    serde_json::json!({"success": true, "output": help})
                )?),
                false => Ok(out!("{help}")?),
            }
        }
        Verb::Unknown => {
            let options = cli.request_options(cli.parallel);
            plugin.execute(args, &[], None, &cli.start_dir()?, options)
        }
    }
}

/// The help of `args` (`meta <word> <verb> ... --help`), a command that
/// runs as typed with `tool` in each selected repo: which repos those are
/// now, meta's options that choose them, and where `tool`'s own help is.
fn as_typed_help(cli: &Cli, args: &[String], plugin: &SubprocessPlugin, tool: &str) -> String {
    // the command as typed, without its help flags
    let typed: Vec<String> = args
        .iter()
        .take_while(|arg| *arg != "--")
        .filter(|arg| !matches!(arg.as_str(), "--help" | "-h"))
        .cloned()
        .collect();
    let run: Vec<String> = std::iter::once(tool.to_string())
        .chain(typed.iter().skip(1).cloned())
        .collect();
    let (typed, run, word) = (
        escape(&loop_lib::display_command(&typed)).into_owned(),
        escape(&loop_lib::display_command(&run)).into_owned(),
        escape(&plugin.word),
    );
    let tool = escape(tool);
    let scope = match cli.start_dir().and_then(|start| target(cli, &start)) {
        Ok(target) => {
            let names: Vec<String> = target
                .dirs
                .iter()
                .map(|dir| match dir.strip_prefix(&target.root) {
                    Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
                    Ok(rel) => escape_path(rel),
                    Err(_) => escape_path(dir),
                })
                .collect();
            format!("{} repo(s): {}", names.len(), names.join(", "))
        }
        Err(e) => format!(
            "no repos here ({})",
            format!("{e:#}").lines().next().unwrap_or_default()
        ),
    };
    let note = plugin
        .info
        .help
        .as_ref()
        .and_then(|help| help.note.as_ref())
        .map(|note| format!("\n{}\n", escape(note)))
        .unwrap_or_default();
    format!(
        "`meta {typed}` runs as typed (`{run}`) in each selected repo.\n\n\
         Usage: meta [META_OPTIONS] {typed} [{}_ARGS...]\n\n\
         Current scope: {scope}\n\n\
         Meta options (before `{word}`):\n{}\n\n\
         Examples:\n  meta {typed}\n  meta --dry-run {typed}\n  meta --recursive --dry-run {typed}\n\n\
         For {tool}'s own options: {run} --help\n{note}",
        tool.to_uppercase(),
        selection_options_help().trim_end()
    )
}

/// The help of meta's options that choose the repos a command runs in, and
/// how: from meta's own definitions, so it says what meta takes.
fn selection_options_help() -> String {
    const SHOWN: [&str; 9] = [
        "dry_run",
        "recursive",
        "depth",
        "include",
        "exclude",
        "tag",
        "parallel",
        "sequential",
        "primary",
    ];
    let options: Vec<clap::Arg> = Cli::command()
        .get_arguments()
        .filter(|arg| SHOWN.contains(&arg.get_id().as_str()))
        .cloned()
        .collect();
    clap::Command::new("meta")
        .disable_help_flag(true)
        .disable_version_flag(true)
        .help_template("{options}")
        .args(options)
        .render_help()
        .to_string()
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
    // `git clone` makes the workspace, so it needs none; worktree commands
    // find their repos in the set they name (exec applies meta's selection
    // there)
    let finds_its_repos = command_args.first().is_some_and(|s| s == "git")
        && command_args
            .get(1)
            .is_some_and(|s| s == "clone" || s == "worktree");
    // (meta's --parallel as typed: no config's default applies to them)
    if let Some(plugin) = plugin.filter(|_| finds_its_repos) {
        let options = cli.request_options(cli.parallel);
        return plugin.execute(command_args, &[], None, &start, options);
    }

    let target = target(cli, &start)?;
    if cli.verbose {
        errln!(
            "Executing command: {}",
            escape(&loop_lib::display_command(command_args))
        );
    }
    match plugin {
        None => {
            let config = loop_lib::LoopConfig {
                verbose: cli.verbose,
                silent: cli.silent,
                parallel: target.parallel,
                dry_run: cli.dry_run,
                json_output: cli.json,
                aliases: loop_lib::aliases()?,
                root_dir: Some(target.root.clone()),
                outer: Some(outer(&["exec", "--"])),
                ..Default::default()
            };
            // only in the selected projects that are repositories
            let commands = target
                .dirs
                .iter()
                .map(|dir| loop_lib::DirCommand {
                    dir: dir.clone(),
                    argv: command_args.to_vec(),
                    env: None,
                    refused: None,
                })
                .collect();
            let commands = run_list::in_repos(commands, &target.dirs, Some(&target.root));
            loop_lib::run_commands(&config, &commands)
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

/// meta's options, and what of this process's arguments is them
fn meta_options() -> meta_core::args::MetaOptions {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    meta_core::args::MetaOptions::of(&Cli::command(), &args)
}

/// meta, as the program that runs a command in each repo: its options, so a
/// command that rejected one is told it goes before the command (`before`:
/// the words between meta's options and the command).
fn outer(before: &[&str]) -> loop_lib::Outer {
    loop_lib::Outer {
        meta: meta_options(),
        before: before.iter().map(ToString::to_string).collect(),
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
    let location = match cli.primary {
        true => None,
        false => worktree::detect_worktree_location(start)?,
    };
    if let Some((name, set_dir)) = location {
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
                errln!(
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
                    Some(w) => errln!("Using config {}", escape_path(&w.config_path)),
                    None => meta_core::out::warn(format!(
                        "No .meta.yaml found for worktree '{}'. Tags, plugins, and dependency features unavailable.",
                        escape(&name)
                    )),
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
        errln!(
            "Resolved config file path: {}",
            escape_path(&workspace.config_path)
        );
    }
    // From the config already loaded (the default or --config's): -r into
    // the meta repos nested under it, -A from the outermost one
    let parallel = parallel(Some(&workspace));
    let reach = Reach::of(workspace, cli.recursive, cli.all, cli.depth)?;
    Ok(Target {
        dirs: reach.select(&filters, with_all)?,
        parallel,
        root: reach.root,
    })
}

/// The line as typed with meta's `-r` in it as `-A`, which reaches the
/// whole workspace.
fn with_all() -> String {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let typed = meta_options().typed.len();
    let line: Vec<String> = std::iter::once("meta".to_string())
        .chain(args.iter().enumerate().map(|(i, arg)| {
            match i < typed && (arg == "-r" || arg == "--recursive") {
                true => "-A".to_string(),
                false => arg.clone(),
            }
        }))
        .collect();
    loop_lib::display_command(&line)
}

// === Plugin Management ===

/// The extensions on PATH of each tool a plugin runs as typed (`git-foo`
/// for meta-git's git), by the command that reaches them: they are part of
/// the tool, not meta plugins. One the plugin's own command shadows is
/// left out: meta never runs it.
fn tool_extensions(plugins: &[SubprocessPlugin]) -> Result<()> {
    let found: Vec<(&SubprocessPlugin, Vec<String>)> = plugins
        .iter()
        .filter_map(|plugin| {
            let help = plugin.info.help.as_ref()?;
            let names: Vec<String> =
                subprocess_plugins::extensions_on_path(help.runs_as_typed.as_deref()?)
                    .into_iter()
                    .filter(|name| !help.commands.contains_key(name))
                    .collect();
            (!names.is_empty()).then_some((plugin, names))
        })
        .collect();
    if found.is_empty() {
        return Ok(());
    }
    outln!("\nTool extensions on PATH, run as typed (part of their tool, not meta plugins):\n")?;
    for (plugin, names) in found {
        let names: Vec<_> = names.iter().map(|name| escape(name)).collect();
        let command = format!("meta {} <name>", escape(&plugin.word));
        outln!("{command:<20} {}", names.join(", "))?;
    }
    Ok(())
}

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
        PluginCommands::Search { query } => {
            let client = RegistryClient::new()?;
            let results = client.search(query)?;

            if json {
                outln!("{}", serde_json::to_string_pretty(&results)?)?;
            } else if results.is_empty() {
                outln!("No plugins found matching '{}'", escape(query))?;
            } else {
                outln!("Found {} plugin(s):", results.len())?;
                for plugin in results {
                    outln!(
                        "  {} v{} - {}",
                        escape(&plugin.name),
                        escape(&plugin.version),
                        escape(&plugin.description)
                    )?;
                    outln!("    by {}", escape(&plugin.author))?;
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
                    outln!(
                        "Successfully installed {} to {location}",
                        escape(&plugin_name)
                    )?;
                }
            } else if let Some(shorthand) = GitHubShorthand::parse(name) {
                // GitHub shorthand install (user/repo[@version])
                let plugin_name = installer.install_from_github(&shorthand)?;
                if !json {
                    outln!(
                        "Successfully installed {} to {location}",
                        escape(&plugin_name)
                    )?;
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
                                outln!(
                                    "Successfully installed {} from {} to {location}",
                                    escape(&plugin_name),
                                    escape(&source)
                                )?;
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
                            outln!(
                                "Successfully installed {} v{} ({}) to {location}",
                                escape(&metadata.name),
                                escape(&metadata.version),
                                escape(&installed.join(", "))
                            )?;
                        }
                    }
                }
            }
        }
        PluginCommands::List { local } => {
            if *local {
                let installer = PluginInstaller::new_local()?;
                let plugins = installer.list_plugins_detailed()?;
                if json {
                    outln!("{}", serde_json::to_string_pretty(&plugins)?)?;
                } else if plugins.is_empty() {
                    outln!("No project-local plugins installed")?;
                } else {
                    outln!("Project-local plugins ({}):", plugins.len())?;
                    outln!()?;
                    outln!("{:<12} {:<12} PATH", "NAME", "VERSION")?;
                    outln!("{}", "-".repeat(70))?;
                    for plugin in plugins {
                        let name = plugin
                            .name
                            .strip_prefix(PLUGIN_PREFIX)
                            .unwrap_or(&plugin.name);
                        let version = plugin.version.as_deref().unwrap_or("-");
                        let path = installer.plugins_dir().join(&plugin.name);
                        outln!(
                            "{:<12} {:<12} {}",
                            escape(name),
                            escape(version),
                            escape_path(&path)
                        )?;
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
                    outln!("{}", serde_json::to_string_pretty(&json_plugins)?)?;
                } else if plugins.is_empty() {
                    outln!("No plugins installed")?;
                } else {
                    outln!("Installed plugins ({}):", plugins.len())?;
                    outln!()?;
                    outln!("{:<12} {:<12} PATH", "NAME", "VERSION")?;
                    outln!("{}", "-".repeat(70))?;
                    for plugin in &plugins {
                        outln!(
                            "{:<12} {:<12} {}",
                            escape(&plugin.word),
                            escape(&plugin.info.version),
                            escape_path(&plugin.path)
                        )?;
                    }
                    tool_extensions(&plugins)?;
                }
            }
        }
        PluginCommands::Uninstall { name, local } => {
            let installer = create_installer(*local)?;
            let location = format_plugin_location(*local);
            installer.uninstall(name)?;

            if !json {
                outln!("Successfully uninstalled {} from {location}", escape(name))?;
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
                                outln!(
                                    "Update available for {}: {} → {}",
                                    escape(plugin_name),
                                    escape(&current),
                                    escape(&latest)
                                )?;
                            }
                        } else {
                            let updated = installer.update_plugin(plugin_name)?;
                            if !json {
                                outln!(
                                    "Successfully updated {} from {} to {} in {}",
                                    escape(&updated),
                                    escape(&current),
                                    escape(&latest),
                                    escape(location)
                                )?;
                            }
                        }
                    }
                    None => {
                        if !json {
                            outln!("{} is already up to date", escape(plugin_name))?;
                        }
                    }
                }
            } else {
                // Update every plugin meta installed. One that can't be
                // checked or updated is reported and the rest still are;
                // then the command fails naming them.
                let mut updates_available = Vec::new();
                let mut updated_count = 0;
                let mut failed = Vec::new();
                let mut report = |name: &str, e: anyhow::Error| {
                    errln!("{}", escape_lines(&format!("{e:#}")));
                    failed.push(name.to_string());
                };

                for name in installer.installed_words()? {
                    match installer.check_update(&name) {
                        Ok(Some((current, latest))) => {
                            updates_available.push((name, current, latest))
                        }
                        Ok(None) => {}
                        Err(e) => report(&name, e),
                    }
                }

                if updates_available.is_empty() {
                    if !json && failed.is_empty() {
                        outln!("All plugins are up to date")?;
                    }
                } else if check {
                    if !json {
                        outln!("Updates available:")?;
                        for (name, current, latest) in &updates_available {
                            outln!(
                                "  {} {} → {}",
                                escape(name),
                                escape(current),
                                escape(latest)
                            )?;
                        }
                    }
                } else {
                    for (name, current, latest) in &updates_available {
                        match installer.update_plugin(name) {
                            Ok(_) => {
                                if !json {
                                    outln!(
                                        "Updated {} from {} to {}",
                                        escape(name),
                                        escape(current),
                                        escape(latest)
                                    )?;
                                }
                                updated_count += 1;
                            }
                            Err(e) => report(
                                name,
                                e.context(format!("Failed to update {}", escape(name))),
                            ),
                        }
                    }
                    if !json {
                        outln!(
                            "\nUpdated {} plugin(s) in {}",
                            updated_count,
                            escape(location)
                        )?;
                    }
                }
                if !failed.is_empty() {
                    let not = if check { "checked" } else { "updated" };
                    anyhow::bail!("Plugins not {not}: {}", escape(&failed.join(", ")));
                }
            }
        }
    }

    Ok(())
}

// === Helpers ===

/// The usage error for a command meta doesn't know, with a suggestion made by changing the line as typed at the command word alone,
/// so everything else typed stays as it was: the command its first word is
/// a typo of, else (unless `meta help` was asked) `exec --` before it, to run
/// it in each repo. A program on PATH (`cp`) is no typo, and a plugin word is
/// suggested only once its file answers as a plugin: the file the suggestion
/// would start.
fn unrecognized_command_error(
    command_args: &[String],
    plugins: &SubprocessPluginManager,
    run_it: bool,
) -> anyhow::Error {
    let first_cmd = command_args.first().map(|s| s.as_str()).unwrap_or("");
    let cli = Cli::command();
    let builtins: Vec<&str> = cli.get_subcommands().map(|sub| sub.get_name()).collect();
    // Plugins by their file names: none is started to list them
    let words = plugins.words();
    let near = (!subprocess_plugins::on_path(first_cmd))
        .then(|| {
            meta_core::args::nearest(
                first_cmd,
                builtins
                    .iter()
                    .copied()
                    .chain(words.iter().map(String::as_str)),
            )
        })
        .flatten()
        .filter(|word| builtins.contains(word) || matches!(plugins.find(word), Ok(Some(_))));
    // the line as typed, whose words from the command word on are command_args
    let typed: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let at = typed.len().saturating_sub(command_args.len());
    let edited = |command: &[&str]| -> String {
        let argv: Vec<String> = typed
            .iter()
            .take(at)
            .cloned()
            .chain(command.iter().map(|word| word.to_string()))
            .chain(typed.iter().skip(at + 1).cloned())
            .collect();
        escape(&loop_lib::display_command(&argv)).into_owned()
    };
    let suggestion = match near {
        // help takes the command to explain, not the typo's arguments
        Some("help") => "\n\nA command with a similar name exists:\n    meta help".to_string(),
        Some(word) => format!(
            "\n\nA command with a similar name exists:\n    meta {}",
            edited(&[word])
        ),
        None if run_it => format!(
            "\n\nTo run '{}' across all repos:\n    meta {}",
            escape(&loop_lib::display_command(command_args)),
            edited(&["exec", "--", first_cmd])
        ),
        None => String::new(),
    };
    // a workspace's own plugins are found from its .meta.yaml
    let outside = std::env::current_dir()
        .ok()
        .is_some_and(|cwd| matches!(find_meta_config(&cwd, None), Ok(None)));
    let outside = match outside {
        true => "\n\nNo .meta.yaml here or above, so no workspace's own plugins were looked for.",
        false => "",
    };
    meta_core::exit::Usage(format!(
        "unrecognized command '{}'{suggestion}{outside}\n\n\
         The commands there are, the ones plugins add too: meta --help",
        escape(first_cmd),
    ))
    .into()
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

    // inside a set, or where that can't be told (the command reports why)
    if !matches!(worktree::detect_worktree_context(&current_dir), Ok(None)) {
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
    if !matches!(config::set_source(&meta_dir), Ok(None)) {
        return;
    }
    if let Some(warning) = config::check_orphan_status(&meta_dir) {
        meta_core::out::warn(format!(
            "This meta repo is not tracked by its parent.\n  \
             Current: {}\n  Parent:  {}\n\n  \
             To include it, add to {}:\n    {}:\n      repo: <git-url>\n      meta: true\n",
            escape_path(&warning.current),
            escape_path(&warning.parent),
            escape_path(&warning.parent_config),
            escape(&warning.suggested_key)
        ));
    }
}
