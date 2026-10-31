//! SSH multiplexing for parallel git operations, and the remote checks that
//! go with it.
//!
//! Parallel git commands that reach one host over SSH can be rate-limited.
//! ControlMaster connections, set up here with explicit `-o` flags (no
//! dependency on `~/.ssh/config`), let them share one connection through the
//! `GIT_SSH_COMMAND` this module returns.

use console::style;
use log::{debug, warn};
use meta_core::config::{ProjectInfo, Workspace};
use meta_core::display::escape;
use meta_core::display::shell_quote;
use meta_core::git::GitUrl;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Result of SSH multiplexing setup.
pub enum SshMasters {
    /// We established masters; callers should inject `GIT_SSH_COMMAND`.
    OurSockets(PathBuf),
    /// All hosts already have active masters from user's own SSH config.
    /// Parallel execution is safe; no `GIT_SSH_COMMAND` override needed.
    UserManaged,
    /// Setup failed for all hosts. Callers should fall back to serial.
    Failed,
}

/// Where an SSH remote connects.
struct SshTarget {
    /// `None` when the URL omits a user — SSH resolves from config or current OS user.
    user: Option<String>,
    host: String,
    port: u16,
}

impl SshTarget {
    /// The target of `url`, if git reaches it over SSH.
    fn of(url: &str) -> Option<Self> {
        let url: GitUrl = url.parse().ok()?;
        url.is_ssh().then(|| SshTarget {
            user: url.user,
            host: url.host,
            port: url.port.unwrap_or(22),
        })
    }

    /// SSH destination string: `user@host` when user is explicit, otherwise just `host`.
    fn destination(&self) -> String {
        match &self.user {
            Some(u) => format!("{u}@{}", self.host),
            None => self.host.clone(),
        }
    }

    /// Key for socket naming. When the URL omits a user, SSH resolves `%r`
    /// to the current OS user, so we must match that to find our own sockets.
    fn socket_name(&self) -> String {
        format!("{}@{}-{}", self.resolved_user(), self.host, self.port)
    }

    /// The effective SSH user: explicit if provided, otherwise the OS user
    /// (matching what SSH resolves `%r` to).
    fn resolved_user(&self) -> String {
        match &self.user {
            Some(u) => u.clone(),
            None => std::env::var("USER")
                .or_else(|_| std::env::var("LOGNAME"))
                .unwrap_or_else(|_| "_".to_string()),
        }
    }

    /// `ssh` with `args`, for this target, quiet.
    fn ssh(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.args(args);
        if self.port != 22 {
            cmd.args(["-p", &self.port.to_string()]);
        }
        cmd.arg(self.destination())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd
    }
}

/// The distinct SSH remotes of `projects`.
pub(crate) fn ssh_urls(projects: &[ProjectInfo]) -> Vec<String> {
    let urls: BTreeSet<&String> = projects
        .iter()
        .filter_map(|p| p.repo.as_ref())
        .filter(|repo| SshTarget::of(repo).is_some())
        .collect();
    urls.into_iter().cloned().collect()
}

/// Establish SSH ControlMaster connections for the SSH remotes among `urls`.
///
/// Returns:
/// - `SshMasters::OurSockets(dir)` if we established at least one master
/// - `SshMasters::UserManaged` if all hosts already have active masters
/// - `SshMasters::Failed` if sockets dir couldn't be created or all connections failed
pub fn establish_ssh_masters(urls: &[&str]) -> SshMasters {
    let mut seen = BTreeSet::new();
    let targets: Vec<SshTarget> = urls
        .iter()
        .filter_map(|url| SshTarget::of(url))
        .filter(|t| seen.insert(t.socket_name()))
        .collect();

    // No SSH remotes — nothing to do, parallel execution is safe
    if targets.is_empty() {
        return SshMasters::UserManaged;
    }

    let Some(home) = dirs::home_dir() else {
        warn!("Could not determine HOME directory for SSH sockets");
        return SshMasters::Failed;
    };
    let sockets_dir = match ensure_ssh_sockets_dir(&home) {
        Ok(dir) => dir,
        Err(e) => {
            warn!("Failed to create SSH sockets directory: {e}");
            return SshMasters::Failed;
        }
    };

    let mut any_needed_our_master = false;
    let mut any_succeeded = false;
    let ran = |mut cmd: Command| cmd.status().is_ok_and(|s| s.success());

    for target in &targets {
        // If user already has a working master from their own config, skip
        if ran(target.ssh(&["-O", "check"])) {
            debug!(
                "Host {} (port {}) already has an active ControlMaster",
                target.destination(),
                target.port
            );
            continue;
        }
        any_needed_our_master = true;

        // A socket from a previous run, if its master is still live
        let socket_path = sockets_dir.join(target.socket_name());
        if socket_path.exists() {
            let probe = target.ssh(&["-O", "check", "-S", &socket_path.to_string_lossy()]);
            if ran(probe) {
                debug!(
                    "Reusing live socket for {} from previous run",
                    target.destination()
                );
                any_succeeded = true;
                continue;
            }
            // Socket exists but master isn't active — stale. Remove and recreate.
            let _ = std::fs::remove_file(&socket_path);
            debug!("Removed stale socket for {}", target.destination());
        }

        // No shell here, so no quoting (GIT_SSH_COMMAND, which a shell reads,
        // is quoted in git_ssh_command)
        let control_path = format!("ControlPath={}/%r@%h-%p", sockets_dir.display());
        let master = target.ssh(&[
            "-fNM",
            "-o",
            "ControlMaster=auto",
            "-o",
            &control_path,
            "-o",
            "ControlPersist=600",
            "-o",
            "ConnectTimeout=10",
        ]);
        if ran(master) {
            debug!(
                "Established SSH ControlMaster for {} (port {})",
                target.destination(),
                target.port
            );
            any_succeeded = true;
        } else {
            warn!(
                "SSH master for {} (port {}) could not be started",
                target.destination(),
                target.port
            );
        }
    }

    // If no host needed our master (all had existing connections), return UserManaged
    if !any_needed_our_master {
        debug!("All hosts have existing ControlMaster connections, no override needed");
        return SshMasters::UserManaged;
    }

    if any_succeeded {
        SshMasters::OurSockets(sockets_dir)
    } else {
        SshMasters::Failed
    }
}

