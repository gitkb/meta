use anyhow::{Context, Result};
use colored::*;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use meta_core::display::{escape, escape_path};
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoopConfig {
    #[serde(default)]
    pub directories: Vec<String>,
    #[serde(default)]
    pub ignore: Vec<String>,
    #[serde(default)]
    pub verbose: bool,
    #[serde(default)]
    pub silent: bool,
    #[serde(default)]
    pub add_aliases_to_global_looprc: bool,
    #[serde(default)]
    pub parallel: bool,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub json_output: bool,
    /// Milliseconds to wait between spawning threads in parallel mode.
    /// Default is 0 (no stagger). Set to e.g. 10 to spread out connections.
    #[serde(default)]
    pub spawn_stagger_ms: u64,
    /// Environment variables to set for all command subprocesses.
    /// Tool-specific env vars (e.g., GIT_PAGER) should be set by the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    /// Maximum number of commands to run in parallel (pool size limit).
    /// When set, limits the rayon thread pool to this many threads.
    /// Use for operations with shared resources (e.g., SSH ControlMaster
    /// has a default session limit of 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<usize>,
    /// The root directory of the meta workspace. When set, this directory
    /// is displayed as "." or ". (basename)" instead of its full path,
    /// and is sorted first in output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_dir: Option<PathBuf>,
}

/// A command to execute in a specific directory
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DirCommand {
    pub dir: String,
    /// The program and its arguments, run as given: no shell parses them.
    pub argv: Vec<String>,
    /// Environment variables to set for this command's subprocess
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
}

impl Default for LoopConfig {
    fn default() -> Self {
        LoopConfig {
            directories: vec![],
            ignore: vec![".git".to_string()],
            verbose: false,
            silent: false,
            add_aliases_to_global_looprc: false,
            parallel: false,
            dry_run: false,
            json_output: false,
            spawn_stagger_ms: 0,
            env: None,
            max_parallel: None,
            root_dir: None,
        }
    }
}

#[derive(Default)]
pub struct CommandResult {
    pub success: bool,
    pub exit_code: i32,
    pub directory: PathBuf,
    pub command: String,
    pub stdout: String,
    pub stderr: String,
}

pub fn load_aliases_from_file(path: &Path) -> Result<HashMap<String, String>> {
    let content = fs::read_to_string(path)?;
    let config: serde_json::Value = serde_json::from_str(&content)?;
    let aliases = config["aliases"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("No 'aliases' object found in config file"))?;
    Ok(aliases
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
        .collect())
}

fn prompt_user(question: &str) -> Result<bool> {
    print!("{question} [y/N]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_lowercase() == "y")
}

pub fn add_aliases_to_global_looprc() -> Result<()> {
    println!("Starting add_aliases_to_global_looprc function");

    let home = env::var("HOME").context("Failed to get HOME directory")?;
    let global_looprc = PathBuf::from(home).join(".looprc");
    println!("Global .looprc path: {global_looprc:?}");

    let mut aliases = HashMap::new();
    let mut existing_content = String::new();

    if global_looprc.exists() {
        println!("Global .looprc exists, loading existing aliases");
        existing_content = fs::read_to_string(&global_looprc)?;
        aliases = load_aliases_from_file(&global_looprc)?;
    } else {
        println!("Global .looprc does not exist");
        if !prompt_user("The global .looprc file does not exist. Do you want to create it?")? {
            println!("Operation cancelled by user.");
            return Ok(());
        }
    }

    if !prompt_user("Do you want to set the value of the 'aliases' property?")? {
        println!("Operation cancelled by user.");
        return Ok(());
    }

    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    println!("Using shell: {shell}");

    println!("Executing 'alias' command");
    let output = Command::new(&shell)
        .arg("-i")
        .arg("-c")
        .arg("alias")
        .output()?;

    println!("Processing 'alias' command output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some((alias, command)) = line.split_once('=') {
            let alias = alias.trim().trim_start_matches("alias ").to_string();
            let command = command
                .trim()
                .trim_matches('\'')
                .trim_matches('"')
                .to_string();
            aliases.insert(alias, command);
        }
    }

    println!("Creating config JSON");
    let config = serde_json::json!({
        "aliases": aliases
    });

    println!("Serializing config to string");
    let new_content = serde_json::to_string_pretty(&config)?;

    println!("\nPreview of changes:");
    print!("{}", preview(&existing_content, &new_content));

    if !prompt_user("Do you want to apply these changes?")? {
        println!("Operation cancelled by user.");
        return Ok(());
    }

    println!("Writing config to file");
    fs::write(global_looprc, new_content)?;

    println!("Aliases have been added to ~/.looprc");
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
fn resolve_aliases(argv: &[String], aliases: &HashMap<String, String>) -> Result<Vec<String>> {
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
                escape(&meta_core::git::redact_urls(alias))
            )
        })?;
    words.extend_from_slice(args);
    Ok(words)
}

