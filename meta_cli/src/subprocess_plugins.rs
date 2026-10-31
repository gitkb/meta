//! Subprocess-based plugin system for meta.
//!
//! Plugins are standalone executables that communicate via JSON over stdin/stdout.
//! This approach provides better isolation, language flexibility, and simpler debugging.

use anyhow::{Context, Result};
use meta_core::display::{escape, escape_path};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use meta_plugin_protocol::{
    ExecutionPlan, PlanResponse, PluginInfo, PluginRequest, PluginRequestOptions,
};

/// Query metadata with one deadline for both process exit and stdout EOF.
pub(crate) fn probe_plugin_info(path: &Path) -> Result<Option<PluginInfo>> {
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
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return Ok(None),
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(None);
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
                Ok(Some(status)) => {
                    return Ok(output
                        .ok()
                        .filter(|_| status.success())
                        .and_then(|bytes| serde_json::from_slice(&bytes).ok()))
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                Ok(None) => break,
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("Cannot wait for plugin {}; skipping", escape_path(path))
                    })
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
    anyhow::bail!(
        "Plugin {} did not answer --meta-plugin-info within 5 seconds; skipping",
        escape_path(path)
    )
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

/// A discovered subprocess plugin
#[derive(Debug, Clone)]
pub struct SubprocessPlugin {
    pub path: PathBuf,
    pub info: PluginInfo,
}

/// Manager for subprocess-based plugins
pub struct SubprocessPluginManager {
    plugins: HashMap<String, SubprocessPlugin>,
    verbose: bool,
}

