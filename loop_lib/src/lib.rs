use anyhow::{Context, Result};
use colored::*;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use meta_core::display::{escape, escape_path, program_output};
use meta_core::{errln, out, outln};
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::io::{self, IsTerminal};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// What loop runs and how: the `.looprc` file (JSON), and what meta builds for
/// the commands it runs. A key the file leaves out takes its default; a key
/// loop doesn't know is an error.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoopConfig {
    pub directories: Vec<PathBuf>,
    pub ignore: Vec<String>,
    pub verbose: bool,
    pub silent: bool,
    pub parallel: bool,
    pub dry_run: bool,
    pub json_output: bool,
    /// Milliseconds between two command starts in parallel mode (0: none),
    /// to spread out connections
    pub spawn_stagger_ms: u64,
    /// Environment variables to set for all command subprocesses.
    /// Tool-specific env vars (e.g., GIT_PAGER) should be set by the caller.
    pub env: Option<HashMap<String, String>>,
    /// At most this many commands at once in parallel mode, for shared
    /// resources (an SSH ControlMaster allows 10 sessions by default)
    pub max_parallel: Option<NonZeroUsize>,
    /// Programs that stand for a command: with `{"gs": "git status"}`,
    /// `gs -s` runs `git status -s`
    pub aliases: BTreeMap<String, String>,
    /// The root directory of the meta workspace. When set, this directory
    /// is displayed as "." or ". (basename)" instead of its full path,
    /// and is sorted first in output.
    pub root_dir: Option<PathBuf>,
}

/// A command to execute in a specific directory
#[derive(Debug, Clone)]
pub struct DirCommand {
    pub dir: PathBuf,
    /// The program and its arguments, run as given: no shell parses them.
    pub argv: Vec<String>,
    /// Environment variables to set for this command's subprocess
    pub env: Option<HashMap<String, String>>,
    /// Why the command must not run here (its folder is no project the
    /// caller acts in, say): reported as a failed result, never run.
    pub refused: Option<String>,
}

impl Default for LoopConfig {
    fn default() -> Self {
        LoopConfig {
            directories: vec![],
            ignore: vec![".git".to_string()],
            verbose: false,
            silent: false,
            parallel: false,
            dry_run: false,
            json_output: false,
            spawn_stagger_ms: 0,
            env: None,
            max_parallel: None,
            aliases: BTreeMap::new(),
            root_dir: None,
        }
    }
}

#[derive(Default)]
pub struct CommandResult {
    pub success: bool,
    /// None when a signal ended the command; -1 when it never started
    pub exit_code: Option<i32>,
    /// The signal that ended the command (unix)
    pub signal: Option<i32>,
    pub directory: PathBuf,
    pub command: String,
    /// What the command wrote, as it wrote it (not necessarily UTF-8)
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl CommandResult {
    /// How the command ended, for a failure line: `exit code 1`,
    /// `killed by signal 9`.
    fn ending(&self) -> String {
        match (self.signal, self.exit_code) {
            (Some(signal), _) => format!("killed by signal {signal}"),
            (None, Some(code)) => format!("exit code {code}"),
            (None, None) => "no exit code".to_string(),
        }
    }
}

/// The exit code and, on unix, the signal of a finished command.
fn exit_of(status: ExitStatus) -> (Option<i32>, Option<i32>) {
    #[cfg(unix)]
    let signal = std::os::unix::process::ExitStatusExt::signal(&status);
    #[cfg(not(unix))]
    let signal = None;
    (status.code(), signal)
}

/// The `.looprc` at `path`. Errors name the file: missing, unreadable, not
/// JSON, or with a key loop doesn't know.
pub fn parse_config(path: &Path) -> Result<LoopConfig> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", escape_path(path)))?;
    parse_looprc(path, &text)
}

