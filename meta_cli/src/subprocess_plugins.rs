//! Subprocess-based plugin system for meta.
//!
//! Plugins are standalone executables that communicate via JSON over stdin/stdout.
//! This approach provides better isolation, language flexibility, and simpler debugging.

use anyhow::{Context, Result};
use meta_core::display::{escape, escape_path};
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

/// Print a plugin's output. A closed stdout (`meta ... | head`) ends meta
/// quietly, as it does for command output.
pub(crate) fn print_output(text: &str) -> Result<()> {
    use std::io::Write;
    std::io::stdout()
        .lock()
        .write_all(text.as_bytes())
        .map_err(loop_lib::StdoutError)?;
    Ok(())
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
            eprintln!(
                "Warning: {} Skipping project plugins.",
                escape(&format!("{e:#}"))
            );
            None
        });
        let project_dirs = nearest.map(|nearest| meta_core::config::workspace_meta_dirs(&nearest));
        match project_dirs.transpose() {
            Ok(project_dirs) => {
                for dir in project_dirs.into_iter().flatten() {
                    add(dir.join(".meta/plugins"), false);
                }
            }
            Err(e) => eprintln!(
                "Warning: {} Skipping project plugins.",
                escape(&format!("{e:#}"))
            ),
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
                    eprintln!("Warning: {}; skipping", e.into_error());
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
                println!("Scanning for subprocess plugins in: {}", escape_path(dir));
            }
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) if *on_path || e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    eprintln!(
                        "Warning: Cannot read plugins directory {}: {}; skipping",
                        escape_path(dir),
                        escape(&e.to_string())
                    );
                    continue;
                }
            };
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) if *on_path => continue,
                    Err(e) => {
                        eprintln!(
                            "Warning: Cannot read plugins directory {}: {}; skipping entry",
                            escape_path(dir),
                            escape(&e.to_string())
                        );
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
                    eprintln!(
                        "Warning: Plugin {} is not executable; skipping",
                        escape_path(&path)
                    );
                }
            }
        }
        files
    }

    fn loaded(&self, word: String, path: PathBuf, info: PluginInfo) -> SubprocessPlugin {
        if self.verbose {
            println!(
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
            println!(
                "Executing plugin {} for command '{}'",
                escape(&self.word),
                escape(&request.command)
            );
        }

        // A plugin may answer without reading its request; then its output
        // and exit status decide, not the pipe
        let output = meta_core::children::spawn(
            Command::new(&self.path)
                .arg("--meta-plugin-exec")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped()) // Capture stdout to parse response
                .stderr(Stdio::inherit()), // Let stderr pass through for error messages
        )
        .and_then(|child| child.exchange(request_json.as_bytes()))
        .with_context(|| format!("Failed to execute plugin {}", escape_path(&self.path)))?;

        if !output.status.success() {
            // Plugin already printed its error to stderr. What it printed
            // before failing (say the worktrees it removed before a store
            // error) is shown too; then its exit code is propagated.
            let _ = std::io::Write::write_all(&mut std::io::stdout(), &output.stdout);
            std::process::exit(output.status.code().unwrap_or(1));
        }

        // Try to parse the response as JSON
        let stdout_str = String::from_utf8_lossy(&output.stdout);

        // If stdout is empty, plugin handled execution silently
        if stdout_str.trim().is_empty() {
            return Ok(());
        }

        // If stdout doesn't look like JSON, print it (legacy plugin output)
        if !stdout_str.trim().starts_with('{') {
            return print_output(&stdout_str);
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
            Err(_) => print_output(&stdout_str),
        }
    }
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
            eprintln!("{} Preparing connections...", "⟳".cyan());
        }

        for pre_cmd in &plan.pre_commands {
            // Ignore failures for pre_commands (e.g., SSH socket already exists)
            // The main commands will fail if setup was actually needed
            if let Err(e) = run_commands(&phase_config, &[job(pre_cmd)]) {
                if options.verbose {
                    eprintln!("Pre-command failed (continuing): {e}");
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
                eprintln!("Post-command failed: {e}");
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
    use meta_plugin_protocol::{PlannedCommand, PluginHelp};

    #[test]
    fn test_plugin_info_serialization() {
        let info = PluginInfo {
            name: "test".to_string(),
            version: "1.0.0".to_string(),
            commands: vec!["test cmd".to_string()],
            description: Some("A test plugin".to_string()),
            help: None,
        };

        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"name\":\"test\""));
        assert!(json.contains("\"version\":\"1.0.0\""));

        // Deserialize back
        let parsed: PluginInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "test");
        assert_eq!(parsed.version, "1.0.0");
        assert_eq!(parsed.commands, vec!["test cmd"]);
    }

    #[test]
    fn test_plugin_request_serialization() {
        let request = PluginRequest {
            command: "git status".to_string(),
            args: vec!["--verbose".to_string()],
            projects: vec!["project1".to_string(), "project2".to_string()],
            cwd: "/home/user/workspace".into(),
            options: PluginRequestOptions {
                json_output: true,
                verbose: false,
                parallel: true,
                dry_run: false,
                ..Default::default()
            },
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"command\":\"git status\""));
        assert!(json.contains("\"json_output\":true"));
        assert!(json.contains("\"parallel\":true"));
    }

    #[test]
    fn test_plugin_request_options_default() {
        let options = PluginRequestOptions::default();
        assert!(!options.json_output);
        assert!(!options.verbose);
        assert!(!options.parallel);
        assert!(!options.dry_run);
        assert!(!options.silent);
        assert!(options.include_filters.is_none());
        assert!(options.exclude_filters.is_none());
    }

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

    #[test]
    fn test_plugin_help_serialization() {
        let mut commands = indexmap::IndexMap::new();
        commands.insert("cmd1".to_string(), "Description 1".to_string());

        let help = PluginHelp {
            usage: "test usage".to_string(),
            commands,
            command_sections: indexmap::IndexMap::new(),
            examples: vec!["example 1".to_string()],
            note: Some("a note".to_string()),
        };

        let json = serde_json::to_string(&help).unwrap();
        assert!(json.contains("\"usage\":\"test usage\""));
        assert!(json.contains("\"examples\":[\"example 1\"]"));

        // Deserialize back
        let parsed: PluginHelp = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.usage, "test usage");
        assert_eq!(parsed.examples, vec!["example 1"]);
        assert_eq!(parsed.note, Some("a note".to_string()));
    }

    #[test]
    fn test_plugin_help_deserialization_with_defaults() {
        // Test that missing optional fields use defaults
        let json = r#"{"usage": "test"}"#;
        let help: PluginHelp = serde_json::from_str(json).unwrap();
        assert_eq!(help.usage, "test");
        assert!(help.commands.is_empty());
        assert!(help.examples.is_empty());
        assert!(help.note.is_none());
    }

    // ============ PlanResponse Parsing Tests ============

    #[test]
    fn test_plugin_response_basic() {
        let json = r#"{
            "plan": {
                "commands": [
                    {"dir": "./repo1", "argv": ["git", "status"]},
                    {"dir": "./repo2", "argv": ["git", "status"]}
                ]
            }
        }"#;
        let response: PlanResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.plan.commands.len(), 2);
        assert_eq!(response.plan.commands[0].dir, "./repo1");
        assert_eq!(response.plan.commands[0].argv, ["git", "status"]);
        assert_eq!(response.plan.commands[1].dir, "./repo2");
        assert_eq!(response.plan.commands[1].argv, ["git", "status"]);
        assert!(response.plan.parallel.is_none());
    }

    #[test]
    fn test_plugin_response_with_parallel() {
        let json = r#"{
            "plan": {
                "commands": [
                    {"dir": "/abs/path", "argv": ["npm", "install"]}
                ],
                "parallel": true
            }
        }"#;
        let response: PlanResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.plan.commands.len(), 1);
        assert_eq!(response.plan.parallel, Some(true));
    }

    #[test]
    fn test_plugin_response_parallel_false() {
        let json = r#"{
            "plan": {
                "commands": [
                    {"dir": ".", "argv": ["echo", "hello"]}
                ],
                "parallel": false
            }
        }"#;
        let response: PlanResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.plan.parallel, Some(false));
    }

    #[test]
    fn test_plugin_response_empty_commands() {
        let json = r#"{
            "plan": {
                "commands": []
            }
        }"#;
        let response: PlanResponse = serde_json::from_str(json).unwrap();
        assert!(response.plan.commands.is_empty());
    }

    // ============ ExecutionPlan Tests ============

    #[test]
    fn test_execution_plan_deserialization() {
        let json = r#"{
            "commands": [
                {"dir": "proj1", "argv": ["make", "build"]},
                {"dir": "proj2", "argv": ["cargo", "build"]}
            ],
            "parallel": true
        }"#;
        let plan: ExecutionPlan = serde_json::from_str(json).unwrap();
        assert_eq!(plan.commands.len(), 2);
        assert_eq!(plan.commands[0].dir, "proj1");
        assert_eq!(plan.commands[0].argv, ["make", "build"]);
        assert_eq!(plan.commands[1].dir, "proj2");
        assert_eq!(plan.commands[1].argv, ["cargo", "build"]);
        assert_eq!(plan.parallel, Some(true));
    }

    #[test]
    fn test_execution_plan_no_parallel_field() {
        let json = r#"{
            "commands": [
                {"dir": ".", "argv": ["ls"]}
            ]
        }"#;
        let plan: ExecutionPlan = serde_json::from_str(json).unwrap();
        assert!(plan.parallel.is_none());
    }

    // ============ PlannedCommand Tests ============

    #[test]
    fn test_planned_command_deserialization() {
        let json = r#"{"dir": "/home/user/project", "argv": ["git", "pull", "--rebase"]}"#;
        let cmd: PlannedCommand = serde_json::from_str(json).unwrap();
        assert_eq!(cmd.dir, "/home/user/project");
        assert_eq!(cmd.argv, ["git", "pull", "--rebase"]);
    }

    #[test]
    fn test_planned_command_relative_dir() {
        let json = r#"{"dir": "./relative/path", "argv": ["npm", "test"]}"#;
        let cmd: PlannedCommand = serde_json::from_str(json).unwrap();
        assert_eq!(cmd.dir, "./relative/path");
        assert_eq!(cmd.argv, ["npm", "test"]);
    }

    #[test]
    fn test_planned_command_complex_command() {
        let json = r#"{"dir": ".", "argv": ["git", "clone", "https://github.com/org/repo.git", "--depth", "1"]}"#;
        let cmd: PlannedCommand = serde_json::from_str(json).unwrap();
        assert_eq!(
            cmd.argv,
            [
                "git",
                "clone",
                "https://github.com/org/repo.git",
                "--depth",
                "1"
            ]
        );
    }

    // ============ PluginRequest with dry_run Tests ============

    #[test]
    fn test_plugin_request_with_dry_run() {
        let request = PluginRequest {
            command: "git status".to_string(),
            args: vec![],
            projects: vec!["proj1".to_string()],
            cwd: "/workspace".into(),
            options: PluginRequestOptions {
                json_output: false,
                verbose: false,
                parallel: false,
                dry_run: true,
                ..Default::default()
            },
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"dry_run\":true"));
    }

    #[test]
    fn test_plugin_request_all_options_enabled() {
        let request = PluginRequest {
            command: "build".to_string(),
            args: vec!["--release".to_string()],
            projects: vec![],
            cwd: ".".into(),
            options: PluginRequestOptions {
                json_output: true,
                verbose: true,
                parallel: true,
                dry_run: true,
                silent: true,
                recursive: true,
                depth: Some(3),
                include_filters: Some(vec!["frontend".to_string()]),
                exclude_filters: Some(vec!["tests".to_string()]),
                tag_filters: Some(vec!["rust".to_string()]),
                strict: false,
            },
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"json_output\":true"));
        assert!(json.contains("\"verbose\":true"));
        assert!(json.contains("\"parallel\":true"));
        assert!(json.contains("\"dry_run\":true"));
        assert!(json.contains("\"silent\":true"));
        assert!(json.contains("\"include_filters\""));
        assert!(json.contains("\"exclude_filters\""));
        assert!(json.contains(r#""tag_filters":["rust"]"#));
    }

    // ============ Edge Cases and Error Handling ============

    #[test]
    fn test_plugin_response_invalid_json() {
        // Invalid JSON should fail to parse
        let json = r#"{"plan": "not an object"}"#;
        let result: Result<PlanResponse, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_plugin_response_missing_plan() {
        // Missing plan field should fail
        let json = r#"{"commands": []}"#;
        let result: Result<PlanResponse, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_execution_plan_multiple_different_commands() {
        // Test a realistic execution plan with different commands per directory
        let json = r#"{
            "commands": [
                {"dir": "./frontend", "argv": ["npm", "install"]},
                {"dir": "./backend", "argv": ["cargo", "build"]},
                {"dir": "./scripts", "argv": ["python", "setup.py", "install"]},
                {"dir": "./docs", "argv": ["make", "html"]}
            ],
            "parallel": false
        }"#;
        let plan: ExecutionPlan = serde_json::from_str(json).unwrap();
        assert_eq!(plan.commands.len(), 4);
        assert_eq!(plan.commands[0].argv, ["npm", "install"]);
        assert_eq!(plan.commands[1].argv, ["cargo", "build"]);
        assert_eq!(plan.commands[2].argv, ["python", "setup.py", "install"]);
        assert_eq!(plan.commands[3].argv, ["make", "html"]);
    }

    #[test]
    fn test_execution_plan_with_pre_commands() {
        // Test ExecutionPlan with pre_commands (e.g., SSH setup)
        let json = r#"{
            "pre_commands": [
                {"dir": ".", "argv": ["ssh", "-fNM", "-o", "ControlMaster=auto", "git@github.com"]}
            ],
            "commands": [
                {"dir": "./repo1", "argv": ["git", "push"]},
                {"dir": "./repo2", "argv": ["git", "push"]}
            ],
            "parallel": true
        }"#;
        let plan: ExecutionPlan = serde_json::from_str(json).unwrap();
        assert_eq!(plan.pre_commands.len(), 1);
        assert!(plan.pre_commands[0].argv[0] == "ssh");
        assert_eq!(plan.commands.len(), 2);
        assert_eq!(plan.parallel, Some(true));
    }

    #[test]
    fn test_execution_plan_with_post_commands() {
        // Test ExecutionPlan with post_commands (e.g., cleanup)
        let json = r#"{
            "commands": [
                {"dir": ".", "argv": ["make", "build"]}
            ],
            "post_commands": [
                {"dir": ".", "argv": ["make", "clean"]}
            ],
            "parallel": false
        }"#;
        let plan: ExecutionPlan = serde_json::from_str(json).unwrap();
        assert!(plan.pre_commands.is_empty());
        assert_eq!(plan.commands.len(), 1);
        assert_eq!(plan.post_commands.len(), 1);
        assert!(plan.post_commands[0].argv.contains(&"clean".to_string()));
    }

    #[test]
    fn test_execution_plan_backward_compatible() {
        // Test that old JSON without pre/post_commands still works
        let json = r#"{
            "commands": [{"dir": ".", "argv": ["ls"]}]
        }"#;
        let plan: ExecutionPlan = serde_json::from_str(json).unwrap();
        assert!(plan.pre_commands.is_empty());
        assert!(plan.post_commands.is_empty());
        assert_eq!(plan.commands.len(), 1);
    }

    #[test]
    fn test_planned_command_with_special_characters() {
        let json = r#"{"dir": "./path with spaces", "argv": ["echo", "\"hello world\" && echo 'single quotes'"]}"#;
        let cmd: PlannedCommand = serde_json::from_str(json).unwrap();
        assert_eq!(cmd.dir, "./path with spaces");
        assert_eq!(
            cmd.argv,
            ["echo", "\"hello world\" && echo 'single quotes'"]
        );
    }
}
