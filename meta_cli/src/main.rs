use anyhow::Result;
use clap::{Args, FromArgMatches, Subcommand};
use meta_core::config::{self, find_meta_config, Workspace};
use meta_core::repo_filter::{Filters, Reach};
use meta_plugin::display::{escape, escape_lines, escape_path};
use meta_plugin::ExecutionMode;
use meta_plugin::{errln, out, outln};
use std::path::PathBuf;

mod completions;
mod context;
mod events;
mod index;
mod init;
mod invoke;
mod project;
mod registry;
mod report;
mod run_list;
mod subprocess_plugins;
use meta_core::worktree;
use meta_plugin::Scope;
use report::Report;
use subprocess_plugins::{SubprocessPlugin, SubprocessPluginManager};

// === CLI Structs ===

pub const VERSION: &str = include_str!("../../VERSION");

/// The version, and for a build from a git checkout its commit and date:
/// `<version> (git 1c96c7e 2026-11-03)`
fn version() -> String {
    format!("{}{}", VERSION.trim(), env!("META_BUILD_COMMIT"))
}

/// meta-host.toml: meta's own options and commands, the one declaration of
/// them.
pub const HOST_MANIFEST: &str = include_str!("../meta-host.toml");

/// The types `meta_plugin::build` generated from it.
#[expect(
    dead_code,
    reason = "clap derive types parse the built-in commands until the host parses them itself"
)]
mod host {
    include!(concat!(env!("OUT_DIR"), "/host.rs"));
}
pub use host::MetaOptions as Cli;

/// meta-host.toml, read. The build generated its types from this very text,
/// so a problem in it is a failed build, never this run.
fn host_manifest() -> meta_plugin::manifest::Manifest {
    meta_plugin::manifest::Manifest::parse(HOST_MANIFEST).unwrap_or_default()
}

/// meta's command line: its options from meta-host.toml, then its commands.
/// meta's options go before the command; every word after the command
/// belongs to it, so `meta git pull --tag v1` gives `--tag v1` to git.
pub fn command_line() -> clap::Command {
    let manifest = host_manifest();
    let meta = clap::Command::new("meta")
        // named meta however it was started (meta.exe, a link): its help and
        // errors say `meta`, and meta's own help is told apart by its text
        .bin_name("meta")
        .version(version())
        .about(manifest.description.unwrap_or_default())
        .disable_help_subcommand(true)
        .after_help(
            "meta's options go before the command (meta --dry-run git update); \
             what follows the command belongs to it.",
        )
        .args(meta_plugin::parser::args(&manifest.meta_options));
    Commands::augment_subcommands(meta)
}

/// What `matches` (of [`command_line`]) says: meta's options, and the command.
fn parsed(matches: &clap::ArgMatches) -> Result<(Cli, Option<Commands>)> {
    let values = meta_plugin::parser::values(matches, &host_manifest().meta_options);
    let cli = serde_json::from_value(serde_json::Value::Object(values))?;
    let command = match matches.subcommand().is_some() {
        true => Some(Commands::from_arg_matches(matches)?),
        false => None,
    };
    Ok((cli, command))
}

impl Cli {
    /// How events are shown as they arrive.
    pub fn show(&self) -> events::Show {
        match (self.json_stream, self.json) {
            (true, _) => events::Show::Stream,
            (false, true) => events::Show::Json,
            (false, false) => events::Show::Text,
        }
    }

    /// `--dry-run`, resolved once for every command
    fn mode(&self) -> ExecutionMode {
        ExecutionMode::from_dry_run(self.dry_run)
    }

    /// `--include`, `--exclude` and `--tag`
    pub fn filters(&self) -> Filters {
        let given = |values: &Vec<String>| Some(values.clone()).filter(|v| !v.is_empty());
        Filters::new(
            given(&self.include),
            given(&self.exclude),
            self.tag.as_deref(),
        )
    }

    /// Where the command runs from: the current directory, or with
    /// `--primary` inside a worktree set, its place in the workspace the set
    /// was made from
    pub fn start_dir(&self) -> Result<PathBuf> {
        let cwd = std::env::current_dir()?;
        if !self.primary {
            return Ok(cwd);
        }
        Ok(worktree::primary_dir(&cwd)?.unwrap_or(cwd))
    }