/// [`parse_config`], or the default when there is no file at `path`.
pub fn parse_config_if_present(path: &Path) -> Result<LoopConfig> {
    match fs::read_to_string(path) {
        Ok(text) => parse_looprc(path, &text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(LoopConfig::default()),
        Err(e) => Err(e).with_context(|| format!("Failed to read {}", escape_path(path))),
    }
}

fn parse_looprc(path: &Path, text: &str) -> Result<LoopConfig> {
    // serde's message quotes the file's text (an unknown key): escaped too
    serde_json::from_str(text).map_err(|e| {
        anyhow::anyhow!(
            "Failed to parse {}: {}",
            escape_path(path),
            escape(&e.to_string())
        )
    })
}

fn global_looprc() -> Option<PathBuf> {
    env::var_os("HOME").map(|home| PathBuf::from(home).join(".looprc"))
}

/// `local` over the aliases of `~/.looprc`.
pub fn with_global_aliases(local: BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    let mut aliases = match global_looprc() {
        Some(path) => parse_config_if_present(&path)?.aliases,
        None => BTreeMap::new(),
    };
    aliases.extend(local);
    Ok(aliases)
}

/// The aliases meta expands: those of `~/.looprc`, then those of `./.looprc`,
/// which win.
pub fn aliases() -> Result<BTreeMap<String, String>> {
    with_global_aliases(parse_config_if_present(Path::new(".looprc"))?.aliases)
}

fn prompt_user(question: &str) -> Result<bool> {
    out!("{question} [y/N]: ")?;
    meta_core::out::flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_lowercase() == "y")
}

/// The aliases your interactive shell defines (`alias` in `$SHELL -i`).
fn shell_aliases() -> Result<BTreeMap<String, String>> {
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let output = Command::new(&shell)
        .args(["-i", "-c", "alias"])
        .output()
        .with_context(|| format!("Failed to run {}", escape(&shell)))?;
    anyhow::ensure!(
        output.status.success(),
        "`{} -i -c alias` failed:{}",
        escape(&shell),
        program_output(&String::from_utf8_lossy(&output.stderr))
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(name, value)| {
            // `alias ll='ls -l'` (bash) or `ll='ls -l'` (zsh): the value is
            // one word in shell quoting
            let name = name.trim().trim_start_matches("alias ").to_string();
            match shlex::split(value.trim()).as_deref() {
                Some([command]) => Ok((name, command.clone())),
                _ => Err(anyhow::anyhow!(
                    "Can't read the alias '{}' your shell lists: {}",
                    escape(&name),
                    escape(value)
                )),
            }
        })
        .collect()
}

/// `--add-aliases-to-global-looprc`: your shell's aliases into the `aliases`
/// of `~/.looprc`, after a preview and your yes. Its other keys stay as they
/// are; a file loop can't read is reported, not replaced.
pub fn import_aliases(dry_run: bool) -> Result<()> {
    if dry_run {
        outln!("[DRY RUN] Would add your shell's aliases to ~/.looprc")?;
        return Ok(());
    }
    let path = global_looprc().context("HOME is not set")?;
    let cancelled = || anyhow::anyhow!("Cancelled: nothing written to {}", escape_path(&path));

    let existing = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", escape_path(&path))),
    };
    let (mut doc, mut aliases) = match &existing {
        Some(text) => {
            // Read twice: as a .looprc, so a broken one is reported, and as
            // plain JSON, so its other keys are written back as they are
            let aliases = parse_looprc(&path, text)?.aliases;
            let doc: serde_json::Map<String, serde_json::Value> = serde_json::from_str(text)
                .with_context(|| format!("Failed to parse {}", escape_path(&path)))?;
            (doc, aliases)
        }
        None => {
            if !prompt_user("The global .looprc file does not exist. Do you want to create it?")? {
                return Err(cancelled());
            }
            Default::default()
        }
    };
    if !prompt_user("Do you want to set the value of the 'aliases' property?")? {
        return Err(cancelled());
    }

    aliases.extend(shell_aliases()?);
    doc.insert("aliases".to_string(), serde_json::to_value(&aliases)?);
    let new_content = serde_json::to_string_pretty(&doc)?;

    outln!("\nPreview of changes:")?;
    match &existing {
        Some(old) => {
            for diff in diff::lines(old, &new_content) {
                match diff {
                    diff::Result::Left(l) => outln!("{}", format!("-{l}").red())?,
                    diff::Result::Both(l, _) => outln!(" {l}")?,
                    diff::Result::Right(r) => outln!("{}", format!("+{r}").green())?,
                }
            }
        }
        None => outln!("{}", new_content.green())?,
    }
    if !prompt_user("Do you want to apply these changes?")? {
        return Err(cancelled());
    }

    meta_core::store::write_atomic(&path, &doc)?;
    outln!("Aliases have been added to {}", escape_path(&path))?;
    Ok(())
}

