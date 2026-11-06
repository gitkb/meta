use anyhow::{Context, Result};
use colored::*;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
pub use meta_plugin::display::display_command;
use meta_plugin::display::{escape, escape_path, program_output};
use meta_plugin::process::TooMuchOutput;
use meta_plugin::{errln, out, outln};
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs;
use std::io::{self, IsTerminal};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// A write to command-output stdout failed (meta's one stdout error).
pub use meta_plugin::out::StdoutError;

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
    /// The program that runs loop, when its options may be typed after the
    /// command by mistake
    #[serde(skip)]
    pub outer: Option<Outer>,
    /// Warnings about the run, from whoever planned it: printed before the
    /// commands run, or with `json_output` listed in the report
    #[serde(skip)]
    pub warnings: Vec<String>,
    /// Told of each command as it ends (in parallel, from its thread)
    #[serde(skip)]
    pub on_done: OnDone,
}

/// A function told of each command as it ends; a closed stdout it found
/// stops the run.
type Tell = dyn Fn(&CommandResult) -> Result<(), StdoutError> + Send + Sync;

/// What a caller is told as each command ends.
#[derive(Clone, Default)]
pub struct OnDone(Option<std::sync::Arc<Tell>>);

impl OnDone {
    pub fn call(
        f: impl Fn(&CommandResult) -> Result<(), StdoutError> + Send + Sync + 'static,
    ) -> Self {
        OnDone(Some(std::sync::Arc::new(f)))
    }

    fn tell(&self, result: &CommandResult) -> Result<(), StdoutError> {
        match &self.0 {
            Some(f) => f(result),
            None => Ok(()),
        }
    }
}

impl std::fmt::Debug for OnDone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "OnDone(..)"
        } else {
            "OnDone(none)"
        })
    }
}

/// meta, running commands through loop: when every command fails because
/// its program rejected one of meta's options, a tip after the summary says
/// where it goes.
#[derive(Debug, Clone, Default)]
pub struct Outer {
    pub meta: meta_plugin::args::MetaOptions,
    /// The words between meta's options and the command (`exec --`)
    pub before: Vec<String>,
}

impl Outer {
    /// The tip for `results`, the commands of `argv` having all failed:
    /// for the option of meta's that each one's error output says it
    /// rejected (git: ``unknown option `recursive'``, `unrecognized
    /// argument: --json`; clap: `unexpected argument '--recursive'`).
    /// Output that wasn't captured (`--sequential`) gives none.
    fn tip(&self, argv: &[String], results: &[CommandResult]) -> Option<String> {
        let typed = |spelling: &str| {
            argv.iter().skip(1).find(|arg| {
                *arg == spelling
                    || spelling.starts_with("--") && arg.starts_with(&format!("{spelling}="))
            })
        };
        let rejected = |spelling: &str, result: &CommandResult| {
            let name = spelling.trim_start_matches('-');
            String::from_utf8_lossy(&result.stderr).lines().any(|line| {
                let lower = line.to_lowercase();
                [
                    "unknown option",
                    "unknown switch",
                    "unrecognized option",
                    "unrecognized argument",
                    "unexpected argument",
                ]
                .iter()
                .any(|says| lower.contains(says))
                    && (line.contains(&format!("`{name}'"))
                        || line.contains(&format!("`{name}="))
                        || line.contains(&format!("'{spelling}'"))
                        || line.contains(&format!("'{spelling}="))
                        || line.trim_end().ends_with(&format!(": {spelling}")))
            })
        };
        let word = self
            .meta
            .options
            .iter()
            .flat_map(|option| &option.spellings)
            .filter(|spelling| results.iter().all(|r| rejected(spelling, r)))
            .find_map(|spelling| typed(spelling))?;
        let command: Vec<String> = self.before.iter().chain(argv).cloned().collect();
        // the commands' programs can't be asked whether the line would do:
        // none is given while another long option of meta's stays after the
        // command (git names only the first word it rejects)
        let ours = |word: &String| {
            let spelling = word.split('=').next().unwrap_or(word);
            spelling.starts_with("--")
                && self
                    .meta
                    .options
                    .iter()
                    .any(|o| o.spellings.iter().any(|s| s == spelling))
        };
        let check = |_: &[String], rest: &[String]| match rest.iter().any(ours) {
            true => Err(None),
            false => Ok(()),
        };
        self.meta.tip(&command, word, &check)
    }
}