    /// meta's options from a whole command line, for tests.
    #[cfg(test)]
    pub fn parse_from<'a>(line: impl IntoIterator<Item = &'a str>) -> Self {
        let matches = command_line().get_matches_from(line);
        let values = meta_plugin::parser::values(&matches, &host_manifest().meta_options);
        serde_json::from_value(serde_json::Value::Object(values)).unwrap()
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
    /// Print the script that completes meta's commands in a shell
    Completions {
        /// The shell to complete in
        #[arg(value_name = "SHELL", value_parser = ["bash", "zsh", "fish", "powershell"])]
        shell: String,
    },
    /// List the completions of a command line (what the scripts of `meta completions` call)
    #[command(name = "__complete", hide = true)]
    Complete {
        /// The words after `meta`, the last one being completed
        #[arg(
            value_name = "WORD",
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        words: Vec<String>,
    },
    /// Initialize meta integrations
    Init(InitArgs),
    /// Manage plugins
    Plugin(PluginArgs),
    /// Inspect and validate the workspace's projects
    Project(project::ProjectArgs),
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

impl PluginCommands {
    /// Its words, as the Envelope names the command.
    fn name(&self) -> &'static str {
        match self {
            PluginCommands::Search { .. } => "plugin search",
            PluginCommands::Install { .. } => "plugin install",
            PluginCommands::List { .. } => "plugin list",
            PluginCommands::Uninstall { .. } => "plugin uninstall",
            PluginCommands::Update { .. } => "plugin update",
        }
    }
}

// === Help Utilities ===

/// meta's help, with an entry for each plugin's command word (each plugin is
/// started for its info) unless a built-in command has that name: its
/// description and version, its own commands, and what becomes of the
/// rest. Plugins name and describe themselves, so their values are escaped.
fn help_with_plugins(plugins: &SubprocessPluginManager) -> String {
    let index = index::load(plugins);
    let mut cmd = command_line();
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
    // a word only a manifest declares
    for word in index::top_words(&index) {
        let shown = escape(word).into_owned();
        if cmd.find_subcommand(&shown).is_some() {
            continue;
        }
        let summary = index
            .commands
            .iter()
            .find(|held| held.name.split(' ').next() == Some(word))
            .map(|held| escape(&held.command.summary).into_owned());
        cmd = cmd.subcommand(clap::Command::new(shown).about(summary.unwrap_or_default()));
    }
    format!("{}{}", cmd.render_help(), index::notices(&index))
}

// === Main Entry Point ===

/// The command being run, once known: what an Envelope of an error that
/// ends meta names.
static COMMAND: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// The words of meta's own command typed after meta's options (`project
/// dependents`, `plugin install`), each by its name (an alias by the command
/// it stands for), as far as they name commands, and the word typed where
/// one of them expects a subcommand; empty when the first word is no command
/// of meta's.
fn typed_command() -> String {
    let typed: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let skip = meta_options().typed.len();
    let mut command = command_line();
    let mut words = Vec::new();
    for word in typed.iter().skip(skip) {
        let Some(sub) = command.find_subcommand(word).cloned() else {
            // a word where a command of its takes subcommands: the one typed
            // (`project nonesuch`)
            if command.has_subcommands() && !words.is_empty() && !word.starts_with('-') {
                words.push(word.clone());
            }
            break;
        };
        words.push(sub.get_name().to_string());
        command = sub;
    }
    words.join(" ")
}

/// The command being run is `name` (the Envelope of an error names it).
pub fn running(name: &str) {
    if let Ok(mut command) = COMMAND.lock() {
        name.clone_into(&mut command);
    }
}