/// How a directory is named in progress and result lines: `.` for the root,
/// `./open-source/meta/loop_cli` for a directory under it, so repos that share
/// a basename stay distinguishable, and the full path for one outside it.
/// Without a root, the basename. Control characters in it are escaped.
fn display_name(dir: &Path, root: Option<&Path>) -> String {
    escape(&plain_display_name(dir, root)).into_owned()
}

fn plain_display_name(dir: &Path, root: Option<&Path>) -> String {
    let Some(root) = root else {
        return dir
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(".")
            .to_string();
    };
    match path_from(dir, root) {
        Some(rel) if rel.is_empty() => ".".to_string(),
        Some(rel) => format!("./{rel}"),
        None => dir.display().to_string(),
    }
}

/// `dir`'s path from `root`, `/`-separated and empty for `root` itself;
/// None when `dir` is not under `root`.
fn path_from(dir: &Path, root: &Path) -> Option<String> {
    let rel = dir.strip_prefix(root).ok()?;
    let parts: Vec<_> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect();
    Some(parts.join("/"))
}

/// [`display_name`], with the root's folder name added: `. (harmony)`.
fn result_name(dir: &Path, root: Option<&Path>) -> String {
    let name = display_name(dir, root);
    if name != "." {
        return name;
    }
    let base = if dir == Path::new(".") {
        env::current_dir().ok()
    } else {
        Some(dir.to_path_buf())
    };
    match base.as_deref().and_then(Path::file_name) {
        Some(base) => format!(". ({})", escape(&base.to_string_lossy())),
        None => name,
    }
}

/// `argv` with a `.looprc` alias for its program expanded. The alias value is
/// split into words the way a POSIX shell splits them (quotes are honored,
/// nothing is expanded or run) and replaces the program.
fn resolve_aliases(argv: &[String], aliases: &BTreeMap<String, String>) -> Result<Vec<String>> {
    let Some((program, args)) = argv.split_first() else {
        anyhow::bail!("No command given");
    };
    let Some(alias) = aliases.get(program) else {
        return Ok(argv.to_vec());
    };
    let mut words = shlex::split(alias)
        .filter(|words| !words.is_empty())
        .with_context(|| {
            format!(
                "Alias '{}' is not a command: {}",
                escape(program),
                escape(alias)
            )
        })?;
    words.extend_from_slice(args);
    Ok(words)
}

/// `argv` as one line, quoted so that it reads back as the same words.
pub fn display_command(argv: &[String]) -> String {
    shlex::try_join(argv.iter().map(String::as_str)).unwrap_or_else(|_| argv.join(" "))
}

/// The process for `argv` in `dir`. The program runs directly: no shell sees
/// the arguments, so quotes, `$()`, `;` and globs in them stay literal.
fn build_command(
    argv: &[String],
    dir: &Path,
    extra_env: Option<&HashMap<String, String>>,
) -> Result<Command> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("No command given"))?;
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(dir);
    // Each directory is its own repository: a GIT_DIR from a git hook around
    // meta would point git in every one of them at the hook's
    meta_core::git::clear_repo_env(&mut cmd);
    // Plugin-specified environment variables (e.g., GIT_PAGER=cat)
    if let Some(extra) = extra_env {
        cmd.envs(extra);
    }
    Ok(cmd)
}

fn failed_result(dir: &Path, command: String, stderr: String) -> CommandResult {
    CommandResult {
        success: false,
        exit_code: Some(-1),
        signal: None,
        directory: dir.to_path_buf(),
        command,
        stdout: Vec::new(),
        stderr: stderr.into_bytes(),
    }
}

