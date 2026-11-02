//! Subprocess-based plugin system for meta.
//!
//! Plugins are standalone executables that communicate via JSON over stdin/stdout.
//! This approach provides better isolation, language flexibility, and simpler debugging.

use anyhow::{Context, Result};
use meta_core::display::{escape, escape_path};
use meta_core::errln;
use std::io::Read;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use meta_plugin_protocol::{
    ExecutionPlan, PlanResponse, PlannedCommand, PluginInfo, PluginRequest, PluginRequestOptions,
};

/// Why a file gave no plugin info
#[derive(Debug)]
pub(crate) enum NoInfo {
    /// It couldn't start, failed, or answered with something else: a file
    /// on PATH named `meta-*` may be any program, so there it is just no
    /// plugin
    NotAPlugin(anyhow::Error),
    /// It didn't answer in time, or couldn't be waited for: reported
    /// wherever it is, as it costs every command that asks
    NoAnswer(anyhow::Error),
}

impl NoInfo {
    pub(crate) fn into_error(self) -> anyhow::Error {
        match self {
            NoInfo::NotAPlugin(e) | NoInfo::NoAnswer(e) => e,
        }
    }
}

/// Query metadata with one deadline for both process exit and stdout EOF.
pub(crate) fn probe_plugin_info(path: &Path) -> Result<PluginInfo, NoInfo> {
    let not_a_plugin = |reason: String| {
        NoInfo::NotAPlugin(anyhow::anyhow!(
            "Plugin {} did not answer --meta-plugin-info with its info: {}",
            escape_path(path),
            escape(&reason)
        ))
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    #[expect(
        clippy::disallowed_methods,
        reason = "a plugin is meta's own program, not one in a repo"
    )]
    let mut command = Command::new(path);
    command
        .arg("--meta-plugin-info")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| not_a_plugin(format!("it could not start ({e})")))?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(not_a_plugin("its output could not be read".to_string()));
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        // Plugin info is small; a plugin flooding stdout can't fill memory
        let _ = sender.send(stdout.take(1 << 20).read_to_end(&mut bytes).map(|_| bytes));
    });
    // stdout ends once the plugin, and anything it started that holds it, is done
    let remaining = deadline.saturating_duration_since(Instant::now());
    if let Ok(output) = receiver.recv_timeout(remaining) {
        loop {
            match child.try_wait() {
                Ok(Some(status)) if !status.success() => {
                    return Err(not_a_plugin(format!("it ended with {status}")))
                }
                Ok(Some(_)) => {
                    let bytes =
                        output.map_err(|e| not_a_plugin(format!("reading it failed ({e})")))?;
                    return serde_json::from_slice(&bytes)
                        .map_err(|e| not_a_plugin(format!("its answer is not plugin info ({e})")));
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                Ok(None) => break,
                Err(e) => {
                    return Err(NoInfo::NoAnswer(
                        anyhow::Error::new(e)
                            .context(format!("Cannot wait for plugin {}", escape_path(path))),
                    ))
                }
            }
        }
    }
    // A wrapper may already have exited while its descendants hold stdout open.
    // Kill its process group even when the wrapper itself has exited.
    // The child was started as leader of its own process group.
    #[cfg(unix)]
    let _ = rustix::process::kill_process_group(
        rustix::process::Pid::from_child(&child),
        rustix::process::Signal::KILL,
    );
    // Elsewhere only the plugin itself is killed: a process it started that
    // still holds stdout outlives it, but meta doesn't wait for that reader
    let _ = child.kill();
    let _ = child.wait();
    Err(NoInfo::NoAnswer(anyhow::anyhow!(
        "Plugin {} did not answer --meta-plugin-info within 5 seconds",
        escape_path(path)
    )))
}

/// A plugin: the file `meta-<word>` that serves `meta <word>`, and what it
/// answered to `--meta-plugin-info`
#[derive(Debug, Clone)]
pub struct SubprocessPlugin {
    pub word: String,
    pub path: PathBuf,
    pub info: PluginInfo,
}

/// Where plugins are found. A plugin serves the command word in its file
/// name, so finding the one for a command runs nothing else.
///
/// Search order (first file with the name wins):
/// 1. `.meta/plugins/` of each of the workspace's meta directories, from
///    the nearest one up (`meta_core::config::workspace_meta_dirs`)
/// 2. `meta_core::meta_dir()/plugins/` (global installed)
/// 3. PATH (bundled/system plugins)
pub struct SubprocessPluginManager {
    /// Plugin directories in search order, and whether one is a PATH
    /// entry (those may hold anything, so reading one fails quietly)
    dirs: Vec<(PathBuf, bool)>,
    verbose: bool,
}