/// `--json-stream`: an error that ends meta is the stream's last line.
static STREAM: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn main() -> std::process::ExitCode {
    meta_plugin::exit::run(|| {
        if let Err(e) = meta_plugin::process::own() {
            meta_plugin::out::warn(format!(
                "cannot pass signals on to the programs meta starts: {e}"
            ));
        }
        let result = run_cli();
        match result {
            // under --json an error ends in the Envelope too, unless one
            // was printed already (a failed command's)
            Err(e) if meta_plugin::out::json() && !reported(&e) => {
                let error = meta_plugin::envelope::info_of(&e);
                let code = meta_plugin::exit::ending(&e).unwrap_or(error.code.exit_code());
                let command = COMMAND.lock().map(|c| c.clone()).unwrap_or_default();
                let envelope = meta_plugin::Envelope::failed(command, error);
                match STREAM.load(std::sync::atomic::Ordering::Relaxed) {
                    true => envelope.print_last(&[])?,
                    false => envelope.print(&[])?,
                }
                Err(meta_plugin::exit::ExitWith(code).into())
            }
            result => result,
        }
    })
}

/// Whether `error` is an ending already reported: an exit code alone (the
/// Envelope or summary said why), or a closed stdout.
fn reported(error: &anyhow::Error) -> bool {
    meta_plugin::exit::ending(error).is_some()
        && error.downcast_ref::<meta_plugin::ErrorInfo>().is_none()
        && !error
            .chain()
            .any(|cause| cause.is::<meta_plugin::ErrorInfo>())
}

/// Tell the programs meta starts how to call it back (`META_EXE`, and
/// `META_DEPTH`: how many meta calls are around this one), refusing a meta
/// deeper than calls may go: a command that calls itself through meta ends.
fn called_back() -> Result<()> {
    use meta_plugin::client::{depth, DEPTH_VAR, EXE_VAR, MAX_DEPTH};
    let depth = depth()?;
    if depth > MAX_DEPTH {
        anyhow::bail!(
            "This run of meta is {depth} calls deep, deeper than {MAX_DEPTH}: a command calls itself through meta"
        );
    }
    // set while meta has one thread, before it starts anything
    std::env::set_var(DEPTH_VAR, depth.to_string());
    if let Ok(exe) = std::env::current_exe() {
        std::env::set_var(EXE_VAR, exe);
    }
    Ok(())
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
    called_back()?;

    // meta's own --help also lists the commands plugins add, so it is
    // answered here, before anything else runs; every other result of
    // parsing is reported as usual
    let matches = match command_line().try_get_matches() {
        Ok(matches) => matches,
        Err(e) if is_root_help(&e) => {
            check_and_warn_orphan();
            let plugins = SubprocessPluginManager::new(false)?;
            return Ok(out!("{}", help_with_plugins(&plugins))?);
        }
        Err(e) => {
            // the Envelope of a usage error names the command it is about
            running(&typed_command());
            match project::usage_error(&e) {
                Some(error) => return Err(error),
                None => meta_plugin::args::parse_command(command_line())?,
            }
        }
    };
    let (mut cli, command) = parsed(&matches)?;
    // the stream is JSON too: every command reads `json` for it, and only
    // the printing tells the two apart
    cli.json |= cli.json_stream;
    STREAM.store(cli.json_stream, std::sync::atomic::Ordering::Relaxed);
    meta_plugin::out::set_json(cli.json);
    let fields = cli.fields.clone();
    anyhow::ensure!(
        cli.json || fields.is_empty(),
        meta_plugin::exit::Usage(
            "--fields picks fields of the --json output: add --json".to_string()
        )
    );

    // Check for orphaned nested meta repo and warn the user; a TAB says nothing
    if !matches!(command, Some(Commands::Complete { .. })) {
        check_and_warn_orphan();
    }

    // Where plugins are; nothing is started until a command needs one
    let subprocess_plugins = SubprocessPluginManager::new(cli.verbose)?;

    // `--tag "$UNSET"` is a slip, not "every repo"
    anyhow::ensure!(
        cli.tag.is_none() || !cli.filters().tags.is_empty(),
        "--tag '{}' names no tag",
        escape(cli.tag.as_deref().unwrap_or_default())
    );

    let mut report = Report::new(match &command {
        None => "help",
        Some(Commands::Help { .. }) => "help",
        Some(Commands::Completions { .. }) => "completions",
        Some(Commands::Complete { .. }) => "__complete",
        Some(Commands::Context(_)) => "context",
        Some(Commands::Init(_)) => "init claude",
        Some(Commands::Plugin(args)) => args.command.name(),
        Some(Commands::Exec(_)) => "exec",
        Some(Commands::Project(args)) => args.command.name(),
        Some(Commands::External(_)) => "",
    });
    running(&report.command);
    match &command {
        None => help(&cli, &[], &subprocess_plugins, &mut report)?,
        Some(Commands::Help { command }) => help(&cli, command, &subprocess_plugins, &mut report)?,
        Some(Commands::Completions { shell }) => out!("{}", completions::script(shell))?,
        Some(Commands::Complete { words }) => {
            for word in completions::complete(&index::load(&subprocess_plugins), words) {
                outln!("{word}")?;
            }
        }
        Some(Commands::Context(args)) => {
            let workspace =
                Workspace::find(&cli.start_dir()?, cli.config.as_ref())?.ok_or_else(|| {
                    meta_plugin::ErrorInfo::new(
                        meta_plugin::ErrorCode::NotFound,
                        "Not a meta workspace (no .meta.yaml found)",
                    )
                })?;
            context::handle_context(
                cli.json,
                args.no_status,
                cli.verbose,
                Reach::of(workspace, cli.recursive, cli.all, cli.depth)?,
                &cli.filters(),
                with_all,
                &mut report,
            )?
        }
        Some(Commands::Init(InitArgs {
            command: InitCommands::Claude { force, update },
        })) => init::init_claude(*force, *update, cli.mode(), cli.verbose)?,
        Some(Commands::Plugin(args)) => handle_plugin_command(
            &args.command,
            cli.mode(),
            cli.json,
            &subprocess_plugins,
            &mut report,
        )?,
        Some(Commands::Exec(args)) => invoke::exec(&cli, &args.command, &mut report)?,
        Some(Commands::Project(args)) => project::run(&cli, &args.command, &mut report)?,
        Some(Commands::External(args)) => {
            external(&cli, args, &subprocess_plugins, &mut report, false)?
        }
    }
    report.finish(cli.show(), cli.silent, &fields)
}