/// The change from `existing` to `new` (a `.looprc`), line by line, without
/// the credentials in URLs: aliases may hold them.
fn preview(existing: &str, new: &str) -> String {
    let shown_new = shown_looprc(new);
    if existing.is_empty() {
        return format!("{}\n", shown_new.green());
    }
    let shown_old = shown_looprc(existing);
    let mut shown: String = diff::lines(&shown_old, &shown_new)
        .into_iter()
        .map(|change| match change {
            diff::Result::Left(l) => format!("{}\n", format!("-{l}").red()),
            diff::Result::Both(l, _) => format!(" {l}\n"),
            diff::Result::Right(r) => format!("{}\n", format!("+{r}").green()),
        })
        .collect();
    let json = |text: &str| serde_json::from_str::<serde_json::Value>(text).ok();
    if shown_old == shown_new && json(existing) != json(new) {
        shown.push_str("(the change is only in credentials, which aren't shown)\n");
    }
    shown
}

/// A `.looprc` as shown: its JSON written out again with the URLs in its
/// strings redacted, as an escape (`\/`, `\u0040`) may spell a URL no scan
/// of the text reads. Text that isn't JSON is shown redacted, unless it holds
/// a `\`.
fn shown_looprc(text: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(mut value) => {
            redact_json_strings(&mut value);
            serde_json::to_string_pretty(&value).unwrap_or_default()
        }
        Err(_) if text.contains('\\') => "<not shown: it may hold credentials>".to_string(),
        Err(_) => meta_core::git::redact_urls(text).into_owned(),
    }
}

/// Redact the URLs in every string and key of `value`.
fn redact_json_strings(value: &mut serde_json::Value) {
    use serde_json::Value;
    let redact = |s: &str| meta_core::git::redact_urls(s).into_owned();
    match value {
        Value::String(s) => *s = redact(s),
        Value::Array(items) => items.iter_mut().for_each(redact_json_strings),
        Value::Object(map) => {
            *map = std::mem::take(map)
                .into_iter()
                .map(|(key, mut value)| {
                    redact_json_strings(&mut value);
                    (redact(&key), value)
                })
                .collect();
        }
        _ => {}
    }
}

/// `argv` as one line, quoted so that it reads back as the same words, with
/// the credentials in URLs hidden: it is for showing, never for running.
pub fn display_command(argv: &[String]) -> String {
    let shown: Vec<String> = argv
        .iter()
        .map(|arg| meta_core::git::redact_urls(arg).into_owned())
        .collect();
    shlex::try_join(shown.iter().map(String::as_str)).unwrap_or_else(|_| shown.join(" "))
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
        exit_code: -1,
        directory: dir.to_path_buf(),
        command,
        stdout: String::new(),
        stderr,
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

/// A write to command-output stdout failed. Kept distinct from child stdin errors.
#[derive(Debug)]
pub struct StdoutError(pub io::Error);

impl std::fmt::Display for StdoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Failed to write command output: {}", self.0)
    }
}

impl std::error::Error for StdoutError {}