impl SubprocessPluginManager {
    pub fn new(verbose: bool) -> Result<Self> {
        let mut dirs = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut add = |dir: PathBuf, on_path: bool| {
            if visited.insert(resolved(&dir)) {
                dirs.push((dir, on_path));
            }
        };

        // Project plugins: those of each meta directory from the nearest one
        // up to its workspace's root, nearest first, independently of --config
        let cwd = std::env::current_dir()?;
        // A config meta can't use leaves no project plugins; the command
        // itself reports it, and `--help` still prints
        let nearest = meta_core::config::nearest_meta_dir(&cwd).unwrap_or_else(|e| {
            meta_core::out::warn(format!(
                "{} Skipping project plugins.",
                escape(&format!("{e:#}"))
            ));
            None
        });
        let project_dirs = nearest.map(|nearest| meta_core::config::workspace_meta_dirs(&nearest));
        match project_dirs.transpose() {
            Ok(project_dirs) => {
                for dir in project_dirs.into_iter().flatten() {
                    add(dir.join(".meta/plugins"), false);
                }
            }
            Err(e) => meta_core::out::warn(format!(
                "{} Skipping project plugins.",
                escape(&format!("{e:#}"))
            )),
        }

        // ~/.meta/plugins/ (global installed). No home just means no global
        // plugins.
        if let Ok(meta_dir) = meta_core::meta_dir() {
            add(meta_dir.join("plugins"), false);
        }

        // meta-* executables on PATH (bundled/system)
        if let Some(path_var) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_var) {
                if dir.exists() {
                    add(dir, true);
                }
            }
        }

        Ok(Self { dirs, verbose })
    }

    /// The plugin serving `meta <word>`: the first executable file named
    /// `meta-<word>` in search order. Only that file is started, to ask for
    /// its info; one that doesn't answer is an error naming it.
    pub fn find(&self, word: &str) -> Result<Option<SubprocessPlugin>> {
        let file_name = format!("meta-{word}{}", std::env::consts::EXE_SUFFIX);
        // The same rule as listing; a word that isn't one file name names no plugin
        if plugin_word(&file_name) != Some(word)
            || Path::new(&file_name).file_name() != Some(file_name.as_ref())
        {
            return Ok(None);
        }
        let Some(path) = self
            .dirs
            .iter()
            .map(|(dir, _)| dir.join(&file_name))
            .find(|path| is_executable(path))
        else {
            return Ok(None);
        };
        let info = probe_plugin_info(&path).map_err(NoInfo::into_error)?;
        Ok(Some(self.loaded(word.to_string(), path, info)))
    }

    /// Every plugin, sorted by word. Each one is started to ask for its info;
    /// one that doesn't give it is warned about and left out, except a
    /// program on PATH that isn't a plugin.
    pub fn all(&self) -> Vec<SubprocessPlugin> {
        self.files()
            .into_iter()
            .filter_map(|(word, (path, on_path))| match probe_plugin_info(&path) {
                Ok(info) => Some(self.loaded(word, path, info)),
                Err(NoInfo::NotAPlugin(_)) if on_path => None,
                Err(e) => {
                    meta_core::out::warn(format!("{}; skipping", e.into_error()));
                    None
                }
            })
            .collect()
    }

    /// The file serving each word (the first executable one in search
    /// order, even one that turns out not to answer), and whether it is on
    /// PATH, sorted by word
    fn files(&self) -> std::collections::BTreeMap<String, (PathBuf, bool)> {
        let mut files = std::collections::BTreeMap::new();
        for (dir, on_path) in &self.dirs {
            if self.verbose {
                errln!("Scanning for subprocess plugins in: {}", escape_path(dir));
            }
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) if *on_path || e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    meta_core::out::warn(format!(
                        "Cannot read plugins directory {}: {}; skipping",
                        escape_path(dir),
                        escape(&e.to_string())
                    ));
                    continue;
                }
            };
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) if *on_path => continue,
                    Err(e) => {
                        meta_core::out::warn(format!(
                            "Cannot read plugins directory {}: {}; skipping entry",
                            escape_path(dir),
                            escape(&e.to_string())
                        ));
                        continue;
                    }
                };
                let path = entry.path();
                let Some(word) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(plugin_word)
                else {
                    continue;
                };
                if is_executable(&path) {
                    files.entry(word.to_string()).or_insert((path, *on_path));
                } else if !on_path && path.is_file() {
                    meta_core::out::warn(format!(
                        "Plugin {} is not executable; skipping",
                        escape_path(&path)
                    ));
                }
            }
        }
        files
    }

    fn loaded(&self, word: String, path: PathBuf, info: PluginInfo) -> SubprocessPlugin {
        if self.verbose {
            errln!(
                "  Found plugin: {} v{} ({})",
                escape(&word),
                escape(&info.version),
                escape_path(&path)
            );
        }
        SubprocessPlugin { word, path, info }
    }
}

