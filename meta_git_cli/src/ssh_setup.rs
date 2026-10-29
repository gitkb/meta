//! Self-contained SSH multiplexing for parallel git operations.
//!
//! Establishes SSH ControlMaster connections using explicit `-o` flags,
//! with no dependency on `~/.ssh/config`. Returns a `GIT_SSH_COMMAND`
//! value that git subprocesses use to share the established connections.

use log::{debug, warn};
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

/// Parsed SSH target from a remote URL.
struct SshTarget {
    /// `None` when the URL omits a user — SSH resolves from config or current OS user.
    user: Option<String>,
    host: String,
    port: u16,
}

impl SshTarget {
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
        let user_part = self.resolved_user();
        format!("{user_part}@{}-{}", self.host, self.port)
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

    /// Key for deduplication.
    fn dedup_key(&self) -> String {
        let user_part = self.resolved_user();
        format!("{user_part}@{}:{}", self.host, self.port)
    }
}

/// Parse an SSH remote URL into user, host, port components.
///
/// Handles both `git@github.com:org/repo.git` (SCP) and
/// `ssh://user@host:port/path` (URL) formats.
fn parse_ssh_target(url: &str) -> Option<SshTarget> {
    if let Some(rest) = url.strip_prefix("ssh://") {
        // ssh://user@host:port/path or ssh://user@host/path
        let (user_host, _path) = rest.split_once('/').unwrap_or((rest, ""));
        let (user, host_port) = if let Some((u, hp)) = user_host.split_once('@') {
            (Some(u.to_string()), hp)
        } else {
            (None, user_host)
        };
        let (host, port) = if let Some((h, p)) = host_port.split_once(':') {
            let port = p.parse::<u16>().unwrap_or_else(|_| {
                warn!("Invalid port '{p}' in SSH URL, defaulting to 22");
                22
            });
            (h.to_string(), port)
        } else {
            (host_port.to_string(), 22)
        };
        Some(SshTarget { user, host, port })
    } else if url.starts_with("https://") || url.starts_with("http://") {
        None
    } else if let Some((user_host, _path)) = url.split_once(':') {
        // git@github.com:org/repo.git (SCP-style)
        let (user, host) = if let Some((u, h)) = user_host.split_once('@') {
            (Some(u.to_string()), h.to_string())
        } else {
            (None, user_host.to_string())
        };
        Some(SshTarget {
            user,
            host,
            port: 22,
        })
    } else {
        None
    }
}