/// A command to execute in a specific directory
#[derive(Debug, Clone)]
pub struct DirCommand {
    /// The project it runs for, by its path from the workspace root (the
    /// Envelope names its row so)
    pub project: Option<String>,
    pub dir: PathBuf,
    /// The program and its arguments, run as given: no shell parses them.
    pub argv: Vec<String>,
    /// Environment variables to set for this command's subprocess
    pub env: Option<HashMap<String, String>>,
    /// Why the command must not run here (its folder is no project the
    /// caller acts in, say): reported as a failed result, never run.
    pub refused: Option<String>,
    /// The exit codes that are success (`[0]` for most programs)
    pub ok_codes: Vec<i32>,
}

impl DirCommand {
    /// `argv` in `dir`, with exit code 0 its only success.
    pub fn new(dir: PathBuf, argv: Vec<String>) -> Self {
        DirCommand {
            project: None,
            dir,
            argv,
            env: None,
            refused: None,
            ok_codes: vec![0],
        }
    }
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
            outer: None,
            warnings: vec![],
            on_done: OnDone::default(),
        }
    }
}

#[derive(Default)]
pub struct CommandResult {
    /// The project it ran for (see [`DirCommand::project`])
    pub project: Option<String>,
    pub success: bool,
    /// Whether it ran: false in a dry run, and when it couldn't start
    pub ran: bool,
    /// None when a signal ended the command; -1 when it never started
    pub exit_code: Option<i32>,
    /// The signal that ended the command (unix)
    pub signal: Option<i32>,
    pub directory: PathBuf,
    pub command: String,
    /// The program and arguments, aliases expanded
    pub argv: Vec<String>,
    /// What the command wrote, as it wrote it (not necessarily UTF-8)
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// Its output went straight to meta's (`--sequential` text), so none
    /// of it is here
    pub live: bool,
    /// It wrote more than meta keeps of one command, so it was stopped and
    /// its output dropped
    pub too_much_output: Option<TooMuchOutput>,
}

impl CommandResult {
    /// How the command ended, for a failure line: `exit code 1`,
    /// `killed by signal 9`.
    fn ending(&self) -> String {
        match (self.too_much_output, self.signal, self.exit_code) {
            (Some(over), _, _) => format!("stopped at {} MiB of output", over.limit >> 20),
            (None, Some(signal), _) => format!("killed by signal {signal}"),
            (None, None, Some(code)) => format!("exit code {code}"),
            (None, None, None) => "no exit code".to_string(),
        }
    }

    /// Why it failed, for the end of a line (`: why`): the last line of
    /// its stderr that has anything on it, or that it wrote none. Nothing for
    /// a command that ran live: its stderr was the user's, shown and not kept.
    fn why(&self) -> String {
        if self.live {
            return String::new();
        }
        if self.too_much_output.is_some() {
            return ": no more of one command's output is kept".to_string();
        }
        String::from_utf8_lossy(&self.stderr)
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map_or_else(
                || ": no stderr".to_string(),
                |line| format!(": {}", escape(line)),
            )
    }
}

/// Whether a command that ended so succeeded: no signal, and an exit code
/// among `ok_codes`.
fn accepted(exit_code: Option<i32>, signal: Option<i32>, ok_codes: &[i32]) -> bool {
    signal.is_none() && exit_code.is_some_and(|code| ok_codes.contains(&code))
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
    meta_plugin::out::flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_lowercase() == "y")
}

