//! SSH helpers for parallel git operations.
//!
//! When running multiple git commands in parallel (e.g., `meta git update`),
//! SSH connections to the same host can be rate-limited. SSH multiplexing
//! allows multiple sessions to share a single TCP connection, avoiding this issue.

use std::fs;
use std::io;
use std::path::PathBuf;

/// Patterns that indicate SSH rate-limiting or connection issues
const SSH_ERROR_PATTERNS: &[&str] = &[
    "Connection closed by",
    "Operation timed out",
    "ssh_dispatch_run_fatal",
    "Connection reset by peer",
    "Connection refused",
];

/// Check if an error message indicates SSH rate-limiting
pub fn is_ssh_rate_limit_error(error_output: &str) -> bool {
    SSH_ERROR_PATTERNS
        .iter()
        .any(|pattern| error_output.contains(pattern))
}

/// Validate that a hostname contains only valid characters.
///
/// Valid hostnames contain:
/// - Alphanumeric characters (a-z, A-Z, 0-9)
/// - Hyphens (but not at start/end of labels)
/// - Dots (as label separators)
/// - Underscores (technically invalid per RFC but common in internal hostnames)
///
/// Also accepts:
/// - IPv4 addresses (e.g., 192.168.1.1)
/// - IPv6 addresses in brackets (e.g., [::1], [2001:db8::1])
fn is_valid_hostname(host: &str) -> bool {
    if host.is_empty() {
        return false;
    }

    // Handle bracketed IPv6 addresses
    if host.starts_with('[') && host.ends_with(']') {
        let inner = &host[1..host.len() - 1];
        // Basic IPv6 validation: hex digits, colons, and optional dots for mapped IPv4
        return !inner.is_empty()
            && inner
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
    }

    // Reject hosts that are just dots or start/end with dots
    if host == "." || host == ".." || host.starts_with('.') || host.ends_with('.') {
        return false;
    }

    // Reject hosts with consecutive dots
    if host.contains("..") {
        return false;
    }

    // All characters must be alphanumeric, hyphen, underscore, or dot
    host.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Extract the SSH hostname from a git remote URL.
///
/// Supports:
/// - SCP-like syntax: `git@HOST:path`
/// - SSH URL: `ssh://HOST/path` or `ssh://user@HOST/path`
///
/// Returns `None` for:
/// - Non-SSH URLs (https://, file://, etc.)
/// - Malformed URLs with invalid hostnames
/// - URLs with embedded credentials (user:password@host)
pub fn extract_ssh_host(url: &str) -> Option<String> {
    let url = url.trim();

    if let Some(rest) = url.strip_prefix("ssh://") {
        // ssh://[user[:password]@]host[:port]/path
        let host_part = rest.split('/').next()?;

        // Check for embedded password (user:password@host) - reject these
        if let Some(at_pos) = host_part.rfind('@') {
            let user_part = &host_part[..at_pos];
            if user_part.contains(':') {
                // Embedded password detected - reject for security
                return None;
            }
        }

        // Strip optional port (but be careful with IPv6 brackets)
        let host_no_port = if host_part.contains('[') {
            // IPv6 address: [::1]:port or [::1]
            let bracket_end = host_part.find(']')?;
            &host_part[..=bracket_end]
        } else {
            // Regular host:port
            host_part.split(':').next()?
        };

        let host = host_no_port.split('@').next_back()?;
        if !is_valid_hostname(host) {
            return None;
        }
        Some(host.to_string())
    } else if url.contains('@') && url.contains(':') && !url.contains("://") {
        // git@host:path (SCP-like syntax)
        // Must have exactly one @ for valid SCP syntax
        let parts: Vec<&str> = url.splitn(2, '@').collect();
        if parts.len() != 2 {
            return None;
        }

        let after_at = parts[1];

        // Check for embedded password in user part (user:password@host:path)
        if parts[0].contains(':') {
            return None;
        }

        let host = after_at.split(':').next()?;
        if !is_valid_hostname(host) {
            return None;
        }
        Some(host.to_string())
    } else {
        None
    }
}

/// Normalize a git remote URL for comparison purposes.
///
/// Strips trailing `.git` suffix, trailing slashes, and converts
/// `ssh://git@host/path` to SCP-like `git@host:path` form so that
/// equivalent URLs compare equal.
pub fn normalize_git_url(url: &str) -> String {
    let mut s = url.trim().to_string();

    // Strip trailing .git
    if s.ends_with(".git") {
        s.truncate(s.len() - 4);
    }
    // Strip trailing slashes
    while s.ends_with('/') {
        s.pop();
    }

    // Normalize ssh:// URLs to SCP-like form for consistent comparison
    if let Some(rest) = s.strip_prefix("ssh://") {
        // ssh://[user@]host[:port]/path -> user@host:path (drop port)
        if let Some(slash_pos) = rest.find('/') {
            let host_part = &rest[..slash_pos];
            let path = &rest[slash_pos + 1..];
            // Strip optional port
            let host_no_port = if let Some(colon_pos) = host_part.rfind(':') {
                // Only strip if after @ (it's a port, not user separator)
                if host_part[colon_pos + 1..]
                    .chars()
                    .all(|c| c.is_ascii_digit())
                {
                    &host_part[..colon_pos]
                } else {
                    host_part
                }
            } else {
                host_part
            };
            // Ensure user@ prefix (default to git@)
            let with_user = if host_no_port.contains('@') {
                host_no_port.to_string()
            } else {
                format!("git@{host_no_port}")
            };
            return format!("{with_user}:{path}");
        }
    }

    s
}

/// Get the origin remote URL of a git repository.
///
/// Returns `None` if the directory doesn't exist, isn't a git repo,
/// or has no `origin` remote.
pub fn get_remote_url(repo_path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_path)
        .output()
        .ok()?;

    if output.status.success() {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if url.is_empty() {
            None
        } else {
            Some(url)
        }
    } else {
        None
    }
}