impl SubprocessPlugin {
    /// How many leading args name the command: the longest command the
    /// plugin claims that is a whole-word prefix of `args`, else the word
    /// alone. Only claimed words are joined into the request's command; what
    /// the user typed stays separate args.
    fn command_len(&self, args: &[String]) -> usize {
        self.info
            .commands
            .iter()
            .map(|cmd| cmd.split_whitespace().collect::<Vec<_>>())
            .filter(|words| {
                !words.is_empty()
                    && words.len() <= args.len()
                    && words.iter().zip(args).all(|(word, arg)| word == arg)
            })
            .map(|words| words.len())
            .max()
            .unwrap_or(1)
    }

    /// Run `args` (the command as typed after `meta`, starting with this
    /// plugin's word) through the plugin, in `projects` (the selected repos
    /// of the workspace or set at `root_dir`), from `cwd`.
    pub fn execute(
        &self,
        args: &[String],
        projects: &[PathBuf],
        root_dir: Option<&Path>,
        cwd: &Path,
        options: PluginRequestOptions,
    ) -> Result<()> {
        // the protocol is JSON: a path that isn't UTF-8 can't be sent
        let projects = projects
            .iter()
            .map(|dir| {
                dir.to_str().map(str::to_string).with_context(|| {
                    format!(
                        "{} is not UTF-8, so plugin {} can't be sent it",
                        escape_path(dir),
                        escape(&self.word)
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let (command, remaining_args) = args
            .split_at_checked(self.command_len(args))
            .unwrap_or((args, &[]));
        let command = command.join(" ");

        let request = PluginRequest {
            command,
            args: remaining_args.to_vec(),
            projects,
            cwd: cwd.to_path_buf(),
            options,
        };

        // the protocol is JSON: a path that isn't UTF-8 can't be sent
        let request_json = serde_json::to_string(&request).with_context(|| {
            format!(
                "Cannot send the request to plugin {} (is a path not UTF-8?)",
                escape_path(&self.path)
            )
        })?;

        if request.options.verbose {
            errln!(
                "Executing plugin {} for command '{}'",
                escape(&self.word),
                escape(&request.command)
            );
        }

        // A plugin may answer without reading its request; then its output
        // and exit status decide, not the pipe. Meta shows what it prints,
        // so it colours as meta does.
        #[expect(
            clippy::disallowed_methods,
            reason = "a plugin is meta's own program, not one in a repo"
        )]
        let mut plugin = Command::new(&self.path);
        plugin
            .arg("--meta-plugin-exec")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped()) // Capture stdout to parse response
            .stderr(Stdio::inherit()); // Let stderr pass through for error messages
        meta_core::out::pass_color(&mut plugin);
        let output = meta_core::children::spawn(&mut plugin)
            .and_then(|child| child.exchange(request_json.as_bytes()))
            .with_context(|| format!("Failed to execute plugin {}", escape_path(&self.path)))?;

        if !output.status.success() {
            // Plugin already printed its error to stderr. What it printed
            // before failing (say the worktrees it removed before a store
            // error) is shown too; then its exit code is propagated.
            let json = request.options.json_output.then_some(self.word.as_str());
            relay(&output.stdout, json, false)?;
            return Err(meta_core::exit::ExitWith::of(output.status).into());
        }

        // Try to parse the response as JSON
        let stdout_str = String::from_utf8_lossy(&output.stdout);

        // If stdout is empty, plugin handled execution silently
        if stdout_str.trim().is_empty() {
            return Ok(());
        }

        // If stdout doesn't look like JSON, print it (legacy plugin output),
        // as the plugin wrote it
        if !stdout_str.trim().starts_with('{') {
            return relay(
                &output.stdout,
                request.options.json_output.then_some(""),
                true,
            );
        }

        // A plan meta cannot read must not pass for success: an older plugin
        // still sends `cmd` strings, and printing them would run nothing
        let is_plan = serde_json::from_str::<serde_json::Value>(&stdout_str)
            .is_ok_and(|value| value.get("plan").is_some());
        match serde_json::from_str::<PlanResponse>(&stdout_str) {
            // Plugin returned an execution plan - execute it via loop_lib
            Ok(response) => execute_plan(
                &response.plan,
                &request.projects,
                root_dir,
                &request.options,
            ),
            Err(e) if is_plan => anyhow::bail!(
                "plugin {} sent an execution plan this meta cannot read ({e}); \
                 upgrade the plugin to match this meta",
                escape_path(&self.path)
            ),
            // Not a plan - print output as-is
            Err(_) => relay(
                &output.stdout,
                request.options.json_output.then_some(""),
                true,
            ),
        }
    }
}

/// Show what a plugin printed, as it wrote it. With `--json` (`json`: the
/// plugin's word), text that is no JSON goes into
/// `{"success": .., "output": ..}`, and a failed plugin that printed
/// nothing (its error is on stderr) leaves `{"success": false, "error": ..}`,
/// so stdout stays JSON.
fn relay(stdout: &[u8], json: Option<&str>, success: bool) -> Result<()> {
    let Some(word) = json else {
        return Ok(meta_core::out::stdout_bytes(stdout)?);
    };
    if stdout.trim_ascii().is_empty() {
        if !success {
            let failed = serde_json::json!({
                "success": false,
                "error": format!("plugin {word} failed"),
            });
            meta_core::outln!("{failed}")?;
        }
        return Ok(());
    }
    let is_json = serde_json::Deserializer::from_slice(stdout)
        .into_iter::<serde::de::IgnoredAny>()
        .all(|value| value.is_ok());
    if is_json {
        return Ok(meta_core::out::stdout_bytes(stdout)?);
    }
    let wrapped = serde_json::json!({
        "success": success,
        "output": String::from_utf8_lossy(stdout),
    });
    Ok(meta_core::outln!("{wrapped}")?)
}

/// Execute an execution plan via loop_lib
fn execute_plan(
    plan: &ExecutionPlan,
    projects: &[String],
    root_dir: Option<&Path>,
    options: &PluginRequestOptions,
) -> Result<()> {
    use loop_lib::{run_commands, DirCommand, LoopConfig};

    // Checked once, before any phase runs, so a plan never stops half done
    // over them
    let max_parallel = plan
        .max_parallel
        .map(|max| NonZeroUsize::new(max).context("The plugin's plan has max_parallel 0"))
        .transpose()?;
    let aliases = loop_lib::aliases()?;
    let job = |c: &PlannedCommand| DirCommand {
        dir: PathBuf::from(&c.dir),
        argv: c.argv.clone(),
        env: c.env.clone(),
        refused: None,
    };
    // Setup and cleanup run one at a time and silently unless verbose;
    // setup runs in a dry run too (it's required)
    let phase_config = LoopConfig {
        verbose: options.verbose,
        silent: true,
        aliases: aliases.clone(),
        ..LoopConfig::default()
    };

    // Phase 1: Run pre_commands sequentially (setup tasks like SSH ControlMaster)
    if !plan.pre_commands.is_empty() {
        if !options.silent {
            use colored::Colorize;
            errln!("{} Preparing connections...", "⟳".cyan());
        }

        for pre_cmd in &plan.pre_commands {
            // Ignore failures for pre_commands (e.g., SSH socket already exists)
            // The main commands will fail if setup was actually needed
            if let Err(e) = run_commands(&phase_config, &[job(pre_cmd)]) {
                if options.verbose {
                    errln!("Pre-command failed (continuing): {e}");
                }
            }
        }
    }

    // Phase 2: Run main commands (may be parallel)
    if !plan.commands.is_empty() {
        let commands: Vec<DirCommand> = plan.commands.iter().map(job).collect();
        // only in the selected projects that are repositories
        let projects: Vec<PathBuf> = projects.iter().map(PathBuf::from).collect();
        let commands = meta_cli::run_list::in_repos(commands, &projects, root_dir);

        // Repos are named by their path from the root (shown as "."). Without
        // projects from meta_cli the plugin lists the root first.
        let root_dir = root_dir
            .map(Path::to_path_buf)
            .or_else(|| commands.first().map(|c| c.dir.clone()));

        let config = LoopConfig {
            verbose: options.verbose,
            silent: options.silent,
            parallel: plan.parallel.unwrap_or(options.parallel),
            dry_run: options.dry_run,
            json_output: options.json_output,
            spawn_stagger_ms: plan.spawn_stagger_ms.unwrap_or(0),
            max_parallel,
            aliases,
            root_dir,
            ..LoopConfig::default()
        };

        run_commands(&config, &commands)?;
    }

    // Phase 3: Run post_commands sequentially (cleanup tasks)
    for post_cmd in &plan.post_commands {
        if let Err(e) = run_commands(&phase_config, &[job(post_cmd)]) {
            if options.verbose {
                errln!("Post-command failed: {e}");
            }
        }
    }

    Ok(())
}

/// The command word a plugin file serves: `meta-<word>` without the
/// platform's executable suffix. Libraries next to plugins serve none.
fn plugin_word(file_name: &str) -> Option<&str> {
    let word = file_name
        .strip_prefix("meta-")?
        .strip_suffix(std::env::consts::EXE_SUFFIX)?;
    let library = [".dylib", ".so", ".dll"]
        .iter()
        .any(|ext| word.ends_with(ext));
    (!word.is_empty() && !library).then_some(word)
}

/// `dir` with its links resolved, so one directory reached by two paths
/// (`/bin` linked to `/usr/bin`) is scanned once.
fn resolved(dir: &Path) -> PathBuf {
    dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf())
}

/// Check if a file is executable
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = path.metadata() {
        let mode = metadata.permissions().mode();
        mode & 0o111 != 0 && metadata.is_file()
    } else {
        false
    }
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin(word: &str, commands: &[&str]) -> SubprocessPlugin {
        SubprocessPlugin {
            word: word.to_string(),
            path: PathBuf::from(format!("/fake/path/meta-{word}")),
            info: PluginInfo {
                name: word.to_string(),
                version: "1.0.0".to_string(),
                commands: commands.iter().map(|c| c.to_string()).collect(),
                description: None,
                help: None,
            },
        }
    }