/// `meta <word> ...`, served by the plugin of `word`; `delegated` when a
/// plugin asked for it (its own delegate is refused: it could loop).
fn external(
    cli: &Cli,
    args: &[String],
    plugins: &SubprocessPluginManager,
    report: &mut Report,
    delegated: bool,
) -> Result<()> {
    // Only the plugin serving this word is started
    let plugin = match args.first() {
        Some(word) => plugins.find(word)?,
        None => None,
    };
    let Some(plugin) = plugin else {
        return Err(unrecognized_command_error(args, plugins, true));
    };
    let delegate = plugin_command(cli, args, &plugin, report)?;
    match delegate {
        None => Ok(()),
        Some(_) if delegated => Err(meta_plugin::ErrorInfo::new(
            meta_plugin::ErrorCode::PluginFailed,
            format!(
                "plugin {} delegated a command that was itself a delegate",
                escape(&plugin.word)
            ),
        )
        .into()),
        Some(line) if line.first().is_some_and(|w| w == "exec") => {
            let argv = line.get(1..).unwrap_or_default();
            let argv = argv.strip_prefix(&["--".to_string()]).unwrap_or(argv);
            invoke::exec(cli, argv, report)
        }
        Some(line) => external(cli, &line, plugins, report, true),
    }
}

/// `text`, the help asked for: printed, or under `--json` the Envelope's
/// message.
fn help_message(cli: &Cli, text: &str, report: &mut Report) -> Result<()> {
    match cli.json {
        true => report.message = Some(text.to_string()),
        false => out!("{text}")?,
    }
    Ok(())
}

/// Whether `error` is clap showing meta's own help (`meta --help`, also
/// before a command: `meta --help exec`), not a command's
fn is_root_help(error: &clap::Error) -> bool {
    let mut root = command_line();
    root.build();
    let shown = error.render().to_string();
    error.kind() == clap::error::ErrorKind::DisplayHelp
        && (shown == root.render_help().to_string() || shown == root.render_long_help().to_string())
}