/// The aliases your interactive shell defines (`alias` in `$SHELL -i`).
fn shell_aliases() -> Result<BTreeMap<String, String>> {
    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    #[expect(
        clippy::disallowed_methods,
        reason = "the user's shell, in no repo: it lists their aliases"
    )]
    let output = Command::new(&shell)
        .args(["-i", "-c", "alias"])
        .output()
        .with_context(|| format!("Failed to run {}", escape(&shell)))?;
    anyhow::ensure!(
        output.status.success(),
        "`{} -i -c alias` failed:{}",
        escape(&shell),
        program_output(&output.stderr)
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
    // Each directory is its own repository: a GIT_DIR from a git hook around
    // meta would point git in every one of them at the hook's
    let mut cmd = meta_core::git::in_repo(program, dir);
    cmd.args(args);
    // Plugin-specified environment variables (e.g., GIT_PAGER=cat)
    if let Some(extra) = extra_env {
        cmd.envs(extra);
    }
    Ok(cmd)
}

fn failed_result(dir: &Path, command: String, stderr: String) -> CommandResult {
    CommandResult {
        project: None,
        success: false,
        ran: false,
        argv: Vec::new(),
        exit_code: Some(-1),
        signal: None,
        directory: dir.to_path_buf(),
        command,
        stdout: Vec::new(),
        stderr: stderr.into_bytes(),
        ..CommandResult::default()
    }
}