    fn command_len(plugin: &SubprocessPlugin, args: &[&str]) -> usize {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        plugin.command_len(&args)
    }

    #[test]
    fn the_longest_claimed_command_matching_whole_words_is_the_command() {
        let git = plugin(
            "git",
            &["git status", "git worktree", "git worktree create"],
        );
        let cargo = plugin("cargo", &["cargo", "cargo build"]);

        // The longest claimed command wins; the rest are the plugin's args
        assert_eq!(command_len(&git, &["git", "worktree", "create", "x"]), 3);
        assert_eq!(command_len(&cargo, &["cargo", "build", "--release"]), 2);
        // Otherwise the word alone is the command, the rest staying args;
        // words match whole, so "buildx" is not "build"
        assert_eq!(command_len(&cargo, &["cargo", "--version"]), 1);
        assert_eq!(command_len(&cargo, &["cargo", "buildx", "x"]), 1);
        assert_eq!(command_len(&git, &["git", "log"]), 1);
        // A claim under another word never shapes this plugin's command
        let other = plugin("other", &["git status"]);
        assert_eq!(command_len(&other, &["other", "status"]), 1);
    }

    #[cfg(unix)]
    #[test]
    fn a_word_that_is_a_path_finds_no_plugin() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let bin = temp.path().join("bin");
        std::fs::create_dir_all(bin.join("meta-d")).unwrap();
        let evil = temp.path().join("evil");
        let ran = temp.path().join("ran");
        std::fs::write(&evil, format!("#!/bin/sh\ntouch '{}'\n", ran.display())).unwrap();
        std::fs::set_permissions(&evil, std::fs::Permissions::from_mode(0o755)).unwrap();
        let manager = SubprocessPluginManager {
            dirs: vec![(bin, true)],
            verbose: false,
        };

        let absolute = evil.to_str().unwrap();
        for word in ["d/../../evil", "d/../evil", absolute, "d/", "d/.", ""] {
            assert!(manager.find(word).unwrap().is_none(), "{word}");
        }
        assert!(!ran.exists());
    }

    #[test]
    fn a_plugin_file_serves_the_word_in_its_name() {
        let exe = std::env::consts::EXE_SUFFIX;
        assert_eq!(plugin_word(&format!("meta-git{exe}")), Some("git"));
        assert_eq!(plugin_word(&format!("meta-my-tool{exe}")), Some("my-tool"));
        for name in [
            "meta-",
            "git",
            "meta-lib.so",
            "meta-lib.dylib",
            "meta-lib.dll",
        ] {
            assert_eq!(plugin_word(&format!("{name}{exe}")), None, "{name}");
        }
    }

    #[test]
    fn test_is_executable_nonexistent() {
        let path = std::path::Path::new("/nonexistent/path/to/binary");
        assert!(!is_executable(path));
    }
}