/// The failed result of a command that was refused (see [`DirCommand::refused`]):
/// `why` as its error output, exit code 1, nothing run.
fn refused_result(job: &DirCommand, why: &str) -> CommandResult {
    CommandResult {
        exit_code: Some(1),
        ..failed_result(&job.dir, display_command(&job.argv), format!("{why}\n"))
    }
}

/// The dry-run line for `command` in `dir`.
fn dry_run_line(dir: &Path, command: &str) -> String {
    let dir_display = if dir.as_os_str() == "." {
        env::current_dir()
            .map(|cwd| escape_path(&cwd))
            .unwrap_or_else(|_| ".".to_string())
    } else {
        escape_path(dir)
    };
    format!("Would execute in {dir_display}:\n  {}", escape(command))
}

fn execute_command_in_directory(
    dir: &Path,
    argv: &[String],
    config: &LoopConfig,
    extra_env: Option<&HashMap<String, String>>,
) -> Result<CommandResult> {
    if !dir.exists() {
        outln!("\nNo directory found for {}", escape_path(dir))?;
        outln!(
            "\n{}",
            format!(
                "✗ {}: No directory found. Command: {} (Exit code: 1)",
                display_name(dir, config.root_dir.as_deref()),
                escape(&display_command(argv)),
            )
            .red()
        )?;
        return Ok(CommandResult {
            exit_code: Some(1),
            ..failed_result(dir, display_command(argv), String::new())
        });
    }

    let argv = match resolve_aliases(argv, &config.aliases) {
        Ok(argv) => argv,
        Err(e) => {
            if !config.silent {
                errln!("{e:#}");
            }
            return Ok(failed_result(
                dir,
                display_command(argv),
                format!("{e:#}\n"),
            ));
        }
    };
    let command = display_command(&argv);

    // Dry run mode: print what would be executed without running it
    if config.dry_run {
        outln!("{} {}", "[DRY RUN]".cyan(), dry_run_line(dir, &command))?;
        return Ok(CommandResult {
            success: true,
            exit_code: Some(0),
            directory: dir.to_path_buf(),
            command,
            ..Default::default()
        });
    }

    if config.verbose {
        outln!("Executing in directory: {}", escape_path(dir))?;
    }

    // Its name before what it prints (a prompt says which repo asks),
    // flushed before it writes to the same terminal
    let name = result_name(dir, config.root_dir.as_deref());
    if !config.silent {
        outln!("\n{}", format!("{name}:").bold())?;
        meta_core::out::flush()?;
    }

    let status = build_command(&argv, dir, extra_env).and_then(|mut cmd| {
        meta_core::children::status(
            cmd.stdout(if config.silent {
                Stdio::null()
            } else {
                Stdio::inherit()
            })
            .stderr(if config.silent {
                Stdio::null()
            } else {
                Stdio::inherit()
            }),
        )
        .with_context(|| format!("Failed to run {}", escape(&command)))
    });
    let result = match status {
        Ok(status) => {
            let (exit_code, signal) = exit_of(status);
            CommandResult {
                success: status.success(),
                exit_code,
                signal,
                directory: dir.to_path_buf(),
                command,
                // Sequential mode uses Stdio::inherit(), so no capture
                ..Default::default()
            }
        }
        Err(e) => {
            if !config.silent {
                errln!("{e:#}");
            }
            failed_result(dir, command, String::new())
        }
    };

    // A failure is said under what the command printed; the summary
    // counts the rest
    if !config.silent && !result.success {
        outln!("{}", format!("✗ {name}: {}", result.ending()).red())?;
    }

    Ok(result)
}