/// `meta help [COMMAND...]`: meta's help, a built-in command's, or the
/// plugin command's, as `meta <command> --help` prints it.
fn help(
    cli: &Cli,
    words: &[String],
    plugins: &SubprocessPluginManager,
    report: &mut Report,
) -> Result<()> {
    if let Some(error) = option_after_help(words) {
        report.fail(error, false);
        return Ok(());
    }
    if cli.json {
        let index = index::load(plugins);
        let answer = match words.is_empty() {
            true => Some(index.clone()),
            false => index::about(&index, words),
        };
        if let Some(answer) = answer {
            let data = serde_json::to_value(answer)?;
            report
                .results
                .push(meta_plugin::ProjectResult::about_workspace(data));
            return Ok(());
        }
    }
    let Some(first) = words.first() else {
        return help_message(cli, &help_with_plugins(plugins), report);
    };
    let mut cmd = command_line();
    // built, so the subcommands carry their names and meta's help settings
    cmd.build();
    if cmd.find_subcommand(first).is_some() {
        let mut found = &mut cmd;
        for (i, word) in words.iter().enumerate() {
            found = match found.find_subcommand_mut(word) {
                Some(sub) => sub,
                None => {
                    return Err(meta_plugin::exit::Usage(format!(
                        "meta {} has no command '{}'",
                        escape(&words.get(..i).unwrap_or_default().join(" ")),
                        escape(word)
                    ))
                    .into())
                }
            };
        }
        return help_message(cli, &found.render_help().to_string(), report);
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
    plugin_command(cli, &args, &plugin, report).map(|_| ())
}

/// The usage error for one of meta's options typed after `help`, with the
/// tip every command gives; `None` when none was.
fn option_after_help(words: &[String]) -> Option<meta_plugin::ErrorInfo> {
    let meta = meta_options();
    let is_meta = |word: &str| {
        let spelling = word.split('=').next().unwrap_or(word);
        let spelled = |o: &meta_plugin::args::MetaOption| o.spellings.iter().any(|s| s == spelling);
        meta.options.iter().any(spelled)
    };
    let rejected = words
        .iter()
        .take_while(|word| *word != "--")
        .find(|word| is_meta(word))?;
    let line: Vec<String> = std::iter::once("help".to_string())
        .chain(words.iter().cloned())
        .collect();
    let check = |moved: &[String], rest: &[String]| {
        let line = std::iter::once("meta".to_string())
            .chain(meta.typed.iter().cloned())
            .chain(moved.iter().cloned())
            .chain(rest.iter().cloned());
        meta_plugin::args::verdict(command_line().try_get_matches_from(line))
    };
    let mut error = meta_plugin::ErrorInfo::new(
        meta_plugin::ErrorCode::Usage,
        format!("unexpected argument '{}'", escape(rejected)),
    );
    error.hint = meta.tip(&line, rejected, &check);
    Some(error)
}

/// `meta <word> ...`, served by `plugin`: its help, or the command in the
/// selected repos. What the plugin delegated to, if it did.
fn plugin_command(
    cli: &Cli,
    args: &[String],
    plugin: &SubprocessPlugin,
    report: &mut Report,
) -> Result<Option<Vec<String>>> {
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
        report.command = plugin.word.clone();
        let help = meta_plugin::help_text(&plugin.word, &plugin.info);
        let help = format!("{}{}\n", answered_by(cli, args, plugin), help.trim_end());
        help_message(cli, &help, report)?;
        return Ok(None);
    }
    if wants_help {
        return command_help(cli, args, plugin, report);
    }
    dispatch(cli, args, plugin, report)
}

/// The `answered by:` line that opens the help of `args` (empty under
/// `--json`, where the help alone is the Envelope's message): from the
/// index, or for a file with no manifest, from what it says of itself.
fn answered_by(cli: &Cli, args: &[String], plugin: &SubprocessPlugin) -> String {
    if cli.json {
        return String::new();
    }
    let typed: Vec<String> = args
        .iter()
        .take_while(|arg| !matches!(arg.as_str(), "--" | "--help" | "-h"))
        .cloned()
        .collect();
    let line = SubprocessPluginManager::new(false)
        .ok()
        .and_then(|plugins| {
            let index = index::load(&plugins);
            let (held, _) = index::resolve(&index, &typed)?;
            Some(index::answered_by(held, &typed))
        });
    let line = line.unwrap_or_else(|| {
        format!(
            "answered by: {} {} ({})",
            escape(&plugin.word),
            escape(&plugin.info.version),
            escape_path(&plugin.path)
        )
    });
    format!("{line}\n\n")
}