macro_rules! command_print {
    ($($arg:tt)*) => { write!(io::stdout().lock(), $($arg)*).map_err(StdoutError)? };
}
macro_rules! command_println {
    ($($arg:tt)*) => { writeln!(io::stdout().lock(), $($arg)*).map_err(StdoutError)? };
}

pub fn execute_command_in_directory(
    dir: &Path,
    argv: &[String],
    config: &LoopConfig,
    aliases: &HashMap<String, String>,
    extra_env: Option<&HashMap<String, String>>,
) -> Result<CommandResult> {
    if !dir.exists() {
        command_println!("\nNo directory found for {}", escape_path(dir));
        command_println!(
            "\x1b[31m\n✗ {}: No directory found. Command: {} (Exit code: {})\x1b[0m",
            display_name(dir, config.root_dir.as_deref()),
            escape(&display_command(argv)),
            1
        );
        return Ok(CommandResult {
            exit_code: 1,
            ..failed_result(dir, display_command(argv), String::new())
        });
    }

    let argv = match resolve_aliases(argv, aliases) {
        Ok(argv) => argv,
        Err(e) => {
            if !config.silent {
                eprintln!("{e:#}");
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
        command_println!("{} {}", "[DRY RUN]".cyan(), dry_run_line(dir, &command));
        return Ok(CommandResult {
            success: true,
            exit_code: 0,
            directory: dir.to_path_buf(),
            command,
            stdout: String::new(),
            stderr: String::new(),
        });
    }

    if config.verbose {
        command_println!("Executing in directory: {}", escape_path(dir));
    }

    if !config.silent {
        command_println!();
        io::stdout().flush().map_err(StdoutError)?;
    }

    let status = build_command(&argv, dir, extra_env).and_then(|mut cmd| {
        cmd.stdout(if config.silent {
            Stdio::null()
        } else {
            Stdio::inherit()
        })
        .stderr(if config.silent {
            Stdio::null()
        } else {
            Stdio::inherit()
        })
        .status()
        .with_context(|| format!("Failed to run {}", escape(&command)))
    });
    let (success, exit_code) = match status {
        Ok(status) => (status.success(), status.code().unwrap_or(-1)),
        Err(e) => {
            if !config.silent {
                eprintln!("{e:#}");
            }
            (false, -1)
        }
    };

    if !config.silent {
        let name = result_name(dir, config.root_dir.as_deref());
        if success {
            command_println!("\x1b[32m\n✓ {name}\x1b[0m");
        } else {
            command_println!("\x1b[31m\n✗ {name}: exited code {exit_code}\x1b[0m");
        }
        io::stdout().flush().map_err(StdoutError)?;
    }

    Ok(CommandResult {
        success,
        exit_code,
        directory: dir.to_path_buf(),
        command,
        stdout: String::new(), // Sequential mode uses Stdio::inherit(), so no capture
        stderr: String::new(),
    })
}

/// Capturing version for parallel execution - captures stdout/stderr for display after completion
pub fn execute_command_in_directory_capturing(
    dir: &Path,
    argv: &[String],
    config: &LoopConfig,
    aliases: &HashMap<String, String>,
    extra_env: Option<&HashMap<String, String>>,
) -> CommandResult {
    if !dir.exists() {
        return CommandResult {
            exit_code: 1,
            ..failed_result(
                dir,
                display_command(argv),
                format!("Directory does not exist: {}", escape_path(dir)),
            )
        };
    }

    let argv = match resolve_aliases(argv, aliases) {
        Ok(argv) => argv,
        Err(e) => return failed_result(dir, display_command(argv), format!("{e:#}\n")),
    };
    let command = display_command(&argv);

    // Dry run mode: return what would be executed without running it
    if config.dry_run {
        let stdout = format!("[DRY RUN] {}", dry_run_line(dir, &command));
        return CommandResult {
            success: true,
            exit_code: 0,
            directory: dir.to_path_buf(),
            command,
            stdout,
            stderr: String::new(),
        };
    }

    let mut cmd_builder = match build_command(&argv, dir, extra_env) {
        Ok(cmd) => cmd,
        Err(e) => return failed_result(dir, command, format!("{e:#}\n")),
    };

    // Force color output when parent stdout is a TTY
    // This is needed because piped stdout makes child processes think they're not in a terminal
    // Note: Tool-specific color vars (e.g., git's GIT_CONFIG_*) should be set by the
    // plugin that knows about that tool, not here. loop_lib is tool-agnostic.
    if io::stdout().is_terminal() {
        cmd_builder
            .env("FORCE_COLOR", "1") // Node.js ecosystem
            .env("CLICOLOR_FORCE", "1"); // BSD/macOS convention
                                         // Preserve TERM if set, otherwise provide a reasonable default
        if env::var("TERM").is_err() {
            cmd_builder.env("TERM", "xterm-256color");
        }
    }

    let output = cmd_builder
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    match output {
        Ok(output) => {
            let success = output.status.success();
            let exit_code = output.status.code().unwrap_or(-1);
            CommandResult {
                success,
                exit_code,
                directory: dir.to_path_buf(),
                command,
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            }
        }
        Err(e) => failed_result(
            dir,
            command.clone(),
            format!("Failed to run {}: {e}\n", escape(&command)),
        ),
    }
}

pub fn expand_directories(directories: &[String], ignore: &[String]) -> Result<Vec<String>> {
    let mut expanded = Vec::new();

    use std::fs;

    for dir in directories {
        let dir_path = PathBuf::from(dir);
        if dir_path.is_dir() {
            // an excluded directory's children still count
            if !should_ignore(&dir_path, ignore) {
                expanded.push(dir_path.to_string_lossy().into_owned());
            }

            for entry in fs::read_dir(&dir_path)? {
                let entry = entry?;
                let path = entry.path();
                if path.is_dir() && !should_ignore(&path, ignore) {
                    expanded.push(path.to_string_lossy().into_owned());
                }
            }
        }
    }

    Ok(expanded)
}

/// Run the same command across multiple directories.
pub fn run(orig_config: &LoopConfig, argv: &[String]) -> Result<()> {
    // Handle special case: add_aliases_to_global_looprc
    if orig_config.add_aliases_to_global_looprc {
        if orig_config.dry_run {
            println!("[DRY RUN] Would add your shell's aliases to ~/.looprc");
            return Ok(());
        }
        return add_aliases_to_global_looprc();
    }

    // Build DirCommand list with same command for each directory
    let commands: Vec<DirCommand> = orig_config
        .directories
        .iter()
        .map(|dir| DirCommand {
            dir: dir.clone(),
            argv: argv.to_vec(),
            env: orig_config.env.clone(),
        })
        .collect();

    run_commands(orig_config, &commands)
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
    pub exit_code: i32,
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

// ============================================================================
// Unified Execution Engine
// ============================================================================

/// Internal execution engine that handles both parallel and sequential execution.
/// This is the unified implementation used by both `run()` and `run_commands()`.
fn execute_commands_internal(config: &LoopConfig, commands: &[DirCommand]) -> Result<()> {
    if commands.is_empty() {
        return Ok(());
    }

    let results = Arc::new(Mutex::new(Vec::new()));
    let aliases = Arc::new(get_aliases());

    if config.parallel {
        // Parallel execution using rayon thread pool with spinners
        let is_tty = std::io::stdout().is_terminal() && !config.json_output;
        let mp = if is_tty {
            Some(Arc::new(MultiProgress::new()))
        } else {
            None
        };
        let spinner_style = ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {wide_msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner())
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");

        let total = commands.len();

        // Pre-create all spinners (in order) so they display in correct sequence
        let spinners: Vec<Option<ProgressBar>> = commands
            .iter()
            .enumerate()
            .map(|(i, dir_cmd)| {
                if let Some(ref mp) = mp {
                    let pb = mp.add(ProgressBar::new_spinner());
                    pb.set_style(spinner_style.clone());
                    pb.set_prefix(format!("[{}/{}]", i + 1, total));
                    let dir_name =
                        display_name(Path::new(&dir_cmd.dir), config.root_dir.as_deref());
                    pb.set_message(format!("{dir_name}: pending..."));
                    pb.enable_steady_tick(Duration::from_millis(100));
                    Some(pb)
                } else {
                    None
                }
            })
            .collect();

        // Atomic counter for staggered spawning - prevents SSH socket saturation
        let spawn_counter = Arc::new(AtomicUsize::new(0));
        let stagger_ms = config.spawn_stagger_ms;

        // Closure to execute commands in parallel
        let execute_parallel = || {
            commands
                .par_iter()
                .enumerate()
                .map(|(i, dir_cmd)| {
                    // Apply stagger delay to spread out connection attempts.
                    // Each thread gets a slot number and sleeps proportionally.
                    if stagger_ms > 0 {
                        let slot = spawn_counter.fetch_add(1, Ordering::SeqCst);
                        let delay = Duration::from_millis(stagger_ms * slot as u64);
                        std::thread::sleep(delay);
                    }

                    let dir = PathBuf::from(&dir_cmd.dir);
                    let dir_name = display_name(&dir, config.root_dir.as_deref());

                    // Update spinner to show running
                    if let Some(ref pb) = spinners[i] {
                        pb.set_message(format!("{dir_name}: running..."));
                    }

                    let result = execute_command_in_directory_capturing(
                        &dir,
                        &dir_cmd.argv,
                        config,
                        &aliases,
                        dir_cmd.env.as_ref(),
                    );

                    // Update spinner with result (only if not JSON output)
                    // Note: We don't print completion status here - detailed results shown after all complete
                    if !config.json_output {
                        if let Some(ref pb) = spinners[i] {
                            // Clear spinner - detailed results shown later
                            pb.finish_and_clear();
                        }
                        // Non-TTY: no per-command output, detailed results shown after all complete
                    }

                    result
                })
                .collect()
        };

        // Use custom thread pool if max_parallel is set, otherwise use global pool
        let parallel_results: Vec<CommandResult> = if let Some(max) = config.max_parallel {
            // Create a custom thread pool with limited threads
            let pool = ThreadPoolBuilder::new()
                .num_threads(max)
                .build()
                .context("Failed to create thread pool")?;
            pool.install(execute_parallel)
        } else {
            execute_parallel()
        };

        // Store results (already collected from rayon), sorted for deterministic output:
        // the root (".") first, then by displayed path, component by component so
        // nested repos sit under their parent (`a/b` before `a-b`)
        let mut sorted_results = parallel_results;
        sorted_results.sort_by_cached_key(|r| {
            let name = display_name(&r.directory, config.root_dir.as_deref());
            (name != ".", PathBuf::from(name))
        });
        results
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .extend(sorted_results);

        // Clear spinner lines before showing detailed output
        if let Some(ref mp) = mp {
            let _ = mp.clear();
        }

        // Print captured output after all spinners complete (if not JSON)
        if !config.silent && !config.json_output {
            let results = results.lock().unwrap_or_else(|e| e.into_inner());
            let has_any_output = results
                .iter()
                .any(|r| !r.stdout.trim().is_empty() || !r.stderr.trim().is_empty());

            if has_any_output {
                command_println!();
            }

            for result in results.iter() {
                let dir_name = result_name(&result.directory, config.root_dir.as_deref());

                let has_output =
                    !result.stdout.trim().is_empty() || !result.stderr.trim().is_empty();
                if has_output {
                    if result.success {
                        command_println!("{} {}:", "✓".green(), dir_name.green());
                    } else {
                        command_println!("{} {}:", "✗".red(), dir_name.red());
                    }
                    if !result.stdout.trim().is_empty() {
                        command_print!("{}", result.stdout);
                    }
                    if !result.stderr.trim().is_empty() {
                        command_print!("{}", result.stderr);
                    }
                    command_println!(); // Blank line after each repo's output
                }
            }
        }
    } else {
        // Sequential execution
        for dir_cmd in commands {
            let dir = PathBuf::from(&dir_cmd.dir);
            let result = if config.json_output {
                // Capture output for JSON mode
                execute_command_in_directory_capturing(
                    &dir,
                    &dir_cmd.argv,
                    config,
                    &aliases,
                    dir_cmd.env.as_ref(),
                )
            } else {
                execute_command_in_directory(
                    &dir,
                    &dir_cmd.argv,
                    config,
                    &aliases,
                    dir_cmd.env.as_ref(),
                )?
            };
            results
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(result);
        }
    }

    // Build results summary
    let results = results.lock().unwrap_or_else(|e| e.into_inner());
    let total = results.len();
    let failed: Vec<_> = results.iter().filter(|r| !r.success).collect();
    let failed_count = failed.len();

    // Output results
    if config.json_output {
        // JSON output mode
        let json_results: Vec<JsonCommandResult> = results
            .iter()
            .map(|r| JsonCommandResult {
                directory: r.directory.display().to_string(),
                command: r.command.clone(),
                success: r.success,
                exit_code: r.exit_code,
                stdout: r.stdout.clone(),
                stderr: r.stderr.clone(),
            })
            .collect();

        let output = JsonOutput {
            success: failed_count == 0,
            results: json_results,
            summary: JsonSummary {
                total,
                succeeded: total - failed_count,
                failed: failed_count,
                dry_run: config.dry_run,
            },
        };

        command_println!("{}", serde_json::to_string_pretty(&output)?);
    } else if !config.silent {
        // Text output mode
        if config.dry_run {
            command_println!(
                "\n{} Would run {} command(s) across {} directories",
                "[DRY RUN]".cyan(),
                total.to_string().yellow(),
                total.to_string().yellow()
            );
        } else if failed_count == 0 {
            command_println!("{} commands complete", total.to_string().green());
        } else {
            command_println!(
                "\nSummary: {} {} out of {} commands failed",
                "✗".red(),
                failed_count.to_string().red(),
                total
            );
            for result in &failed {
                // Without a root the basename is ambiguous, so keep the full path
                let name = match config.root_dir.as_deref() {
                    Some(root) => result_name(&result.directory, Some(root)),
                    None => escape_path(&result.directory),
                };
                command_println!(
                    "\n{} {}: {} (Exit code {}) ",
                    "✗".red(),
                    name,
                    escape(&result.command),
                    result.exit_code
                );
            }
            command_println!();
        }
    }

    // In a dry run only commands that cannot run (a missing directory, a
    // broken alias) fail
    if failed_count > 0 {
        return Err(anyhow::anyhow!("At least one command failed"));
    }

    Ok(())
}

/// Execute a list of commands (each with its own directory)
/// This is the unified execution engine for plugins.
pub fn run_commands(config: &LoopConfig, commands: &[DirCommand]) -> Result<()> {
    execute_commands_internal(config, commands)
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

pub fn parse_config(config_path: &Path) -> Result<LoopConfig> {
    let config_str = fs::read_to_string(config_path)
        .with_context(|| format!("Failed to read looprc config file: {config_path:?}"))?;
    let config: LoopConfig = serde_json::from_str(&config_str)
        .with_context(|| format!("Failed to parse looprc config file: {config_path:?}"))?;
    Ok(config)
}

pub fn get_aliases() -> HashMap<String, String> {
    let mut aliases = HashMap::new();

    if let Some(home) = env::var_os("HOME") {
        let global_looprc = PathBuf::from(home).join(".looprc");
        if global_looprc.exists() {
            if let Ok(global_aliases) = load_aliases_from_file(&global_looprc) {
                aliases.extend(global_aliases);
            }
        }
    }

    if aliases.is_empty() {
        if let Ok(output) = Command::new("sh").arg("-c").arg("alias").output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if let Some((alias, command)) = line.split_once('=') {
                    let alias = alias.trim().trim_start_matches("alias ").to_string();
                    let command = command
                        .trim()
                        .trim_matches('\'')
                        .trim_matches('"')
                        .to_string();
                    aliases.insert(alias, command);
                }
            }
        }
    }

    if let Ok(local_aliases) = load_aliases_from_file(Path::new(".looprc")) {
        aliases.extend(local_aliases);
    }

    aliases
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
