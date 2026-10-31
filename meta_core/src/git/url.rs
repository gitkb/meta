//! Git remote URLs, read the way git reads them.

use std::str::FromStr;

/// A remote URL: `scheme://[user@]host[:port]/path`, scp-like
/// `[user@]host:path` (ssh), or a local path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitUrl {
    /// `ssh` for the scp-like form, `""` for a local path.
    pub scheme: String,
    pub user: Option<String>,
    /// Empty for a local path.
    pub host: String,
    pub port: Option<u16>,
    /// The repository path, without a leading `/` after a host.
    pub path: String,
}

impl FromStr for GitUrl {
    type Err = anyhow::Error;

    fn from_str(url: &str) -> anyhow::Result<Self> {
        let url = url.trim();
        let local = || GitUrl {
            scheme: String::new(),
            user: None,
            host: String::new(),
            port: None,
            path: url.to_string(),
        };
        if let Some((scheme, rest)) = url.split_once("://") {
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            let (user, host_port) = match authority.rsplit_once('@') {
                Some((user, host_port)) => (Some(user), host_port),
                None => (None, authority),
            };
            // `[v6]:port`, `host:port` or a bare host
            let (host, port) = match host_port.rsplit_once(':') {
                Some((host, port)) if !port.contains(']') => (host, Some(port)),
                _ => (host_port, None),
            };
            let port = port
                .filter(|p| !p.is_empty())
                .map(|p| {
                    p.parse().map_err(|_| {
                        anyhow::anyhow!("invalid port in {}", crate::display::escape(url))
                    })
                })
                .transpose()?;
            return Ok(GitUrl {
                scheme: scheme.to_ascii_lowercase(),
                user: user.map(str::to_string),
                host: host.to_string(),
                port,
                path: path.to_string(),
            });
        }
        // As git: scp-like when a `:` comes before any `/`, and what is
        // before it isn't a Windows drive letter
        match url.split_once(':') {
            Some((authority, path))
                if !authority.contains('/') && !(authority.len() == 1 && cfg!(windows)) =>
            {
                let (user, host) = match authority.split_once('@') {
                    Some((user, host)) => (Some(user.to_string()), host),
                    None => (None, authority),
                };
                Ok(GitUrl {
                    scheme: "ssh".to_string(),
                    user,
                    host: host.to_string(),
                    port: None,
                    path: path.to_string(),
                })
            }
            _ => Ok(local()),
        }
    }
}

impl GitUrl {
    /// Whether git reaches this remote through ssh, at a plain host name or
    /// address, as a user that ssh can't read as an option or a password.
    pub fn is_ssh(&self) -> bool {
        matches!(self.scheme.as_str(), "ssh" | "git+ssh" | "ssh+git")
            && valid_host(&self.host)
            && self.user.as_deref().is_none_or(|u| {
                !u.is_empty() && !u.starts_with('-') && !u.contains([':', ' ', '\t'])
            })
    }

    /// A form two URLs of the same repository share: without `.git`, a
    /// trailing `/` or an ssh port, and with ssh's default user `git`.
    pub fn normalized(&self) -> String {
        let path = self.path.trim_end_matches('/');
        let path = path
            .strip_suffix(".git")
            .unwrap_or(path)
            .trim_end_matches('/');
        if self.is_ssh() {
            let user = self.user.as_deref().unwrap_or("git");
            return format!("{user}@{}:{path}", self.host);
        }
        if self.scheme.is_empty() {
            return path.to_string();
        }
        let user = self
            .user
            .as_ref()
            .map(|u| format!("{u}@"))
            .unwrap_or_default();
        let port = self.port.map(|p| format!(":{p}")).unwrap_or_default();
        format!("{}://{user}{}{port}/{path}", self.scheme, self.host)
    }

    /// The repository's name: the last component of the path, without
    /// `.git`, as git names the directory it clones into.
    pub fn name(&self) -> Option<&str> {
        self.components().next()
    }