/// `meta <word> <verb> ... --help`: a line saying who answers the command,
/// then the command's help: the plugin's for one of its own, meta's for one
/// it runs as typed.
fn command_help(
    cli: &Cli,
    args: &[String],
    plugin: &SubprocessPlugin,
    report: &mut Report,
) -> Result<Option<Vec<String>>> {
    use meta_plugin::Verb;
    let verb = args.get(1).map(String::as_str).unwrap_or_default();
    let start = cli.start_dir()?;
    let ask_plugin = |report: &mut Report| {
        let request = invoke::request(cli, plugin, args, &start, Scope::None, None)?;
        invoke::call(cli, plugin, &request, None, report)
    };
    match plugin.info.verb(verb) {
        Verb::Own => {
            out!("{}", answered_by(cli, args, plugin))?;
            ask_plugin(report)
        }
        Verb::AsTyped(tool) => {
            report.command = format!("{} {verb}", plugin.word);
            let help = as_typed_help(cli, args, plugin, tool);
            help_message(
                cli,
                &format!("{}{help}", answered_by(cli, args, plugin)),
                report,
            )?;
            Ok(None)
        }
        Verb::Unknown => ask_plugin(report),
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
    let scope = match cli
        .start_dir()
        .and_then(|start| invoke::target(cli, &start))
    {
        Ok(target) => {
            let names: Vec<String> = target
                .dirs()
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

/// The help of meta's options, from their one declaration.
fn selection_options_help() -> String {
    clap::Command::new("meta")
        .disable_help_flag(true)
        .disable_version_flag(true)
        .help_template("{options}")
        .args(meta_plugin::parser::args(&host_manifest().meta_options))
        .render_help()
        .to_string()
}

// === Command Dispatch ===

/// `args` (starting with the plugin's word) through `plugin`, in the repos
/// meta's options select. `git clone` makes the workspace, so it needs
/// none; worktree commands find their repos in the set they name (in the
/// workspace's members).
fn dispatch(
    cli: &Cli,
    args: &[String],
    plugin: &SubprocessPlugin,
    report: &mut Report,
) -> Result<Option<Vec<String>>> {
    running(
        &args
            .get(..plugin.command_len(args))
            .unwrap_or_default()
            .join(" "),
    );
    let start = cli.start_dir()?;
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    // the set a worktree command names: the word after its verb
    let named = words.get(3).copied().filter(|name| !name.starts_with('-'));
    let scope = match words.as_slice() {
        ["git", "clone", ..] => Scope::None,
        ["git", "update", ..] => Scope::Workspace,
        ["git", "worktree", "exec" | "remove" | "status" | "diff" | "add", ..]
            if named.is_some() =>
        {
            Scope::NamedSet
        }
        ["git", "worktree", ..] => Scope::Workspace,
        _ => Scope::Selection,
    };
    // the host applies meta's selection: a command that finds its repos
    // itself takes none, nor one that acts on a whole named set
    // (`exec --ephemeral` makes its set: there is nothing to select from yet)
    let ephemeral = words
        .iter()
        .take_while(|w| **w != "--")
        .any(|w| *w == "--ephemeral");
    let whole_set = scope == Scope::NamedSet && (words.get(2) != Some(&"exec") || ephemeral);
    let refuses_filters = matches!(scope, Scope::None | Scope::Workspace) || whole_set;
    refuse_filters(
        refuses_filters && !cli.filters().is_empty(),
        args.get(..3).unwrap_or(args),
    )?;
    let target = match (scope, named) {
        // a clone runs where it is, in a workspace or none
        (Scope::None, _) => None,
        (Scope::NamedSet, Some(name)) => invoke::named_set(cli, &start, name)?,
        // outside a workspace, the command says what it needs itself; a
        // config that is refused is an error here
        (Scope::Workspace | Scope::NamedSet, _) => match invoke::target(cli, &start) {
            Ok(target) => Some(target),
            Err(e) if meta_plugin::envelope::code_of(&e) == meta_plugin::ErrorCode::NotFound => {
                None
            }
            Err(e) => return Err(e),
        },
        (Scope::Selection, _) => Some(invoke::target(cli, &start)?),
    };
    // a set is acted on by name only by the provider that made it: one VCS
    // never removes another's checkouts
    if let Some(set) = target.as_ref().and_then(|t| t.set.as_ref()) {
        if scope == Scope::NamedSet && set.provider != plugin.word {
            return Err(meta_plugin::ErrorInfo::new(
                meta_plugin::ErrorCode::Conflict,
                format!(
                    "Worktree set '{}' was made by meta-{}; meta {} acts only on the sets meta-{} made",
                    escape(&set.name),
                    escape(&set.provider),
                    escape(&plugin.word),
                    escape(&plugin.word)
                ),
            )
            .into());
        }
    }
    let mut request = invoke::request(cli, plugin, args, &start, scope, target.as_ref())?;
    // typed after the command, they get no tip to type them before it
    if refuses_filters {
        request.meta_options = request
            .meta_options
            .refusing(&["--include", "--exclude", "--tag"]);
    }
    invoke::call(cli, plugin, &request, target.as_ref(), report)
}

/// A usage error when meta's filters were given (`filtered`) to `command`
/// (its words), which finds its repos itself.
fn refuse_filters(filtered: bool, command: &[impl AsRef<str>]) -> Result<()> {
    if !filtered {
        return Ok(());
    }
    let words: Vec<String> = command.iter().map(|w| w.as_ref().to_string()).collect();
    Err(meta_plugin::exit::Usage(format!(
        "meta {} finds its repos itself (in the .meta.yaml files or its worktree sets); it does not take --include/--exclude/--tag",
        escape(&loop_lib::display_command(&words))
    ))
    .into())
}

/// meta's options, and what of this process's arguments is them
fn meta_options() -> meta_plugin::args::MetaOptions {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    meta_plugin::args::MetaOptions::of(&command_line(), &args)
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

/// The line as typed with meta's `-r` in it as `-A`, which reaches the
/// whole workspace (in a cluster too: `-rv` as `-Av`).
fn with_all() -> String {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let meta = meta_options();
    let valued = |c: char| {
        let short = format!("-{c}");
        meta.options
            .iter()
            .any(|o| o.takes_value && o.spellings.contains(&short))
    };
    // the r of a cluster, up to its first short option that takes a value
    let cluster = |shorts: &str| -> String {
        let mut seen_value = false;
        let swapped: String = shorts
            .chars()
            .map(|c| {
                seen_value |= valued(c);
                match c == 'r' && !seen_value {
                    true => 'A',
                    false => c,
                }
            })
            .collect();
        format!("-{swapped}")
    };
    let line: Vec<String> = std::iter::once("meta".to_string())
        .chain(args.iter().enumerate().map(|(i, arg)| {
            match (i < meta.typed.len(), arg.strip_prefix('-')) {
                (true, _) if arg == "--recursive" => "-A".to_string(),
                (true, Some(shorts)) if !shorts.starts_with('-') => cluster(shorts),
                _ => arg.clone(),
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
    report: &mut Report,
) -> Result<()> {
    use meta_plugin::ProjectResult;
    let mut warnings = Vec::new();
    let results = &mut report.results;
    let mut record = |data: serde_json::Value| results.push(ProjectResult::about_workspace(data));
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

            for plugin in &results {
                record(serde_json::to_value(plugin)?);
            }
            if json {
                // the records above are the --json output
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
                record(serde_json::json!({"installed": plugin_name, "to": location}));
                if !json {
                    outln!(
                        "Successfully installed {} to {location}",
                        escape(&plugin_name)
                    )?;
                }
            } else if let Some(shorthand) = GitHubShorthand::parse(name) {
                // GitHub shorthand install (user/repo[@version])
                let plugin_name = installer.install_from_github(&shorthand)?;
                record(serde_json::json!({"installed": plugin_name, "to": location}));
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
                            record(serde_json::json!({"installed": plugin_name, "to": location}));
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
                        record(serde_json::json!({
                            "installed": metadata.name,
                            "version": metadata.version,
                            "to": location,
                        }));

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
                for plugin in &plugins {
                    record(serde_json::to_value(plugin)?);
                }
                if json {
                    // the records above are the --json output
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
                let index = index::load(subprocess_plugins);
                let declared = index::manifest_only(&index, &plugins);

                for plugin in &plugins {
                    record(serde_json::json!({
                        "name": plugin.word,
                        "version": plugin.info.version,
                        "description": plugin.info.description,
                        "path": plugin.path.to_string_lossy(),
                    }));
                }
                for provider in &declared {
                    record(serde_json::json!({
                        "name": provider.name,
                        "version": provider.version,
                        "path": provider.path.as_deref().map(std::path::Path::to_string_lossy),
                    }));
                }
                warnings.extend(
                    index
                        .problems
                        .iter()
                        .map(|problem| meta_plugin::Warning::new(index::problem_line(problem))),
                );
                if json {
                    // the records above are the --json output
                } else if plugins.is_empty() && declared.is_empty() {
                    outln!("No plugins installed")?;
                } else {
                    outln!("Installed plugins ({}):", plugins.len() + declared.len())?;
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
                    for provider in &declared {
                        let path = provider.path.as_deref();
                        outln!(
                            "{:<12} {:<12} {}",
                            escape(&provider.name),
                            escape(provider.version.as_deref().unwrap_or("-")),
                            path.map_or(String::new(), escape_path)
                        )?;
                    }
                    tool_extensions(&plugins)?;
                }
                if !json {
                    out!("{}", index::notices(&index))?;
                }
            }
        }
        PluginCommands::Uninstall { name, local } => {
            let installer = create_installer(*local)?;
            let location = format_plugin_location(*local);
            installer.uninstall(name)?;
            record(serde_json::json!({"uninstalled": name, "from": location}));

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
                        record(serde_json::json!({
                            "name": plugin_name, "version": current, "latest": latest, "updated": !check,
                        }));
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
                        record(serde_json::json!({"name": plugin_name, "updated": false}));
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
                        Ok(None) if json => {
                            record(serde_json::json!({"name": name, "updated": false}))
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
                    for (name, current, latest) in &updates_available {
                        record(serde_json::json!({
                            "name": name, "version": current, "latest": latest, "updated": false,
                        }));
                    }
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
                                record(serde_json::json!({
                                    "name": name, "version": current, "latest": latest, "updated": true,
                                }));
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

    report.warnings.extend(warnings);
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
    let index = index::load(plugins);
    let builtins = index::top_words(&index);
    // Plugins by their file names: none is started to list them
    let words = plugins.words();
    let near = (!subprocess_plugins::on_path(first_cmd))
        .then(|| {
            meta_plugin::args::nearest(
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
    // declared by a manifest, with no `meta-<word>` file to run it yet
    let declared = index::resolve(&index, command_args)
        .filter(|(held, _)| held.provider.source > meta_plugin::manifest::Source::Host)
        .map(|(held, _)| {
            format!(
                "\n\n{}, but meta runs only `meta-<word>` programs so far: there is no meta-{}.",
                index::answered_by(held, command_args),
                escape(first_cmd)
            )
        })
        .unwrap_or_default();
    meta_plugin::exit::Usage(format!(
        "unrecognized command '{}'{declared}{suggestion}{outside}\n\n\
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
    if !matches!(worktree::detect_worktree_location(&current_dir), Ok(None)) {
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
        meta_plugin::out::warn(format!(
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The clap type of the built-in `words`, by its subcommands.
    fn clap_of(words: &str) -> Option<clap::Command> {
        let mut found = command_line();
        for word in words.split(' ') {
            found = found.find_subcommand(word)?.clone();
        }
        Some(found)
    }

    #[test]
    fn the_clap_types_and_meta_host_toml_declare_the_same_commands() {
        let manifest = host_manifest();
        let mut differences = Vec::new();
        for (name, command) in &manifest.commands {
            let Some(clap) = clap_of(name) else {
                differences.push(format!("{name}: no clap type"));
                continue;
            };
            let found = meta_plugin::parser::differences(&command.summary, &command.options, &clap);
            differences.extend(found.into_iter().map(|d| format!("{name}: {d}")));
        }
        assert!(differences.is_empty(), "{}", differences.join("\n"));
    }
}