/// Check if an existing ControlMaster connection is active for a target.
///
/// Uses `ssh -O check` which respects the user's own SSH config.
fn has_existing_master(target: &SshTarget) -> bool {
    let mut cmd = Command::new("ssh");
    cmd.args(["-O", "check"]);
    if target.port != 22 {
        cmd.args(["-p", &target.port.to_string()]);
    }
    cmd.arg(target.destination());
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

/// Check if a ControlMaster socket exists in the given directory for a target.
fn socket_exists_in(sockets_dir: &Path, target: &SshTarget) -> bool {
    sockets_dir.join(target.socket_name()).exists()
}

/// Establish SSH ControlMaster connections for the given remote URLs.
///
/// Parses each URL to extract user, host, and port. Uses explicit `-o` flags
/// so this works without `~/.ssh/config` having multiplexing configured.
///
/// Returns:
/// - `SshMasters::OurSockets(dir)` if we established at least one master
/// - `SshMasters::UserManaged` if all hosts already have active masters
/// - `SshMasters::Failed` if sockets dir couldn't be created or all connections failed
pub fn establish_ssh_masters(urls: &[&str]) -> SshMasters {
    // Parse and deduplicate targets
    let mut targets: Vec<SshTarget> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for url in urls {
        if let Some(target) = parse_ssh_target(url) {
            let key = target.dedup_key();
            if seen.insert(key) {
                targets.push(target);
            }
        }
    }

    // No SSH remotes — nothing to do, parallel execution is safe
    if targets.is_empty() {
        return SshMasters::UserManaged;
    }

    let sockets_dir = match meta_git_lib::ensure_ssh_sockets_dir() {
        Ok(Some(dir)) => dir,
        Ok(None) => {
            warn!("Could not determine HOME directory for SSH sockets");
            return SshMasters::Failed;
        }
        Err(e) => {
            warn!("Failed to create SSH sockets directory: {e}");
            return SshMasters::Failed;
        }
    };

    let mut any_needed_our_master = false;
    let mut any_succeeded = false;

    for target in &targets {
        // If user already has a working master from their own config, skip
        if has_existing_master(target) {
            debug!(
                "Host {} (port {}) already has an active ControlMaster",
                target.destination(),
                target.port
            );
            continue;
        }

        // Check if we already have a socket from a previous run
        if socket_exists_in(&sockets_dir, target) {
            let socket_path = sockets_dir.join(target.socket_name());
            // Probe the socket with ssh -O check -S <path> to see if it's still live
            let alive = Command::new("ssh")
                .args(["-O", "check", "-S"])
                .arg(&socket_path)
                .arg(target.destination())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if alive {
                debug!(
                    "Reusing live socket for {} from previous run",
                    target.destination()
                );
                any_succeeded = true;
                any_needed_our_master = true;
                continue;
            }
            // Socket exists but master isn't active — stale. Remove and recreate.
            let _ = std::fs::remove_file(&socket_path);
            debug!("Removed stale socket for {}", target.destination());
        }

        any_needed_our_master = true;

        // No shell quoting needed here — Command::new bypasses the shell,
        // so spaces in the path are handled correctly as a single argument.
        // (git_ssh_command() uses single quotes because GIT_SSH_COMMAND is
        // evaluated by the shell.)
        let control_path = format!("{}/{}", sockets_dir.display(), "%r@%h-%p");
        let mut cmd = Command::new("ssh");
        cmd.args([
            "-fNM",
            "-o",
            "ControlMaster=auto",
            "-o",
            &format!("ControlPath={control_path}"),
            "-o",
            "ControlPersist=600",
            "-o",
            "ConnectTimeout=10",
        ]);
        if target.port != 22 {
            cmd.args(["-p", &target.port.to_string()]);
        }
        cmd.arg(target.destination());
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        match cmd.status() {
            Ok(s) if s.success() => {
                debug!(
                    "Established SSH ControlMaster for {} (port {})",
                    target.destination(),
                    target.port
                );
                any_succeeded = true;
            }
            Ok(s) => warn!(
                "SSH master for {} (port {}) exited with status {s}",
                target.destination(),
                target.port
            ),
            Err(e) => warn!(
                "Failed to spawn SSH master for {} (port {}): {e}",
                target.destination(),
                target.port
            ),
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

/// Build a `GIT_SSH_COMMAND` value that reuses established ControlMaster connections.
///
/// Set this on git subprocesses so they share the pre-established connections
/// without needing `~/.ssh/config` to be configured.
///
/// The `ControlPath` value is single-quoted to protect against spaces in the
/// sockets directory path. SSH expands `%r`, `%h`, `%p` inside its own
/// processing, not via shell expansion.
pub fn git_ssh_command(sockets_dir: &Path) -> String {
    format!(
        "ssh -o ControlMaster=auto -o 'ControlPath={sockets}/%r@%h-%p' -o ControlPersist=600 -o ConnectTimeout=10",
        sockets = sockets_dir.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_ssh_command_format() {
        let cmd = git_ssh_command(Path::new("/home/user/.ssh/sockets"));
        assert!(cmd.contains("ControlMaster=auto"));
        assert!(cmd.contains("ControlPath=/home/user/.ssh/sockets/%r@%h-%p"));
        assert!(cmd.contains("ControlPersist=600"));
        // Must NOT contain %% — Rust format! doesn't use % for formatting
        assert!(!cmd.contains("%%"));
    }

    #[test]
    fn test_git_ssh_command_quotes_path() {
        let cmd = git_ssh_command(Path::new("/Users/John Doe/.ssh/sockets"));
        assert!(cmd.contains("'ControlPath=/Users/John Doe/.ssh/sockets/%r@%h-%p'"));
    }

    #[test]
    fn test_socket_exists_in_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let target = SshTarget {
            user: Some("git".into()),
            host: "github.com".into(),
            port: 22,
        };
        assert!(!socket_exists_in(dir.path(), &target));
    }

    #[test]
    fn test_socket_exists_in_when_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("git@github.com-22"), "").unwrap();
        let target = SshTarget {
            user: Some("git".into()),
            host: "github.com".into(),
            port: 22,
        };
        assert!(socket_exists_in(dir.path(), &target));
    }

    #[test]
    fn test_parse_ssh_target_scp_style() {
        let t = parse_ssh_target("git@github.com:org/repo.git").unwrap();
        assert_eq!(t.user.as_deref(), Some("git"));
        assert_eq!(t.host, "github.com");
        assert_eq!(t.port, 22);
    }

    #[test]
    fn test_parse_ssh_target_ssh_url() {
        let t = parse_ssh_target("ssh://alice@example.com:2222/repo.git").unwrap();
        assert_eq!(t.user.as_deref(), Some("alice"));
        assert_eq!(t.host, "example.com");
        assert_eq!(t.port, 2222);
    }

    #[test]
    fn test_parse_ssh_target_ssh_url_default_port() {
        let t = parse_ssh_target("ssh://git@github.com/org/repo.git").unwrap();
        assert_eq!(t.user.as_deref(), Some("git"));
        assert_eq!(t.host, "github.com");
        assert_eq!(t.port, 22);
    }

    #[test]
    fn test_parse_ssh_target_https_returns_none() {
        assert!(parse_ssh_target("https://github.com/org/repo.git").is_none());
    }

    #[test]
    fn test_has_existing_master_nonexistent_host() {
        let target = SshTarget {
            user: Some("git".into()),
            host: "nonexistent-host-12345.example.com".into(),
            port: 22,
        };
        assert!(!has_existing_master(&target));
    }

    #[test]
    fn test_socket_exists_custom_port() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alice@example.com-2222"), "").unwrap();
        let target = SshTarget {
            user: Some("alice".into()),
            host: "example.com".into(),
            port: 2222,
        };
        assert!(socket_exists_in(dir.path(), &target));
    }

    #[test]
    fn test_parse_ssh_target_no_user_ssh_url() {
        let t = parse_ssh_target("ssh://example.com/repo.git").unwrap();
        assert_eq!(t.user, None);
        assert_eq!(t.host, "example.com");
        assert_eq!(t.destination(), "example.com");
    }

    #[test]
    fn test_parse_ssh_target_no_user_scp_style() {
        let t = parse_ssh_target("example.com:org/repo.git").unwrap();
        assert_eq!(t.user, None);
        assert_eq!(t.host, "example.com");
        assert_eq!(t.destination(), "example.com");
    }

    #[test]
    fn test_socket_name_no_user() {
        let target = SshTarget {
            user: None,
            host: "example.com".into(),
            port: 22,
        };
        // Should resolve to the OS user, matching SSH's %r expansion
        let os_user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "_".to_string());
        assert_eq!(target.socket_name(), format!("{os_user}@example.com-22"));
    }

    #[test]
    fn test_destination_with_user() {
        let target = SshTarget {
            user: Some("alice".into()),
            host: "example.com".into(),
            port: 22,
        };
        assert_eq!(target.destination(), "alice@example.com");
    }
}