/// A `GIT_SSH_COMMAND` value that reuses established ControlMaster
/// connections. Git runs it through a shell, so the `ControlPath` option is
/// quoted; SSH expands `%r`, `%h` and `%p` itself.
pub fn git_ssh_command(sockets_dir: &Path) -> String {
    let control_path = format!("ControlPath={}/%r@%h-%p", sockets_dir.display());
    format!(
        "ssh -o ControlMaster=auto -o {} -o ControlPersist=600 -o ConnectTimeout=10",
        shell_quote(&control_path)
    )
}

/// `.ssh/sockets` in `home`, created if missing, with `.ssh` and it kept at
/// mode `0o700` on Unix (a directory made with a loose umask is tightened).
fn ensure_ssh_sockets_dir(home: &Path) -> std::io::Result<PathBuf> {
    let sockets_dir = home.join(".ssh").join("sockets");
    std::fs::create_dir_all(&sockets_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for dir in [Some(sockets_dir.as_path()), sockets_dir.parent()]
            .into_iter()
            .flatten()
        {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(sockets_dir)
}

/// Warn about checkouts whose `origin` differs from the URL `workspace`
/// lists for them (non-interactive).
pub(crate) fn warn_remote_mismatches(workspace: &Workspace) {
    let normalized = |url: &str| {
        url.parse::<GitUrl>()
            .map(|u| u.normalized())
            .unwrap_or_else(|_| url.to_string())
    };
    let mismatches: Vec<(&ProjectInfo, &String, String)> = workspace
        .projects
        .iter()
        .filter_map(|project| {
            let expected = project.repo.as_ref()?;
            let actual = meta_core::git::remote_url(&workspace.dir.join(&project.path))?;
            (normalized(&actual) != normalized(expected)).then_some((project, expected, actual))
        })
        .collect();
    if mismatches.is_empty() {
        return;
    }

    eprintln!(
        "{} Found {} remote URL mismatch{}:",
        style("⚠").yellow().bold(),
        mismatches.len(),
        if mismatches.len() == 1 { "" } else { "es" }
    );
    for (project, expected, actual) in &mismatches {
        eprintln!("  {}", style(escape(&project.name)).bold());
        eprintln!("    actual:   {}", style(escape(actual)).red());
        eprintln!("    expected: {}", style(escape(expected)).green());
    }
    eprintln!();
    eprintln!("  Fix manually with:");
    for (project, expected, _) in &mismatches {
        eprintln!(
            "    git -C {} remote set-url origin {}",
            escape(&shell_quote(&project.path)),
            escape(&shell_quote(expected))
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_ssh_command_quotes_the_control_path_for_the_shell() {
        let cmd = git_ssh_command(Path::new("/Users/John Doe's/.ssh/sockets"));
        let words = shlex::split(&cmd).unwrap();
        assert!(words.contains(&"ControlPath=/Users/John Doe's/.ssh/sockets/%r@%h-%p".to_string()));
        assert!(words.contains(&"ControlPersist=600".to_string()));
    }

    #[test]
    fn targets_come_from_ssh_remotes_only() {
        let t = SshTarget::of("ssh://alice@example.com:2222/repo.git").unwrap();
        assert_eq!(
            (t.destination().as_str(), t.port),
            ("alice@example.com", 2222)
        );
        assert_eq!(t.socket_name(), "alice@example.com-2222");
        let t = SshTarget::of("example.com:org/repo.git").unwrap();
        assert_eq!((t.destination().as_str(), t.port), ("example.com", 22));
        assert!(SshTarget::of("https://github.com/org/repo.git").is_none());
        assert!(SshTarget::of("/srv/repo.git").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn sockets_dir_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let sockets = tmp.path().join(".ssh/sockets");
        std::fs::create_dir_all(&sockets).unwrap();
        std::fs::set_permissions(&sockets, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(ensure_ssh_sockets_dir(tmp.path()).unwrap(), sockets);
        for dir in [&sockets, &tmp.path().join(".ssh")] {
            let mode = std::fs::metadata(dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{}", dir.display());
        }
    }
}