/// Check whether two git remote URLs point to the same repository.
pub fn urls_match(a: &str, b: &str) -> bool {
    normalize_git_url(a) == normalize_git_url(b)
}

/// Get the path to the SSH sockets directory
pub fn ssh_sockets_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".ssh").join("sockets"))
}

/// Ensure the SSH sockets directory exists with correct permissions.
///
/// Creates `~/.ssh/sockets` if missing.
///
/// On Unix, also enforces mode `0o700` on `~/.ssh` and `~/.ssh/sockets`.
/// Returns the path to the sockets directory, or `None` if the home directory
/// cannot be determined.
pub fn ensure_ssh_sockets_dir() -> io::Result<Option<PathBuf>> {
    let Some(sockets_dir) = ssh_sockets_dir() else {
        return Ok(None);
    };
    fs::create_dir_all(&sockets_dir)?;
    // Always enforce permissions — a pre-existing directory may have
    // overly permissive modes from a previous `mkdir` or bad umask.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&sockets_dir, fs::Permissions::from_mode(0o700))?;
        if let Some(ssh_dir) = sockets_dir.parent() {
            fs::set_permissions(ssh_dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(Some(sockets_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_ssh_rate_limit_error() {
        // Test all known SSH error patterns
        assert!(is_ssh_rate_limit_error(
            "Connection closed by 140.82.113.4 port 22"
        ));
        assert!(is_ssh_rate_limit_error(
            "ssh: connect to host github.com port 22: Operation timed out"
        ));
        assert!(is_ssh_rate_limit_error(
            "ssh_dispatch_run_fatal: Connection to 140.82.114.3 port 22"
        ));
        assert!(is_ssh_rate_limit_error("Connection reset by peer"));
        assert!(is_ssh_rate_limit_error("Connection refused"));

        // Test non-matching cases
        assert!(!is_ssh_rate_limit_error("Already up to date."));
        assert!(!is_ssh_rate_limit_error("fatal: not a git repository"));
        assert!(!is_ssh_rate_limit_error(
            "error: pathspec 'foo' did not match any file(s)"
        ));
        assert!(!is_ssh_rate_limit_error(""));
    }

    #[test]
    fn test_extract_ssh_host_scp_syntax() {
        assert_eq!(
            extract_ssh_host("git@github.com:org/repo.git"),
            Some("github.com".to_string())
        );
        assert_eq!(
            extract_ssh_host("git@gitlab.example.com:group/project.git"),
            Some("gitlab.example.com".to_string())
        );
        assert_eq!(
            extract_ssh_host("deploy@bitbucket.org:team/repo.git"),
            Some("bitbucket.org".to_string())
        );
    }

    #[test]
    fn test_extract_ssh_host_ssh_url() {
        assert_eq!(
            extract_ssh_host("ssh://git@github.com/org/repo.git"),
            Some("github.com".to_string())
        );
        assert_eq!(
            extract_ssh_host("ssh://gitlab.example.com/group/project.git"),
            Some("gitlab.example.com".to_string())
        );
        assert_eq!(
            extract_ssh_host("ssh://user@gitea.local:2222/org/repo.git"),
            Some("gitea.local".to_string())
        );
    }

    #[test]
    fn test_extract_ssh_host_non_ssh() {
        assert_eq!(extract_ssh_host("https://github.com/org/repo.git"), None);
        assert_eq!(extract_ssh_host("http://github.com/org/repo.git"), None);
        assert_eq!(extract_ssh_host("file:///path/to/repo"), None);
        assert_eq!(extract_ssh_host("/local/path/to/repo"), None);
        assert_eq!(extract_ssh_host(""), None);
    }

    #[test]
    fn test_ssh_sockets_dir() {
        let path = ssh_sockets_dir();
        if std::env::var("HOME").is_ok() {
            assert!(path.is_some());
            let path = path.unwrap();
            assert!(path.ends_with("sockets"));
            assert!(path.to_str().unwrap().contains(".ssh"));
        }
    }

    #[test]
    fn test_ensure_ssh_sockets_dir() {
        // Just verify the function doesn't panic — actual dir creation
        // depends on HOME being set and filesystem permissions
        let result = ensure_ssh_sockets_dir();
        assert!(result.is_ok());
    }

    /// RAII guard that restores HOME on drop, ensuring cleanup even on panic.
    #[cfg(unix)]
    struct HomeGuard(Option<String>);

    #[cfg(unix)]
    impl HomeGuard {
        fn new() -> Self {
            Self(std::env::var("HOME").ok())
        }
    }

    #[cfg(unix)]
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn test_ensure_ssh_sockets_dir_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let _guard = HomeGuard::new();

        std::env::set_var("HOME", tmp.path());
        let result = ensure_ssh_sockets_dir();
        assert!(result.is_ok());

        let sockets_dir = result.unwrap().expect("should return Some path");
        assert!(sockets_dir.exists());
        assert_eq!(
            fs::metadata(&sockets_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let ssh_dir = sockets_dir.parent().unwrap();
        assert_eq!(
            fs::metadata(ssh_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn test_ensure_ssh_sockets_dir_fixes_permissive_existing_dir() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let _guard = HomeGuard::new();

        // Pre-create with overly permissive mode
        let ssh_dir = tmp.path().join(".ssh");
        let sockets_dir = ssh_dir.join("sockets");
        fs::create_dir_all(&sockets_dir).unwrap();
        fs::set_permissions(&sockets_dir, fs::Permissions::from_mode(0o755)).unwrap();

        std::env::set_var("HOME", tmp.path());
        let result = ensure_ssh_sockets_dir();
        assert!(result.is_ok());

        // Permissions should be tightened
        assert_eq!(
            fs::metadata(&sockets_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn test_is_ssh_rate_limit_error_case_sensitivity() {
        // Should match exact case
        assert!(is_ssh_rate_limit_error("Connection closed by"));
        // Should not match different case (patterns are case-sensitive)
        assert!(!is_ssh_rate_limit_error("connection closed by"));
    }

    // ============ URL Normalization Tests ============

    #[test]
    fn test_normalize_strips_trailing_dot_git() {
        assert_eq!(
            normalize_git_url("git@github.com:org/repo.git"),
            "git@github.com:org/repo"
        );
    }

    #[test]
    fn test_normalize_strips_trailing_slash() {
        assert_eq!(
            normalize_git_url("git@github.com:org/repo/"),
            "git@github.com:org/repo"
        );
    }

    #[test]
    fn test_normalize_ssh_url_to_scp() {
        assert_eq!(
            normalize_git_url("ssh://git@github.com/org/repo.git"),
            "git@github.com:org/repo"
        );
    }

    #[test]
    fn test_normalize_ssh_url_with_port() {
        assert_eq!(
            normalize_git_url("ssh://git@github.com:22/org/repo.git"),
            "git@github.com:org/repo"
        );
    }

    #[test]
    fn test_normalize_ssh_url_no_user() {
        // ssh://host/path should normalize to git@host:path
        assert_eq!(
            normalize_git_url("ssh://github.com/org/repo.git"),
            "git@github.com:org/repo"
        );
    }

    #[test]
    fn test_normalize_https_unchanged() {
        // HTTPS URLs don't get converted — they stay as-is (minus .git)
        assert_eq!(
            normalize_git_url("https://github.com/org/repo.git"),
            "https://github.com/org/repo"
        );
    }

    #[test]
    fn test_normalize_scp_without_dot_git() {
        assert_eq!(
            normalize_git_url("git@github.com:org/repo"),
            "git@github.com:org/repo"
        );
    }

    #[test]
    fn test_normalize_trims_whitespace() {
        assert_eq!(
            normalize_git_url("  git@github.com:org/repo.git  "),
            "git@github.com:org/repo"
        );
    }

    // ============ urls_match Tests ============

    #[test]
    fn test_urls_match_identical() {
        assert!(urls_match(
            "git@github.com:org/repo.git",
            "git@github.com:org/repo.git"
        ));
    }

    #[test]
    fn test_urls_match_with_without_dot_git() {
        assert!(urls_match(
            "git@github.com:org/repo.git",
            "git@github.com:org/repo"
        ));
    }

    #[test]
    fn test_urls_match_scp_vs_ssh_url() {
        assert!(urls_match(
            "git@github.com:org/repo.git",
            "ssh://git@github.com/org/repo.git"
        ));
    }

    #[test]
    fn test_urls_match_ssh_url_with_port() {
        assert!(urls_match(
            "git@github.com:org/repo",
            "ssh://git@github.com:22/org/repo.git"
        ));
    }

    #[test]
    fn test_urls_no_match_different_repos() {
        assert!(!urls_match(
            "git@github.com:org/repo-a.git",
            "git@github.com:org/repo-b.git"
        ));
    }

    #[test]
    fn test_urls_no_match_ssh_vs_https() {
        // SSH and HTTPS are genuinely different remotes
        assert!(!urls_match(
            "git@github.com:org/repo.git",
            "https://github.com/org/repo.git"
        ));
    }

    #[test]
    fn test_urls_match_https_with_without_dot_git() {
        assert!(urls_match(
            "https://github.com/org/repo.git",
            "https://github.com/org/repo"
        ));
    }

    // ============ Hostname Validation Tests ============

    #[test]
    fn test_is_valid_hostname_standard() {
        assert!(is_valid_hostname("github.com"));
        assert!(is_valid_hostname("gitlab.example.com"));
        assert!(is_valid_hostname("bitbucket.org"));
        assert!(is_valid_hostname("localhost"));
    }

    #[test]
    fn test_is_valid_hostname_with_hyphens() {
        assert!(is_valid_hostname("my-server.example.com"));
        assert!(is_valid_hostname("git-hub.com"));
    }

    #[test]
    fn test_is_valid_hostname_with_underscores() {
        // Underscores are technically invalid per RFC but common internally
        assert!(is_valid_hostname("my_server.local"));
        assert!(is_valid_hostname("git_host"));
    }

    #[test]
    fn test_is_valid_hostname_ipv4() {
        assert!(is_valid_hostname("192.168.1.1"));
        assert!(is_valid_hostname("10.0.0.1"));
        assert!(is_valid_hostname("127.0.0.1"));
    }

    #[test]
    fn test_is_valid_hostname_ipv6_bracketed() {
        assert!(is_valid_hostname("[::1]"));
        assert!(is_valid_hostname("[2001:db8::1]"));
        assert!(is_valid_hostname("[fe80::1]"));
        // IPv4-mapped IPv6
        assert!(is_valid_hostname("[::ffff:192.168.1.1]"));
    }

    #[test]
    fn test_is_valid_hostname_invalid() {
        assert!(!is_valid_hostname(""));
        assert!(!is_valid_hostname("."));
        assert!(!is_valid_hostname(".."));
        assert!(!is_valid_hostname(".example.com"));
        assert!(!is_valid_hostname("example.com."));
        assert!(!is_valid_hostname("example..com"));
        // Invalid characters
        assert!(!is_valid_hostname("host name"));
        assert!(!is_valid_hostname("host/path"));
        assert!(!is_valid_hostname("host:port"));
    }

    // ============ Edge Case URL Parsing Tests ============

    #[test]
    fn test_extract_ssh_host_rejects_embedded_password_scp() {
        // SCP-like syntax with password should be rejected
        assert_eq!(
            extract_ssh_host("user:password@github.com:org/repo.git"),
            None
        );
    }

    #[test]
    fn test_extract_ssh_host_rejects_embedded_password_ssh_url() {
        // SSH URL with embedded password should be rejected
        assert_eq!(
            extract_ssh_host("ssh://user:password@github.com/org/repo.git"),
            None
        );
    }

    #[test]
    fn test_extract_ssh_host_handles_whitespace() {
        assert_eq!(
            extract_ssh_host("  git@github.com:org/repo.git  "),
            Some("github.com".to_string())
        );
        assert_eq!(
            extract_ssh_host("\tssh://git@github.com/org/repo.git\n"),
            Some("github.com".to_string())
        );
    }

    #[test]
    fn test_extract_ssh_host_rejects_empty_host() {
        assert_eq!(extract_ssh_host("git@:path"), None);
        assert_eq!(extract_ssh_host("ssh:///path"), None);
        assert_eq!(extract_ssh_host("ssh://user@/path"), None);
    }

    #[test]
    fn test_extract_ssh_host_rejects_invalid_hostname() {
        assert_eq!(extract_ssh_host("git@..:path"), None);
        assert_eq!(extract_ssh_host("git@host/with/slash:path"), None);
        assert_eq!(extract_ssh_host("ssh://host with space/path"), None);
    }

    #[test]
    fn test_extract_ssh_host_ipv6_ssh_url() {
        assert_eq!(
            extract_ssh_host("ssh://git@[::1]/repo.git"),
            Some("[::1]".to_string())
        );
        assert_eq!(
            extract_ssh_host("ssh://[2001:db8::1]/repo.git"),
            Some("[2001:db8::1]".to_string())
        );
    }

    #[test]
    fn test_extract_ssh_host_ipv6_with_port() {
        assert_eq!(
            extract_ssh_host("ssh://git@[::1]:2222/repo.git"),
            Some("[::1]".to_string())
        );
    }

    #[test]
    fn test_extract_ssh_host_internal_hostnames() {
        // Internal hostnames are valid
        assert_eq!(
            extract_ssh_host("git@gitlab.internal:org/repo.git"),
            Some("gitlab.internal".to_string())
        );
        assert_eq!(
            extract_ssh_host("git@git-server:repo.git"),
            Some("git-server".to_string())
        );
    }

    #[test]
    fn test_extract_ssh_host_localhost() {
        assert_eq!(
            extract_ssh_host("git@localhost:repo.git"),
            Some("localhost".to_string())
        );
        assert_eq!(
            extract_ssh_host("ssh://localhost/repo.git"),
            Some("localhost".to_string())
        );
    }

    #[test]
    fn test_extract_ssh_host_numeric_ip() {
        assert_eq!(
            extract_ssh_host("git@192.168.1.100:repo.git"),
            Some("192.168.1.100".to_string())
        );
        assert_eq!(
            extract_ssh_host("ssh://10.0.0.1/repo.git"),
            Some("10.0.0.1".to_string())
        );
    }
}