    /// The last two components of the path, `owner/repo`, without `.git`.
    pub fn owner_repo(&self) -> Option<String> {
        let mut parts = self.components();
        let repo = parts.next()?;
        let owner = parts.next()?;
        Some(format!("{owner}/{repo}"))
    }

    /// The path's components, last first, the last without `.git` (a
    /// trailing `/.git` goes too, as git drops it).
    fn components(&self) -> impl Iterator<Item = &str> {
        let path = self.path.trim_end_matches(['/', '\\']);
        let path = path.strip_suffix(".git").unwrap_or(path);
        let path = path.trim_end_matches(['/', '\\']);
        path.rsplit(['/', '\\']).take_while(|p| !p.is_empty())
    }
}

/// A host name or address with nothing else in it: letters, digits, `-`,
/// `_` and `.` between labels, or a bracketed IPv6 address.
fn valid_host(host: &str) -> bool {
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return !inner.is_empty()
            && inner
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
    }
    !host.is_empty()
        && host
            .split('.')
            .all(|label| !label.is_empty() && !label.starts_with('-'))
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(url: &str) -> GitUrl {
        url.parse().unwrap()
    }

    #[test]
    fn reads_each_form() {
        let scp = parse("git@github.com:org/repo.git");
        assert_eq!(
            (
                scp.scheme.as_str(),
                scp.user.as_deref(),
                scp.host.as_str(),
                scp.path.as_str()
            ),
            ("ssh", Some("git"), "github.com", "org/repo.git")
        );
        let ssh = parse("ssh://deploy@example.com:2222/org/repo");
        assert_eq!((ssh.port, ssh.path.as_str()), (Some(2222), "org/repo"));
        let v6 = parse("ssh://[::1]/repo");
        assert_eq!((v6.host.as_str(), v6.port), ("[::1]", None));
        let https = parse("https://user:token@github.com/org/repo");
        assert_eq!(
            (https.host.as_str(), https.user.as_deref()),
            ("github.com", Some("user:token"))
        );
        assert_eq!(parse("/srv/git/repo.git").scheme, "");
        assert_eq!(parse("./a:b").scheme, "");
        assert!("ssh://host:port/repo".parse::<GitUrl>().is_err());
    }

    #[test]
    fn only_plain_ssh_hosts_are_ssh() {
        assert!(parse("git@github.com:org/repo").is_ssh());
        assert!(parse("ssh://git@github.com/org/repo").is_ssh());
        assert!(parse("host.local:repo").is_ssh());
        assert!(!parse("https://github.com/org/repo").is_ssh());
        assert!(!parse("/srv/repo").is_ssh());
        assert!(!parse("ssh://user:pw@host/repo").is_ssh());
        // what ssh would read as an option
        assert!(!parse("ssh://-oProxyCommand=x@host/repo").is_ssh());
        assert!(!parse("ssh://-oProxyCommand/repo").is_ssh());
        assert!(!parse("git@bad..host:repo").is_ssh());
    }

    #[test]
    fn same_repository_normalizes_alike() {
        let n = |url: &str| parse(url).normalized();
        assert_eq!(
            n("git@github.com:org/repo.git"),
            n("ssh://git@github.com/org/repo")
        );
        assert_eq!(
            n("ssh://github.com:22/org/repo/"),
            n("git@github.com:org/repo")
        );
        assert_ne!(
            n("git@github.com:org/repo"),
            n("https://github.com/org/repo")
        );
        assert_ne!(n("git@github.com:org/repo"), n("git@github.com:org/other"));
    }

    #[test]
    fn owner_repo_is_the_last_two_components() {
        for url in [
            "git@github.com:org/api.git",
            "https://github.com/org/api.git/\n",
            "C:\\remotes\\org\\api.git",
            "ssh://git@host/org/api.git",
            "/srv/org/api",
        ] {
            assert_eq!(parse(url).owner_repo().as_deref(), Some("org/api"), "{url}");
        }
        assert_eq!(parse("api").owner_repo(), None);
        assert_eq!(parse("git@host:repo.git").name(), Some("repo"));
        assert_eq!(parse("/srv/repo/.git").name(), Some("repo"));
    }
}
