//! Subprocess-based plugin system for meta.
//!
//! Plugins are standalone executables: meta finds them, asks each for its
//! info, and calls one with a request in a file (`meta_plugin`'s protocol).

use anyhow::{Context, Result};
use meta_plugin::display::{escape, escape_path};
use meta_plugin::errln;
use meta_plugin::{ErrorCode, ErrorInfo, PluginInfo, Request, Response};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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

/// Ask the plugin at `path` for its info.
pub(crate) fn probe_plugin_info(path: &Path) -> Result<PluginInfo, NoInfo> {
    probe(path, &["--meta-plugin-info".to_string()], "its info")
}

/// The JSON answer (`what`) of the plugin at `path` started with `args`
/// (a flag first, then its words), with one deadline for both process
/// exit and stdout EOF, and at most 1 MiB of it read.
fn probe<T: serde::de::DeserializeOwned>(
    path: &Path,
    args: &[String],
    what: &str,
) -> Result<T, NoInfo> {
    let flag = args.first().map(String::as_str).unwrap_or_default();
    let not_a_plugin = |reason: String| {
        NoInfo::NotAPlugin(anyhow::anyhow!(
            "Plugin {} did not answer {flag} with {what}: {}",
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
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // its own process group (job on Windows): what a wrapper starts is
    // stopped with it
    let mut child = meta_plugin::process::spawn_group(&mut command)
        .map_err(|e| not_a_plugin(format!("it could not start ({e})")))?;
    let Some(stdout) = child.stdout() else {
        child.stop(true);
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
                        .map_err(|e| not_a_plugin(format!("its answer is not {what} ({e})")));
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
    // A wrapper may already have exited while its descendants hold stdout
    // open: its whole group is killed even then
    child.stop(true);
    let _ = child.wait();
    Err(NoInfo::NoAnswer(anyhow::anyhow!(
        "Plugin {} did not answer {flag} within 5 seconds",
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
            meta_plugin::out::warn(format!(
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
            Err(e) => meta_plugin::out::warn(format!(
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

    /// The word of every plugin file, read from the file names: none is
    /// started.
    pub fn words(&self) -> Vec<String> {
        self.files().into_keys().collect()
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
                    meta_plugin::out::warn(format!("{}; skipping", e.into_error()));
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
                    meta_plugin::out::warn(format!(
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
                        meta_plugin::out::warn(format!(
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
                    meta_plugin::out::warn(format!(
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
    pub fn command_len(&self, args: &[String]) -> usize {
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

    /// What `args` (the line, from the plugin's word on) act on: the
    /// plugin's answer, when its info says it answers; otherwise the
    /// projects meta's options select. A failed or malformed answer is an
    /// error naming the plugin; a set name that isn't one, the name's.
    pub fn targeting(&self, args: &[String]) -> Result<meta_plugin::Targeting> {
        if !self.info.answers_targeting {
            return Ok(meta_plugin::Targeting::Selection);
        }
        let flagged: Vec<String> = std::iter::once("--meta-plugin-targeting".to_string())
            .chain(args.iter().cloned())
            .collect();
        let answer: meta_plugin::Targeting =
            probe(&self.path, &flagged, "what the line acts on").map_err(NoInfo::into_error)?;
        // as typed, mostly: said as the name's own error
        if let Some(name) = answer.set() {
            meta_plugin::sets::validate_name(name)?;
        }
        Ok(answer)
    }

    /// Call the plugin with `request`: the request in a file, its answer
    /// read from another (see `meta_plugin`'s protocol). Its stdin and
    /// stderr are meta's, and its stdout too, but under `--json` meta's
    /// stderr, so meta's stdout holds the Envelope alone. Under `--json`
    /// its stderr passes through meta, which keeps its end: a plugin that
    /// fails without an answer has it in the Envelope's error.
    pub fn call(
        &self,
        request: &Request,
        show: crate::events::Show,
    ) -> Result<(Response, crate::events::Received)> {
        let shown = escape_path(&self.path);
        let dir = tempfile::Builder::new()
            .prefix("meta-call-")
            .tempdir()
            .context("Cannot make a folder for the plugin call")?;
        let (request_file, response_file) = (
            dir.path().join("request.json"),
            dir.path().join("response.json"),
        );
        std::fs::write(&request_file, serde_json::to_vec(request)?)
            .context("Cannot write the plugin request")?;
        if request.meta.verbose {
            errln!(
                "Executing plugin {} for command '{}'",
                escape(&self.word),
                escape(&request.command)
            );
        }
        #[expect(
            clippy::disallowed_methods,
            reason = "a plugin is meta's own program, not one in a repo"
        )]
        let mut plugin = Command::new(&self.path);
        plugin
            .arg("--meta-plugin-exec")
            .env(meta_plugin::REQUEST_VAR, &request_file)
            .env(meta_plugin::RESPONSE_VAR, &response_file)
            .stdin(Stdio::inherit());
        match request.meta.json {
            true => plugin.stdout(std::io::stderr()).stderr(Stdio::piped()),
            false => plugin.stdout(Stdio::inherit()).stderr(Stdio::inherit()),
        };
        let events = crate::events::Channel::open(dir.path(), show)
            .context("Cannot make the plugin's events channel")?;
        plugin.env(meta_plugin::event::EVENTS_VAR, events.path());
        meta_plugin::out::pass_color(&mut plugin);
        meta_plugin::out::flush()?;
        let status = meta_plugin::process::spawn(&mut plugin).and_then(|mut child| {
            let stderr = child.stderr().map(StderrTail::pass);
            child.wait().map(|status| (status, stderr))
        });
        let received = events.finish();
        let (status, stderr) =
            status.with_context(|| format!("Failed to execute plugin {shown}"))?;
        let said = stderr.map(StderrTail::finish).unwrap_or_default();
        let ended = meta_plugin::exit::ExitWith::of(status);
        let failed = |why: String| {
            let message = match said.is_empty() {
                true => format!("plugin {shown} {why}"),
                false => format!("plugin {shown} {why} (its stderr ends:\n{said})"),
            };
            let info = ErrorInfo::new(ErrorCode::PluginFailed, message);
            match ended.0 {
                // its own ending: meta ends with it, as the plugin may
                // have said why on stderr
                0 => anyhow::Error::new(info),
                _ => anyhow::Error::new(meta_plugin::exit::ExitWith(ended.0)).context(info),
            }
        };
        let bytes = match std::fs::read(&response_file) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(failed(format!("ended ({status}) without an answer")))
            }
            Err(e) => return Err(failed(format!("wrote an answer meta can't read ({e})"))),
        };
        if let Some(problem) = &received.problem {
            return Err(failed(problem.clone()));
        }
        let response = serde_json::from_slice(&bytes)
            .map_err(|e| failed(format!("wrote an answer that is no response ({e})")))?;
        Ok((response, received))
    }
}

/// A plugin's stderr, passed on to meta's as it comes, its last lines kept.
struct StderrTail {
    tail: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    ended: std::sync::mpsc::Receiver<()>,
}

impl StderrTail {
    /// The most of a plugin's stderr kept for its error.
    const KEPT: usize = 4096;

    fn pass(mut pipe: std::process::ChildStderr) -> StderrTail {
        let tail = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (done, ended) = std::sync::mpsc::channel();
        let kept = std::sync::Arc::clone(&tail);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 8192];
            loop {
                let n = match pipe.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                let bytes = chunk.get(..n).unwrap_or_default();
                meta_plugin::out::stderr_bytes(bytes);
                let mut tail = kept.lock().unwrap_or_else(|p| p.into_inner());
                tail.extend_from_slice(bytes);
                if let Some(last) = tail
                    .len()
                    .checked_sub(Self::KEPT)
                    .and_then(|at| tail.get(at..))
                {
                    *tail = last.to_vec();
                }
            }
            let _ = done.send(());
        });
        StderrTail { tail, ended }
    }

    /// What it kept, once its stderr ended: whole lines, escaped. What
    /// it started may hold its stderr open after it ended: that is waited
    /// for only briefly, and passed on as it comes.
    fn finish(self) -> String {
        let _ = self.ended.recv_timeout(Duration::from_millis(200));
        let tail = std::mem::take(&mut *self.tail.lock().unwrap_or_else(|p| p.into_inner()));
        let text = String::from_utf8_lossy(&tail);
        // a line cut where the tail starts isn't shown
        let whole = match tail.len() >= Self::KEPT {
            true => text.split_once('\n').map_or("", |(_, rest)| rest),
            false => &text,
        };
        meta_plugin::display::escape_lines(whole.trim()).into_owned()
    }
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

/// Whether `program` names an executable file in a PATH directory.
pub fn on_path(program: &str) -> bool {
    let file = format!("{program}{}", std::env::consts::EXE_SUFFIX);
    // a word with a separator is a path, not a program PATH finds
    Path::new(&file).file_name() == Some(file.as_ref())
        && std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).any(|dir| is_executable(&dir.join(&file)))
        })
}

/// The extensions of `tool` on PATH: for each executable file named
/// `<tool>-<name>` in a PATH directory, the `<name>` that `<tool> <name>`
/// runs it as. Sorted, each once.
pub fn extensions_on_path(tool: &str) -> std::collections::BTreeSet<String> {
    let prefix = format!("{tool}-");
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let file = entry.file_name().into_string().ok()?;
            let name = file
                .strip_suffix(std::env::consts::EXE_SUFFIX)?
                .strip_prefix(&prefix)?;
            (!name.is_empty() && is_executable(&entry.path())).then(|| name.to_string())
        })
        .collect()
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
                answers_targeting: false,
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