impl Default for SubprocessPluginManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SubprocessPluginManager {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            verbose: false,
        }
    }

    /// Discover and load all subprocess plugins
    ///
    /// Discovery order (first match wins):
    /// 1. `.meta/plugins/` of each of the workspace's meta directories, from
    ///    the nearest one up (`meta_core::config::workspace_meta_dirs`)
    /// 2. `meta_core::meta_dir()/plugins/` (global installed)
    /// 3. PATH (bundled/system plugins)
    pub fn discover_plugins(&mut self, verbose: bool) -> Result<()> {
        self.verbose = verbose;
        let mut visited = std::collections::HashSet::new();

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
        if let Some(nearest) = nearest {
            for dir in meta_core::config::workspace_meta_dirs(&nearest) {
                let plugin_dir = dir.join(".meta/plugins");
                if visited.insert(resolved(&plugin_dir)) {
                    self.scan_directory(&plugin_dir)?;
                }
            }
        }

        // Search in ~/.meta/plugins/ (global installed). No home just means
        // no global plugins.
        if let Ok(meta_dir) = meta_core::meta_dir() {
            let global_plugins = meta_dir.join("plugins");
            if visited.insert(resolved(&global_plugins)) {
                self.scan_directory(&global_plugins)?;
            }
        }

        // Search in PATH for meta-* executables (bundled/system)
        if let Ok(path_var) = std::env::var("PATH") {
            for path_dir in std::env::split_paths(&path_var) {
                if path_dir.exists() && visited.insert(resolved(&path_dir)) {
                    self.scan_path_directory(&path_dir)?;
                }
            }
        }

        Ok(())
    }

    /// Scan a `.meta/plugins` directory for plugin executables
    fn scan_directory(&mut self, dir: &Path) -> Result<()> {
        if self.verbose {
            println!("Scanning for subprocess plugins in: {}", escape_path(dir));
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                eprintln!(
                    "Warning: Cannot read plugins directory {}: {}; skipping",
                    escape_path(dir),
                    escape(&e.to_string())
                );
                return Ok(());
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
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

            // Look for executables named meta-*
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("meta-")
                    && !name.ends_with(".dylib")
                    && !name.ends_with(".so")
                    && !name.ends_with(".dll")
                    && is_executable(&path)
                {
                    self.try_load_plugin(&path)?;
                }
            }
        }
        Ok(())
    }

    /// Scan a PATH directory for meta-* executables
    fn scan_path_directory(&mut self, dir: &Path) -> Result<()> {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with("meta-") && is_executable(&path) {
                        self.try_load_plugin(&path)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Try to load a plugin by querying its info
    fn try_load_plugin(&mut self, path: &Path) -> Result<()> {
        if !is_executable(path) {
            return Ok(());
        }

        let info = match probe_plugin_info(path) {
            Ok(Some(info)) => info,
            Ok(None) => return Ok(()),
            Err(e) => {
                eprintln!("Warning: {e}");
                return Ok(());
            }
        };
        if self.verbose {
            println!(
                "  Found plugin: {} v{} ({})",
                escape(&info.name),
                escape(&info.version),
                escape_path(path)
            );
        }
        // Don't override if already loaded (first one wins).
        self.plugins
            .entry(info.name.clone())
            .or_insert_with(|| SubprocessPlugin {
                path: path.to_path_buf(),
                info,
            });
        Ok(())
    }

    /// Find the plugin that owns a command, and how many leading args name it.
    ///
    /// `args` is the command as typed after `meta` (e.g. `["git", "status"]`).
    /// The plugin whose claimed command is the longest whole-word prefix of
    /// `args` gets it. Otherwise a plugin claiming commands under the first
    /// word owns that namespace and gets that word as its command, with the
    /// rest as args, and handles subcommands it does not implement itself
    /// (e.g. meta-git runs raw git). Only claimed words are joined into the
    /// request's command; what the user typed stays separate args.
    fn route(&self, args: &[String]) -> Option<(&SubprocessPlugin, usize)> {
        let first = args.first()?;
        let claims = || {
            self.plugins.values().flat_map(|plugin| {
                plugin
                    .info
                    .commands
                    .iter()
                    .map(move |cmd| (plugin, cmd.split_whitespace().collect::<Vec<_>>()))
            })
        };
        claims()
            .filter(|(_, words)| {
                !words.is_empty()
                    && words.len() <= args.len()
                    && words.iter().zip(args).all(|(word, arg)| word == arg)
            })
            .map(|(plugin, words)| (plugin, words.len()))
            .max_by_key(|(_, len)| *len)
            .or_else(|| {
                claims()
                    .find(|(_, words)| words.first() == Some(&first.as_str()))
                    .map(|(plugin, _)| (plugin, 1))
            })
    }

    /// Execute a command via the plugin that owns it; false if none does.
    pub fn execute(
        &self,
        args: &[String],
        projects: &[String],
        root_dir: Option<&Path>,
        options: PluginRequestOptions,
    ) -> Result<bool> {
        match self.route(args) {
            Some((plugin, len)) => {
                self.execute_plugin(plugin, args, len, projects, root_dir, &options)
            }
            None => Ok(false),
        }
    }

    /// Execute a specific plugin with the first `command_len` args as its command
    fn execute_plugin(
        &self,
        plugin: &SubprocessPlugin,
        args: &[String],
        command_len: usize,
        projects: &[String],
        root_dir: Option<&Path>,
        options: &PluginRequestOptions,
    ) -> Result<bool> {
        let (command, remaining_args) = args.split_at_checked(command_len).unwrap_or((args, &[]));
        let command = command.join(" ");

        let request = PluginRequest {
            command,
            args: remaining_args.to_vec(),
            projects: projects.to_vec(),
            cwd: std::env::current_dir()?,
            options: options.clone(),
        };

        let request_json = serde_json::to_string(&request)?;

        if self.verbose {
            println!(
                "Executing plugin {} for command '{}'",
                escape(&plugin.info.name),
                escape(&request.command)
            );
        }

        let mut child = Command::new(&plugin.path)
            .arg("--meta-plugin-exec")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped()) // Capture stdout to parse response
            .stderr(Stdio::inherit()) // Let stderr pass through for error messages
            .spawn()
            .with_context(|| format!("Failed to execute plugin {}", escape_path(&plugin.path)))?;

        // Send request to plugin's stdin. A plugin may answer without
        // reading it; then its output and exit status decide, not the pipe.
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            match stdin.write_all(request_json.as_bytes()) {
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                result => result.with_context(|| {
                    format!(
                        "Cannot send the request to plugin {}",
                        escape_path(&plugin.path)
                    )
                })?,
            }
        }

        let output = child.wait_with_output()?;

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
            return Ok(true);
        }

        // If stdout doesn't look like JSON, print it (legacy plugin output)
        if !stdout_str.trim().starts_with('{') {
            print_output(&stdout_str)?;
            return Ok(true);
        }

        // A plan meta cannot read must not pass for success: an older plugin
        // still sends `cmd` strings, and printing them would run nothing
        let is_plan = serde_json::from_str::<serde_json::Value>(&stdout_str)
            .is_ok_and(|value| value.get("plan").is_some());
        match serde_json::from_str::<PlanResponse>(&stdout_str) {
            Ok(response) => {
                // Plugin returned an execution plan - execute it via loop_lib
                self.execute_plan(&response.plan, root_dir, options)
            }
            Err(e) if is_plan => anyhow::bail!(
                "plugin {} sent an execution plan this meta cannot read ({e}); \
                 upgrade the plugin to match this meta",
                escape_path(&plugin.path)
            ),
            Err(_) => {
                // Not a plan - print output as-is
                print_output(&stdout_str)?;
                Ok(true)
            }
        }
    }

    /// Execute an execution plan via loop_lib
    fn execute_plan(
        &self,
        plan: &ExecutionPlan,
        root_dir: Option<&Path>,
        options: &PluginRequestOptions,
    ) -> Result<bool> {
        use loop_lib::{run_commands, DirCommand, LoopConfig};

        // Phase 1: Run pre_commands sequentially (setup tasks like SSH ControlMaster)
        if !plan.pre_commands.is_empty() {
            if !options.silent {
                use colored::Colorize;
                eprintln!("{} Preparing connections...", "⟳".cyan());
            }

            let pre_config = LoopConfig {
                directories: vec![],
                ignore: vec![],
                verbose: options.verbose,
                silent: true, // Pre-commands run silently unless verbose
                add_aliases_to_global_looprc: false,
                parallel: false, // Sequential execution
                dry_run: false,  // Pre-commands always run (setup is required)
                json_output: false,
                spawn_stagger_ms: 0,
                env: None,
                max_parallel: None, // Pre-commands are sequential
                root_dir: None,     // Pre-commands don't need "." display
            };

            for pre_cmd in &plan.pre_commands {
                let cmd = DirCommand {
                    dir: pre_cmd.dir.clone(),
                    argv: pre_cmd.argv.clone(),
                    env: pre_cmd.env.clone(),
                };
                // Ignore failures for pre_commands (e.g., SSH socket already exists)
                // The main commands will fail if setup was actually needed
                if let Err(e) = run_commands(&pre_config, &[cmd]) {
                    if options.verbose {
                        eprintln!("Pre-command failed (continuing): {e}");
                    }
                }
            }
        }

        // Phase 2: Run main commands (may be parallel)
        if !plan.commands.is_empty() {
            let commands: Vec<DirCommand> = plan
                .commands
                .iter()
                .map(|c| DirCommand {
                    dir: c.dir.clone(),
                    argv: c.argv.clone(),
                    env: c.env.clone(),
                })
                .collect();

            // Repos are named by their path from the root (shown as "."). Without
            // projects from meta_cli the plugin lists the root first.
            let root_dir = root_dir
                .map(Path::to_path_buf)
                .or_else(|| commands.first().map(|c| PathBuf::from(&c.dir)));

            let config = LoopConfig {
                directories: vec![],
                ignore: vec![],
                verbose: options.verbose,
                silent: options.silent,
                add_aliases_to_global_looprc: false,
                parallel: plan.parallel.unwrap_or(options.parallel),
                dry_run: options.dry_run,
                json_output: options.json_output,
                spawn_stagger_ms: plan.spawn_stagger_ms.unwrap_or(0),
                env: None,
                max_parallel: plan.max_parallel,
                root_dir,
            };

            run_commands(&config, &commands)?;
        }

        // Phase 3: Run post_commands sequentially (cleanup tasks)
        if !plan.post_commands.is_empty() {
            let post_config = LoopConfig {
                directories: vec![],
                ignore: vec![],
                verbose: options.verbose,
                silent: true,
                add_aliases_to_global_looprc: false,
                parallel: false,
                dry_run: false,
                json_output: false,
                spawn_stagger_ms: 0,
                env: None,
                max_parallel: None, // Post-commands are sequential
                root_dir: None,     // Post-commands don't need "." display
            };

            for post_cmd in &plan.post_commands {
                let cmd = DirCommand {
                    dir: post_cmd.dir.clone(),
                    argv: post_cmd.argv.clone(),
                    env: post_cmd.env.clone(),
                };
                if let Err(e) = run_commands(&post_config, &[cmd]) {
                    if options.verbose {
                        eprintln!("Post-command failed: {e}");
                    }
                }
            }
        }

        Ok(true)
    }

    /// Get list of all available commands from all plugins
    pub fn available_commands(&self) -> Vec<(&str, &str)> {
        let mut commands = Vec::new();
        for plugin in self.plugins.values() {
            for cmd in &plugin.info.commands {
                commands.push((cmd.as_str(), plugin.info.name.as_str()));
            }
        }
        commands
    }

    /// Get list of all plugins with their descriptions
    pub fn list_plugins(&self) -> Vec<(&str, &str, &str)> {
        let mut plugins: Vec<_> = self
            .plugins
            .values()
            .map(|p| {
                (
                    p.info.name.as_str(),
                    p.info.version.as_str(),
                    p.info
                        .description
                        .as_deref()
                        .unwrap_or("No description available"),
                )
            })
            .collect();
        plugins.sort_by(|a, b| a.0.cmp(b.0));
        plugins
    }

    /// Get detailed list of plugins including path
    pub fn list_plugins_with_paths(&self) -> Vec<(&str, &str, &str, &std::path::Path)> {
        let mut plugins: Vec<_> = self
            .plugins
            .values()
            .map(|p| {
                (
                    p.info.name.as_str(),
                    p.info.version.as_str(),
                    p.info
                        .description
                        .as_deref()
                        .unwrap_or("No description available"),
                    p.path.as_path(),
                )
            })
            .collect();
        plugins.sort_by(|a, b| a.0.cmp(b.0));
        plugins
    }

    /// Returns all top-level (promoted) commands from plugins.
    ///
    /// A "promoted" command is one that doesn't start with the plugin's name,
    /// making it available directly at the top level (e.g., `meta worktree`
    /// instead of only `meta git worktree`).
    ///
    /// Returns Vec<(command_name, description, plugin_name)>
    pub fn get_promoted_commands(&self) -> Vec<(String, String, String)> {
        let mut promoted = Vec::new();

        for plugin in self.plugins.values() {
            let plugin_name = &plugin.info.name;
            let descriptions = plugin
                .info
                .help
                .as_ref()
                .map(|h| &h.commands)
                .cloned()
                .unwrap_or_default();

            for cmd in &plugin.info.commands {
                let first_word = cmd.split_whitespace().next().unwrap_or("");
                // Promoted if first word is NOT the plugin name AND is a root command (no spaces)
                if first_word != plugin_name && !cmd.contains(' ') {
                    let desc = descriptions
                        .get(cmd)
                        .cloned()
                        .unwrap_or_else(|| format!("Provided by {} plugin", plugin_name));
                    promoted.push((cmd.clone(), desc, plugin_name.clone()));
                }
            }
        }

        promoted.sort_by(|a, b| a.0.cmp(&b.0));
        promoted
    }

    /// Get help text for a specific plugin or command.
    ///
    /// First tries to find a plugin by name (e.g., "git" for `meta git --help`).
    /// If not found, searches for a plugin that handles this command
    /// (e.g., "worktree" is handled by the "git" plugin).
    pub fn get_plugin_help(&self, plugin_or_cmd: &str) -> Option<String> {
        // First, try direct plugin name lookup
        let plugin = self.plugins.get(plugin_or_cmd).or_else(|| {
            // If not found by name, find which plugin handles this command
            self.plugins.values().find(|p| {
                p.info.commands.iter().any(|cmd| {
                    cmd == plugin_or_cmd || cmd.starts_with(&format!("{plugin_or_cmd} "))
                })
            })
        })?;

        // Try to get help by executing plugin with --help
        let output = Command::new(&plugin.path)
            .arg("--help")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                return Some(String::from_utf8_lossy(&output.stdout).to_string());
            }
        }

        // Fall back to generating help from plugin info
        Some(meta_plugin_protocol::help_text(&plugin.info))
    }
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
    fn test_plugin_manager_new() {
        let manager = SubprocessPluginManager::new();
        assert!(manager.plugins.is_empty());
    }

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

    fn routed(manager: &SubprocessPluginManager, args: &[&str]) -> Option<(String, usize)> {
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        manager
            .route(&args)
            .map(|(plugin, len)| (plugin.info.name.clone(), len))
    }

    #[test]
    fn test_route_matches_whole_words() {
        let mut manager = SubprocessPluginManager::new();
        for (name, commands) in [
            ("rust", vec!["cargo build", "cargo test"]),
            (
                "git",
                vec!["git status", "git worktree", "git worktree create"],
            ),
        ] {
            manager.plugins.insert(
                name.to_string(),
                SubprocessPlugin {
                    path: std::path::PathBuf::from(format!("/fake/path/meta-{name}")),
                    info: PluginInfo {
                        name: name.to_string(),
                        version: "1.0.0".to_string(),
                        commands: commands.into_iter().map(String::from).collect(),
                        description: None,
                        help: None,
                    },
                },
            );
        }

        // The longest claimed command wins; the rest are the plugin's args
        assert_eq!(
            routed(&manager, &["git", "worktree", "create", "x"]),
            Some(("git".to_string(), 3))
        );
        assert_eq!(
            routed(&manager, &["cargo", "build", "--release"]),
            Some(("rust".to_string(), 2))
        );
        // Unclaimed subcommands go to the namespace owner as its first word,
        // the rest staying args; words match whole, so "buildx" is not "build"
        assert_eq!(
            routed(&manager, &["cargo", "--version"]),
            Some(("rust".to_string(), 1))
        );
        assert_eq!(
            routed(&manager, &["cargo", "buildx", "x"]),
            Some(("rust".to_string(), 1))
        );
        assert_eq!(routed(&manager, &["gitx", "status"]), None);
        assert_eq!(routed(&manager, &["npm", "install"]), None);
        assert_eq!(routed(&manager, &[]), None);
    }

    #[test]
    fn test_available_commands() {
        let mut manager = SubprocessPluginManager::new();

        let plugin = SubprocessPlugin {
            path: std::path::PathBuf::from("/fake/path/meta-git"),
            info: PluginInfo {
                name: "git".to_string(),
                version: "1.0.0".to_string(),
                commands: vec!["git status".to_string(), "git pull".to_string()],
                description: None,
                help: None,
            },
        };
        manager.plugins.insert("git".to_string(), plugin);

        let commands = manager.available_commands();
        assert_eq!(commands.len(), 2);
        assert!(commands.iter().any(|(cmd, _)| *cmd == "git status"));
        assert!(commands.iter().any(|(cmd, _)| *cmd == "git pull"));
    }

    #[test]
    fn test_is_executable_nonexistent() {
        let path = std::path::Path::new("/nonexistent/path/to/binary");
        assert!(!is_executable(path));
    }

    #[test]
    fn test_list_plugins_sorted() {
        let mut manager = SubprocessPluginManager::new();

        // Add plugins in non-alphabetical order
        manager.plugins.insert(
            "zebra".to_string(),
            SubprocessPlugin {
                path: std::path::PathBuf::from("/fake/meta-zebra"),
                info: PluginInfo {
                    name: "zebra".to_string(),
                    version: "1.0.0".to_string(),
                    commands: vec![],
                    description: Some("Z plugin".to_string()),
                    help: None,
                },
            },
        );
        manager.plugins.insert(
            "alpha".to_string(),
            SubprocessPlugin {
                path: std::path::PathBuf::from("/fake/meta-alpha"),
                info: PluginInfo {
                    name: "alpha".to_string(),
                    version: "2.0.0".to_string(),
                    commands: vec![],
                    description: Some("A plugin".to_string()),
                    help: None,
                },
            },
        );

        let plugins = manager.list_plugins();
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].0, "alpha"); // Should be sorted alphabetically
        assert_eq!(plugins[1].0, "zebra");
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