/// Capturing version for parallel execution - captures stdout/stderr for display after completion
fn execute_command_in_directory_capturing(
    dir: &Path,
    argv: &[String],
    config: &LoopConfig,
    extra_env: Option<&HashMap<String, String>>,
) -> CommandResult {
    if !dir.exists() {
        return CommandResult {
            exit_code: Some(1),
            ..failed_result(
                dir,
                display_command(argv),
                format!("Directory does not exist: {}", escape_path(dir)),
            )
        };
    }

    let argv = match resolve_aliases(argv, &config.aliases) {
        Ok(argv) => argv,
        Err(e) => return failed_result(dir, display_command(argv), format!("{e:#}\n")),
    };
    let command = display_command(&argv);

    // Dry run mode: return what would be executed without running it
    if config.dry_run {
        let stdout = format!("[DRY RUN] {}", dry_run_line(dir, &command)).into_bytes();
        return CommandResult {
            success: true,
            exit_code: Some(0),
            directory: dir.to_path_buf(),
            command,
            stdout,
            ..Default::default()
        };
    }

    let mut cmd_builder = match build_command(&argv, dir, extra_env) {
        Ok(cmd) => cmd,
        Err(e) => return failed_result(dir, command, format!("{e:#}\n")),
    };

    // Its output goes through a pipe to meta: it colours as meta does.
    // Tool-specific settings (git's GIT_CONFIG_*) are the plugin's to set;
    // loop_lib is tool-agnostic.
    meta_core::out::pass_color(&mut cmd_builder);

    let output = meta_core::children::output(&mut cmd_builder);

    match output {
        Ok(output) => {
            let (exit_code, signal) = exit_of(output.status);
            CommandResult {
                success: output.status.success(),
                exit_code,
                signal,
                directory: dir.to_path_buf(),
                command,
                stdout: output.stdout,
                stderr: output.stderr,
            }
        }
        Err(e) => failed_result(
            dir,
            command.clone(),
            format!("Failed to run {}: {e}\n", escape(&command)),
        ),
    }
}

/// Each of `directories` and the directories in it, in name order, without
/// those `ignore` names.
pub fn expand_directories(directories: &[PathBuf], ignore: &[String]) -> Result<Vec<PathBuf>> {
    let mut expanded = Vec::new();
    for dir in directories {
        if dir.is_dir() {
            // an excluded directory's children still count
            if !should_ignore(dir, ignore) {
                expanded.push(dir.clone());
            }
            let mut children = Vec::new();
            for entry in fs::read_dir(dir)? {
                let path = entry?.path();
                if path.is_dir() && !should_ignore(&path, ignore) {
                    children.push(path);
                }
            }
            children.sort();
            expanded.extend(children);
        }
    }
    Ok(expanded)
}

/// Run the same command across multiple directories.
pub fn run(config: &LoopConfig, argv: &[String]) -> Result<()> {
    let commands: Vec<DirCommand> = config
        .directories
        .iter()
        .map(|dir| DirCommand {
            dir: dir.clone(),
            argv: argv.to_vec(),
            env: config.env.clone(),
            refused: None,
        })
        .collect();

    run_commands(config, &commands)
}