/// The failed result of a command that was refused (see [`DirCommand::refused`]):
/// `why` as its error output, exit code 1, nothing run.
fn refused_result(job: &DirCommand, why: &str) -> CommandResult {
    CommandResult {
        project: job.project.clone(),
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

fn execute_command_in_directory(job: &DirCommand, config: &LoopConfig) -> Result<CommandResult> {
    let result =
        execute_in_directory(&job.dir, &job.argv, config, job.env.as_ref(), &job.ok_codes)?;
    Ok(CommandResult {
        project: job.project.clone(),
        ..result
    })
}

fn execute_in_directory(
    dir: &Path,
    argv: &[String],
    config: &LoopConfig,
    extra_env: Option<&HashMap<String, String>>,
    ok_codes: &[i32],
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
            argv,
            ..Default::default()
        });
    }

    if config.verbose {
        errln!("Executing in directory: {}", escape_path(dir));
    }

    // Its name before what it prints (a prompt says which repo asks),
    // flushed before it writes to the same terminal
    let name = result_name(dir, config.root_dir.as_deref());
    if !config.silent {
        outln!("\n{}", format!("{name}:").bold())?;
        meta_plugin::out::flush()?;
    }

    // Live, its output the user's own, unless silenced: then it goes nowhere
    let ran = build_command(&argv, dir, extra_env).and_then(|mut cmd| {
        if config.silent {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
        meta_plugin::process::status(&mut cmd).map_err(|e| anyhow::anyhow!(not_started(&argv, &e)))
    });
    let result = match ran {
        Ok(status) => {
            let (exit_code, signal) = exit_of(status);
            CommandResult {
                success: accepted(exit_code, signal, ok_codes),
                ran: true,
                exit_code,
                signal,
                directory: dir.to_path_buf(),
                command,
                argv,
                live: true,
                ..Default::default()
            }
        }
        Err(e) => {
            if !config.silent {
                errln!("{e:#}");
            }
            failed_result(dir, command, format!("{e:#}\n"))
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
fn execute_command_in_directory_capturing(job: &DirCommand, config: &LoopConfig) -> CommandResult {
    CommandResult {
        project: job.project.clone(),
        ..capture_in_directory(&job.dir, &job.argv, config, job.env.as_ref(), &job.ok_codes)
    }
}

fn capture_in_directory(
    dir: &Path,
    argv: &[String],
    config: &LoopConfig,
    extra_env: Option<&HashMap<String, String>>,
    ok_codes: &[i32],
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
            argv,
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
    meta_plugin::out::pass_color(&mut cmd_builder);

    // Silenced (and no JSON wants its output): it goes nowhere, nothing held
    let output = match config.silent && !config.json_output {
        true => meta_plugin::process::status(
            cmd_builder
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .map(|status| std::process::Output {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        }),
        false => meta_plugin::process::output(&mut cmd_builder),
    };

    match output {
        Ok(output) => {
            let (exit_code, signal) = exit_of(output.status);
            CommandResult {
                project: None,
                success: accepted(exit_code, signal, ok_codes),
                ran: true,
                exit_code,
                signal,
                directory: dir.to_path_buf(),
                command,
                argv,
                stdout: output.stdout,
                stderr: output.stderr,
                ..CommandResult::default()
            }
        }
        Err(e) => match TooMuchOutput::of(&e) {
            Some(over) => too_much(dir, command, argv, over),
            None => failed_result(dir, command, format!("{}\n", not_started(&argv, &e))),
        },
    }
}

/// The result of a command that wrote more than meta keeps of one: it ran,
/// was stopped, and nothing of its output is kept.
fn too_much(dir: &Path, command: String, argv: Vec<String>, over: TooMuchOutput) -> CommandResult {
    CommandResult {
        ran: true,
        exit_code: None,
        argv,
        too_much_output: Some(over),
        ..failed_result(dir, command, String::new())
    }
}

/// Why `argv` didn't start; for one argument with a space in it that names
/// no program (`"git status"`), also that a program and its arguments are
/// separate words.
fn not_started(argv: &[String], error: &std::io::Error) -> String {
    let command = display_command(argv);
    let mut said = format!("Failed to run {}: {error}", escape(&command));
    if let [one] = argv {
        // a path with a space in it is one word
        if error.kind() == std::io::ErrorKind::NotFound
            && one.contains(char::is_whitespace)
            && !one.contains(std::path::is_separator)
        {
            // read as a shell would (`git commit -m "a b"` is four words)
            if let Some(words) = shlex::split(one) {
                said.push_str(&format!(
                    "\n  a program and its arguments are separate words: {}, not {}",
                    escape(&display_command(&words)),
                    escape(&command)
                ));
            }
        }
    }
    said
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
pub fn run(config: &LoopConfig, argv: &[String]) -> Result<Outcome> {
    let commands: Vec<DirCommand> = config
        .directories
        .iter()
        .map(|dir| DirCommand {
            env: config.env.clone(),
            ..DirCommand::new(dir.clone(), argv.to_vec())
        })
        .collect();

    run_commands(config, &commands)
}

/// How a run of commands went.
pub struct Outcome {
    /// One per command, in the order shown
    pub results: Vec<CommandResult>,
    /// The tip the text output ended with (an option of meta's typed
    /// after the command, when every command rejected it)
    pub tip: Option<String>,
    /// Whether the commands were only shown (`--dry-run`)
    pub dry_run: bool,
    /// The summary is a line of its own when output is silenced
    silent: bool,
}

impl Outcome {
    /// How many commands failed.
    pub fn failed(&self) -> usize {
        self.results.iter().filter(|r| !r.success).count()
    }

    /// Why the run failed, when a command did: `tool-failed`, with the tip.
    pub fn error(&self) -> Option<meta_plugin::ErrorInfo> {
        let failed = self.failed();
        if failed == 0 {
            return None;
        }
        let mut error = meta_plugin::ErrorInfo::new(
            meta_plugin::ErrorCode::ToolFailed,
            format!("{failed} of {} commands failed", self.results.len()),
        );
        error.hint = self.tip.clone();
        Some(error)
    }

    /// The run's end for a caller that shows text: done, or the failure the
    /// summary already named (with output silenced, the repos that failed).
    pub fn finish(&self) -> Result<()> {
        match self.error() {
            None => Ok(()),
            Some(_) if self.silent => {
                let failed = self.results.iter().filter(|r| !r.success);
                let names: Vec<String> = failed.map(|r| escape_path(&r.directory)).collect();
                anyhow::bail!("{}", failed_line(&names))
            }
            Some(_) => Err(meta_plugin::exit::ExitWith(1).into()),
        }
    }

    /// Each command as a row of the Envelope.
    pub fn rows(&self) -> Vec<meta_plugin::ProjectResult> {
        self.results.iter().map(CommandResult::row).collect()
    }
}

impl CommandResult {
    /// This result as a row of the Envelope: a dry run's has no exit code,
    /// a failure's `error` says how it ended.
    pub fn row(&self) -> meta_plugin::ProjectResult {
        let text = |bytes: &[u8]| Some(String::from_utf8_lossy(bytes).into_owned());
        let error = (!self.success).then(|| {
            let stderr = String::from_utf8_lossy(&self.stderr);
            match (self.too_much_output, self.ran) {
                (Some(over), _) => {
                    let name = match &self.project {
                        Some(project) => project.clone(),
                        None => self.directory.display().to_string(),
                    };
                    meta_plugin::ErrorInfo::new(
                        meta_plugin::ErrorCode::TooMuchOutput,
                        format!("{name}: {over}"),
                    )
                }
                (None, true) => {
                    meta_plugin::ErrorInfo::new(meta_plugin::ErrorCode::ToolFailed, self.ending())
                }
                (None, false) => meta_plugin::ErrorInfo::new(
                    meta_plugin::ErrorCode::ToolFailed,
                    stderr.trim_end(),
                ),
            }
        });
        meta_plugin::ProjectResult {
            project: self.project.clone(),
            argv: Some(match self.argv.is_empty() {
                true => vec![self.command.clone()],
                false => self.argv.clone(),
            }),
            exit_code: self.exit_code.filter(|_| self.ran),
            signal: self.signal,
            stdout: text(&self.stdout),
            stderr: text(&self.stderr),
            error,
            ..meta_plugin::ProjectResult::default()
        }
        .at(&self.directory)
    }
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
    // a closed stdout, found telling of a command: no other one starts
    let closed: Mutex<Option<StdoutError>> = Mutex::new(None);
    let is_closed = || {
        closed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    };
    let execute = || -> Vec<CommandResult> {
        jobs.par_iter()
            .map(|(job, spinner)| {
                if is_closed() {
                    return failed_result(&job.dir, display_command(&job.argv), String::new());
                }
                stagger.wait();
                if let Some(pb) = spinner {
                    pb.set_message(format!("{}: running...", display_name(&job.dir, root)));
                }
                let result = match &job.refused {
                    Some(why) => refused_result(job, why),
                    None => execute_command_in_directory_capturing(job, config),
                };
                if let Err(e) = config.on_done.tell(&result) {
                    closed
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .get_or_insert(e);
                }
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
    if let Some(e) = closed.into_inner().unwrap_or_else(PoisonError::into_inner) {
        return Err(e.into());
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
        .map(|job| -> Result<CommandResult> {
            let result = run_one(config, job)?;
            config.on_done.tell(&result)?;
            Ok(result)
        })
        .collect()
}

/// One command, in turn: its output live unless it goes into JSON.
fn run_one(config: &LoopConfig, job: &DirCommand) -> Result<CommandResult> {
    match (&job.refused, config.json_output) {
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
        (None, true) => Ok(execute_command_in_directory_capturing(job, config)),
        (None, false) => execute_command_in_directory(job, config),
    }
}

/// Execute a list of commands (each with its own directory) and say how
/// they went: in text, unless `json_output` (then the caller reports the
/// [`Outcome`]). A failed command is part of the outcome, not an error.
/// This is the execution engine for meta and its plugins.
pub fn run_commands(config: &LoopConfig, commands: &[DirCommand]) -> Result<Outcome> {
    let text = !config.json_output;
    // on stderr, unless the caller's report lists them
    if text {
        for warning in &config.warnings {
            meta_plugin::out::warn(warning);
        }
    }
    let outcome = |results, tip| Outcome {
        results,
        tip,
        dry_run: config.dry_run,
        silent: config.silent,
    };
    if commands.is_empty() {
        return Ok(outcome(Vec::new(), None));
    }
    let results = if config.parallel {
        let results = run_parallel(config, commands)?;
        if !config.silent && text {
            print_captured(config, &results)?;
        }
        results
    } else {
        run_sequential(config, commands)?
    };

    let total = results.len();
    let failed_count = results.iter().filter(|r| !r.success).count();
    // A failure in every repo may be an option of the caller's typed after
    // the command
    let tip = config
        .outer
        .as_ref()
        .filter(|_| failed_count == total && !config.silent && !config.dry_run)
        .zip(commands.first())
        .filter(|(_, first)| commands.iter().all(|c| c.argv == first.argv))
        .and_then(|(outer, first)| outer.tip(&first.argv, &results));

    if text && !config.silent {
        print_summary(config, &results)?;
    }
    if let Some(tip) = tip.as_ref().filter(|_| text) {
        meta_plugin::out::flush()?;
        errln!(
            "{} {tip}",
            meta_plugin::out::label("tip:", |l| l.cyan().bold())
        );
    }
    Ok(outcome(results, tip))
}

/// The line after the commands: how many ran, or which failed and how. In
/// a dry run only commands that cannot run (a missing directory, a broken
/// alias) fail.
fn print_summary(config: &LoopConfig, results: &[CommandResult]) -> Result<()> {
    let total = results.len();
    let failed_count = results.iter().filter(|r| !r.success).count();
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
        // Without a root the basename is ambiguous, so keep the full path
        let name = |result: &CommandResult| match config.root_dir.as_deref() {
            Some(root) => result_name(&result.directory, Some(root)),
            None => escape_path(&result.directory),
        };
        let failures: Vec<&[CommandResult]> = match same_failure(results) {
            true => vec![results],
            false => results
                .iter()
                .filter(|r| !r.success)
                .map(std::slice::from_ref)
                .collect(),
        };
        for failed in failures {
            let Some(first) = failed.first() else {
                continue;
            };
            let names: Vec<String> = failed.iter().map(name).collect();
            outln!(
                "\n{} {}: {} ({}){}",
                "✗".red(),
                names.join(", "),
                escape(&first.command),
                first.ending(),
                first.why()
            )?;
        }
        outln!()?;
    }
    Ok(())
}

/// The output parallel commands captured, each under its directory's name:
/// what a command wrote to stdout on stdout, to stderr on stderr, as it
/// wrote it. When every command failed the same way, its output is shown
/// once, under all their names.
fn print_captured(config: &LoopConfig, results: &[CommandResult]) -> Result<()> {
    let has_output = |r: &CommandResult| {
        !r.stdout.trim_ascii().is_empty()
            || !r.stderr.trim_ascii().is_empty()
            || r.too_much_output.is_some()
    };
    if results.iter().any(has_output) {
        outln!()?;
    }
    let grouped = same_failure(results);
    let shown = match grouped {
        true => results.get(..1).unwrap_or_default(),
        false => results,
    };
    for result in shown.iter().filter(|r| has_output(r)) {
        let dir_name = match grouped {
            true => results
                .iter()
                .map(|r| result_name(&r.directory, config.root_dir.as_deref()))
                .collect::<Vec<_>>()
                .join(", "),
            false => result_name(&result.directory, config.root_dir.as_deref()),
        };
        if result.success {
            outln!("{} {}:", "✓".green(), dir_name.green())?;
        } else {
            outln!("{} {}:", "✗".red(), dir_name.red())?;
        }
        if !result.stdout.trim_ascii().is_empty() {
            meta_plugin::out::stdout_bytes(&result.stdout)?;
        }
        if !result.stderr.trim_ascii().is_empty() {
            // after the name and the stdout above it, with 2>&1 too
            meta_plugin::out::flush()?;
            meta_plugin::out::stderr_bytes(&result.stderr);
        }
        if let Some(over) = result.too_much_output {
            meta_plugin::out::flush()?;
            errln!("{over}");
        }
        outln!()?; // Blank line after each repo's output
    }
    Ok(())
}

/// The closing error of a silenced run: how many commands failed and where,
/// `2 commands failed: app, platform`.
pub fn failed_line(names: &[String]) -> String {
    match names {
        [] => "At least one command failed".to_string(),
        [one] => format!("1 command failed: {one}"),
        many => format!("{} commands failed: {}", many.len(), many.join(", ")),
    }
}

/// Whether two or more commands all failed the same way: the same command,
/// output and ending, so it is one failure to show once.
fn same_failure(results: &[CommandResult]) -> bool {
    let [first, rest @ ..] = results else {
        return false;
    };
    !rest.is_empty()
        && results.iter().all(|r| {
            !r.success
                && (&r.command, &r.stdout, &r.stderr, r.exit_code, r.signal)
                    == (
                        &first.command,
                        &first.stdout,
                        &first.stderr,
                        first.exit_code,
                        first.signal,
                    )
        })
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