/// JSON output structure for command results
#[derive(Debug, Serialize, Deserialize)]
pub struct JsonOutput {
    pub success: bool,
    pub results: Vec<JsonCommandResult>,
    pub summary: JsonSummary,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonCommandResult {
    pub directory: String,
    pub command: String,
    pub success: bool,
    /// null when a signal ended the command: see `signal`
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdout: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonSummary {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub dry_run: bool,
}

/// Spaces command starts at least `interval` apart.
struct Stagger {
    interval: Duration,
    /// When the last command started
    last: Mutex<Option<Instant>>,
}

impl Stagger {
    fn new(interval: Duration) -> Self {
        Stagger {
            interval,
            last: Mutex::new(None),
        }
    }

    /// Waits until `interval` after the last start, then counts this one as
    /// started. The others wait meanwhile, so no two start closer together.
    fn wait(&self) {
        self.wait_with(Instant::now, std::thread::sleep);
    }

    fn wait_with(&self, now: impl Fn() -> Instant, sleep: impl FnOnce(Duration)) {
        if self.interval.is_zero() {
            return;
        }
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(last) = *last {
            sleep(self.interval.saturating_sub(now().duration_since(last)));
        }
        *last = Some(now());
    }
}

/// Every command at once (up to `max_parallel`), each output captured and
/// shown once all are done, sorted for deterministic output: the root (".")
/// first, then by displayed path, component by component so nested repos sit
/// under their parent (`a/b` before `a-b`).
fn run_parallel(config: &LoopConfig, commands: &[DirCommand]) -> Result<Vec<CommandResult>> {
    let root = config.root_dir.as_deref();
    let progress = (io::stdout().is_terminal() && !config.json_output).then(MultiProgress::new);
    let spinner_style = ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {wide_msg}")
        .unwrap_or_else(|_| ProgressStyle::default_spinner())
        .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");

    // All spinners made up front, in order, so they show in that order
    let total = commands.len();
    let jobs: Vec<(&DirCommand, Option<ProgressBar>)> = commands
        .iter()
        .enumerate()
        .map(|(i, job)| {
            let spinner = progress.as_ref().map(|mp| {
                let pb = mp.add(ProgressBar::new_spinner());
                pb.set_style(spinner_style.clone());
                pb.set_prefix(format!("[{}/{}]", i + 1, total));
                pb.set_message(format!("{}: pending...", display_name(&job.dir, root)));
                pb.enable_steady_tick(Duration::from_millis(100));
                pb
            });
            (job, spinner)
        })
        .collect();

    let stagger = Stagger::new(Duration::from_millis(config.spawn_stagger_ms));
    let execute = || -> Vec<CommandResult> {
        jobs.par_iter()
            .map(|(job, spinner)| {
                stagger.wait();
                if let Some(pb) = spinner {
                    pb.set_message(format!("{}: running...", display_name(&job.dir, root)));
                }
                let result = match &job.refused {
                    Some(why) => refused_result(job, why),
                    None => execute_command_in_directory_capturing(
                        &job.dir,
                        &job.argv,
                        config,
                        job.env.as_ref(),
                    ),
                };
                // results are shown once all are done
                if let Some(pb) = spinner {
                    pb.finish_and_clear();
                }
                result
            })
            .collect()
    };
    let mut results = match config.max_parallel {
        Some(max) => ThreadPoolBuilder::new()
            .num_threads(max.get())
            .build()
            .context("Failed to create thread pool")?
            .install(execute),
        None => execute(),
    };
    if let Some(mp) = &progress {
        let _ = mp.clear();
    }

    results.sort_by_cached_key(|r| {
        let name = display_name(&r.directory, root);
        (name != ".", PathBuf::from(name))
    });
    Ok(results)
}

/// Every command in turn, its output live unless it goes into JSON.
fn run_sequential(config: &LoopConfig, commands: &[DirCommand]) -> Result<Vec<CommandResult>> {
    commands
        .iter()
        .map(|job| match (&job.refused, config.json_output) {
            (Some(why), json) => {
                if !json && !config.silent {
                    outln!(
                        "\n{}",
                        format!(
                            "✗ {}: {} (Exit code: 1)",
                            display_name(&job.dir, config.root_dir.as_deref()),
                            escape(why)
                        )
                        .red()
                    )?;
                }
                Ok(refused_result(job, why))
            }
            (None, true) => Ok(execute_command_in_directory_capturing(
                &job.dir,
                &job.argv,
                config,
                job.env.as_ref(),
            )),
            (None, false) => {
                execute_command_in_directory(&job.dir, &job.argv, config, job.env.as_ref())
            }
        })
        .collect()
}

/// Execute a list of commands (each with its own directory) and report how
/// they went. This is the execution engine for meta and its plugins.
pub fn run_commands(config: &LoopConfig, commands: &[DirCommand]) -> Result<()> {
    if commands.is_empty() {
        return Ok(());
    }
    let results = if config.parallel {
        let results = run_parallel(config, commands)?;
        if !config.silent && !config.json_output {
            print_captured(config, &results)?;
        }
        results
    } else {
        run_sequential(config, commands)?
    };

    let total = results.len();
    let failed_count = results.iter().filter(|r| !r.success).count();

    if config.json_output {
        let output = JsonOutput {
            success: failed_count == 0,
            summary: JsonSummary {
                total,
                succeeded: total - failed_count,
                failed: failed_count,
                dry_run: config.dry_run,
            },
            results: results
                .into_iter()
                .map(|r| JsonCommandResult {
                    directory: r.directory.display().to_string(),
                    command: r.command,
                    success: r.success,
                    exit_code: r.exit_code,
                    signal: r.signal,
                    stdout: String::from_utf8_lossy(&r.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&r.stderr).into_owned(),
                })
                .collect(),
        };
        outln!("{}", serde_json::to_string_pretty(&output)?)?;
    } else if !config.silent {
        if config.dry_run {
            outln!(
                "\n{} Would run {} command(s) across {} directories",
                "[DRY RUN]".cyan(),
                total.to_string().yellow(),
                total.to_string().yellow()
            )?;
        } else if failed_count == 0 {
            outln!("{} commands complete", total.to_string().green())?;
        } else {
            outln!(
                "\nSummary: {} {} out of {} commands failed",
                "✗".red(),
                failed_count.to_string().red(),
                total
            )?;
            for result in results.iter().filter(|r| !r.success) {
                // Without a root the basename is ambiguous, so keep the full path
                let name = match config.root_dir.as_deref() {
                    Some(root) => result_name(&result.directory, Some(root)),
                    None => escape_path(&result.directory),
                };
                outln!(
                    "\n{} {}: {} ({}) ",
                    "✗".red(),
                    name,
                    escape(&result.command),
                    result.ending()
                )?;
            }
            outln!()?;
        }
    }

    // In a dry run only commands that cannot run (a missing directory, a
    // broken alias) fail. The summary or the JSON said which; silenced,
    // the error says it.
    if failed_count > 0 {
        if config.silent && !config.json_output {
            anyhow::bail!("At least one command failed");
        }
        return Err(meta_core::exit::ExitWith(1).into());
    }

    Ok(())
}

/// The output parallel commands captured, each under its directory's name:
/// what a command wrote to stdout on stdout, to stderr on stderr, as it
/// wrote it.
fn print_captured(config: &LoopConfig, results: &[CommandResult]) -> Result<()> {
    let has_output =
        |r: &CommandResult| !r.stdout.trim_ascii().is_empty() || !r.stderr.trim_ascii().is_empty();
    if results.iter().any(has_output) {
        outln!()?;
    }
    for result in results.iter().filter(|r| has_output(r)) {
        let dir_name = result_name(&result.directory, config.root_dir.as_deref());
        if result.success {
            outln!("{} {}:", "✓".green(), dir_name.green())?;
        } else {
            outln!("{} {}:", "✗".red(), dir_name.red())?;
        }
        if !result.stdout.trim_ascii().is_empty() {
            meta_core::out::stdout_bytes(&result.stdout)?;
        }
        if !result.stderr.trim_ascii().is_empty() {
            // after the name and the stdout above it, with 2>&1 too
            meta_core::out::flush()?;
            meta_core::out::stderr_bytes(&result.stderr);
        }
        outln!()?; // Blank line after each repo's output
    }
    Ok(())
}

/// Whether the directory `path` is one an `ignore` entry names: by its name
/// (`.git`, `node_modules`) or the end of its path (`vendor/old`), matched
/// by whole components, so `.git` doesn't ignore `.github`. The directories
/// above it don't count: they are where loop runs, not what it selects.
pub fn should_ignore(path: &Path, ignore: &[String]) -> bool {
    // `./a` and `a` name the same directory
    let named = |path: &Path| -> PathBuf {
        path.components()
            .filter(|c| !matches!(c, std::path::Component::CurDir))
            .collect()
    };
    let path = named(path);
    ignore
        .iter()
        .filter(|entry| !entry.is_empty())
        .any(|entry| {
            let entry = named(Path::new(entry));
            // `.` names the directory loop runs in
            match entry.as_os_str().is_empty() {
                true => path.as_os_str().is_empty(),
                false => path.ends_with(entry),
            }
        })
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
