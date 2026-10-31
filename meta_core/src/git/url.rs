//! Git remote URLs, read the way git reads them.

use crate::display::escape;
use std::borrow::Cow;
use std::ffi::OsStr;
use std::fmt;
use std::ops::Range;

/// A remote URL: as configured, where meta refuses credentials (see
/// [`GitUrl::has_credentials`]), or as a clone's remote has it, which may hold
/// them. Everything that prints one (`Display`, `Debug`, `Serialize`) hides
/// them (`https://***@host/org/repo`); the URL itself reaches only git's
/// argv, through [`GitUrl::as_git_arg`].
#[derive(Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct GitUrl(String);

/// A [`GitUrl`] taken apart as git does, in the order git checks the forms.
#[derive(Debug, PartialEq, Eq)]
enum Parts<'a> {
    /// `<transport>::<address>`: git hands the address to
    /// `git-remote-<transport>` (`ext::`, `fd::`, `codecommit::`).
    Helper(&'a str),
    /// `scheme://[userinfo@]host[:port]/path`
    Url {
        scheme: &'a str,
        userinfo: Option<&'a str>,
        host: &'a str,
        port: Option<&'a str>,
        /// From the `/`, `?` or `#` after the host.
        path: &'a str,
    },
    /// scp-like `[user@]host:path`, which git reaches over ssh.
    Scp {
        user: Option<&'a str>,
        host: &'a str,
        path: &'a str,
    },
    Local(&'a str),
}

impl<'a> Parts<'a> {
    fn of(url: &'a str) -> Self {
        let url = url.trim();
        // A remote helper comes first: `ext::ssh -p 2222 host` is no scp
        if let Some((transport, address)) = url.split_once("::") {
            if is_scheme(transport) {
                return Parts::Helper(address);
            }
        }
        if let Some((scheme, rest)) = url.split_once("://").filter(|(s, _)| is_scheme(s)) {
            // the authority ends where the path, query or fragment starts
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            let (authority, path) = rest.split_at_checked(end).unwrap_or((rest, ""));
            let (userinfo, host_port) = match authority.rsplit_once('@') {
                Some((userinfo, host_port)) => (Some(userinfo), host_port),
                None => (None, authority),
            };
            // `[v6]:port`, `host:port` or a bare host
            let (host, port) = match host_port.rsplit_once(':') {
                Some((host, port)) if !port.contains(']') => (host, Some(port)),
                _ => (host_port, None),
            };
            return Parts::Url {
                scheme,
                userinfo,
                host,
                port: port.filter(|p| !p.is_empty()),
                path,
            };
        }
        // As git: scp-like when a `:` comes before any `/`, and what is
        // before it isn't a Windows drive letter. A bracketed host
        // (`[::1]:repo`, `git@[::1]:repo`: at the start or right after the
        // `@`) keeps its own colons.
        let open = match url.starts_with('[') {
            true => Some(0),
            false => url.find("@[").map(|at| at + 1),
        };
        let host_end = open
            .and_then(|open| Some(open + url.get(open..)?.find(']')?))
            .unwrap_or(0);
        let split = url
            .get(host_end..)
            .and_then(|rest| rest.find(':'))
            .and_then(|at| Some((url.get(..host_end + at)?, url.get(host_end + at + 1..)?)));
        match split {
            Some((authority, path))
                if !authority.contains('/') && !(authority.len() == 1 && cfg!(windows)) =>
            {
                let (user, host) = match authority.split_once('@') {
                    Some((user, host)) => (Some(user), host),
                    None => (None, authority),
                };
                Parts::Scp { user, host, path }
            }
            _ => Parts::Local(url),
        }
    }
}

impl From<String> for GitUrl {
    fn from(url: String) -> Self {
        GitUrl(url)
    }
}

impl From<&str> for GitUrl {
    fn from(url: &str) -> Self {
        GitUrl(url.to_string())
    }
}

/// The URL with its credentials hidden, and unsafe characters escaped.
impl fmt::Display for GitUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&escape(&self.shown()))
    }
}

impl fmt::Debug for GitUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("GitUrl").field(&self.shown()).finish()
    }
}

/// The URL with its credentials hidden.
impl serde::Serialize for GitUrl {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.shown())
    }
}

impl<'de> serde::Deserialize<'de> for GitUrl {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(GitUrl)
    }
}

/// Why meta refuses a URL that [`GitUrl::has_credentials`], and what to do
/// instead.
pub const CREDENTIALS_REFUSED: &str = "holds a password or token, which meta refuses: \
     remove it from the URL and let a git credential helper supply it \
     (for GitHub: gh auth setup-git)";

impl GitUrl {
    /// The URL as configured, for git's argv and nothing else.
    pub fn as_git_arg(&self) -> &OsStr {
        OsStr::new(&self.0)
    }

    /// The URL as configured, for meta_core's own checks of it.
    pub(crate) fn raw(&self) -> &str {
        &self.0
    }

    /// The URL with `***` for its credentials.
    fn shown(&self) -> Cow<'_, str> {
        match (self.credentials(), &self.0) {
            (Some(at), url) => Cow::Owned(format!(
                "{}***{}",
                url.get(..at.start).unwrap_or_default(),
                url.get(at.end..).unwrap_or_default()
            )),
            (None, url) => Cow::Borrowed(url),
        }
    }

    /// Whether the URL holds a password or token: meta refuses such a URL
    /// in its config, and git's own credential helpers are for them.
    pub fn has_credentials(&self) -> bool {
        self.credentials().is_some()
    }

    /// Where the credentials lie in the URL git, or the remote helper named
    /// before it (`persistent-https::https://…`), reads: a userinfo with a
    /// password (`user:password@`, the `:` possibly percent-encoded), or any
    /// userinfo of a URL curl reads, as a token may stand in for the user
    /// name there (`https://ghp_…@github.com/…`). A user name alone, such as
    /// ssh's `git@`, is none. ext::'s address is a command, no URL.
    fn credentials(&self) -> Option<Range<usize>> {
        let mut at = self.0.len() - self.0.trim_start().len();
        let mut url = self.0.get(at..)?;
        while let Some((transport, address)) = url.split_once("::") {
            if transport == "ext" {
                return None;
            }
            if !is_scheme(transport) {
                break;
            }
            at += transport.len() + 2;
            url = address;
        }
        let (scheme, rest) = url.split_once("://").filter(|(s, _)| is_scheme(s))?;
        let authority = rest.split(['/', '?', '#']).next()?;
        let (userinfo, _) = authority.rsplit_once('@')?;
        let curl = matches!(
            scheme.to_ascii_lowercase().as_str(),
            "http" | "https" | "ftp" | "ftps"
        );
        let password = userinfo.contains(':') || userinfo.to_ascii_lowercase().contains("%3a");
        let start = at + scheme.len() + 3;
        (!userinfo.is_empty() && (curl || password)).then_some(start..start + userinfo.len())
    }

    /// The URL taken apart as git does.
    fn parts(&self) -> Parts<'_> {
        Parts::of(&self.0)
    }

    /// Where git connects over ssh for this remote: user (if given), a
    /// plain host name or address, and port. `None` when git doesn't use
    /// ssh, or when ssh would read the host or user as an option or the
    /// user carries a password.
    pub fn ssh(&self) -> Option<(Option<&str>, &str, u16)> {
        let (user, host, port) = match self.parts() {
            Parts::Url {
                scheme,
                userinfo,
                host,
                port,
                ..
            } if is_ssh_scheme(scheme) => (userinfo, host, port),
            Parts::Scp { user, host, .. } => (user, host, None),
            _ => return None,
        };
        let port = port.map_or(Some(22), |p| p.parse().ok())?;
        (valid_host(host) && user.is_none_or(is_plain_user)).then_some((user, host, port))
    }

    /// Whether `other` names the same repository: the same URL but for
    /// `.git`, a trailing `/`, an ssh port or ssh's default user `git`.
    pub fn same_repository(&self, other: &GitUrl) -> bool {
        self.normalized() == other.normalized()
    }

    /// A form two URLs of the same repository share. It holds the URL's
    /// credentials: compare it, never print it.
    fn normalized(&self) -> String {
        let path = self.repo_path();
        if let Some((user, host, _)) = self.ssh() {
            return format!("{}@{host}:{path}", user.unwrap_or("git"));
        }
        let (scheme, user, host, port) = match self.parts() {
            Parts::Url {
                scheme,
                userinfo,
                host,
                port,
                ..
            } => (scheme, userinfo, host, port),
            Parts::Scp { user, host, .. } => ("ssh", user, host, None),
            Parts::Helper(_) => return self.0.trim().to_string(),
            Parts::Local(_) => return path.to_string(),
        };
        let user = user.map(|u| format!("{u}@")).unwrap_or_default();
        let port = port.map(|p| format!(":{p}")).unwrap_or_default();
        format!(
            "{}://{user}{host}{port}/{path}",
            scheme.to_ascii_lowercase()
        )
    }

    /// The repository's name: the last component of the path, without
    /// `.git`, as git names the directory it clones into (where a `:` also
    /// separates components).
    pub fn name(&self) -> Option<&str> {
        self.repo_path()
            .rsplit(['/', '\\', ':'])
            .next()
            .filter(|name| !name.is_empty())
            // in a URL, a query or fragment right after the host is no
            // name; `/#repo.git` is
            .filter(|_| {
                !matches!(self.repository(), Parts::Url { path, .. } if path.starts_with(['?', '#']))
            })
    }

    /// The last two components of the path, `owner/repo`, without `.git`.
    pub fn owner_repo(&self) -> Option<String> {
        let mut parts = self.components();
        let repo = parts.next()?;
        let owner = parts.next()?;
        Some(format!("{owner}/{repo}"))
    }

    /// The path's components, last first.
    fn components(&self) -> impl Iterator<Item = &str> {
        self.repo_path()
            .rsplit(['/', '\\'])
            .take_while(|p| !p.is_empty())
    }

    /// The parts of the repository the URL names: a helper's address may be
    /// a URL of its own.
    fn repository(&self) -> Parts<'_> {
        let mut parts = self.parts();
        while let Parts::Helper(address) = parts {
            parts = Parts::of(address);
        }
        parts
    }

    /// The path without a trailing `/`, `.git` or `/.git`, as git drops them.
    fn repo_path(&self) -> &str {
        let path = match self.repository() {
            Parts::Url { path, .. } => path.strip_prefix('/').unwrap_or(path),
            Parts::Helper(path) | Parts::Scp { path, .. } | Parts::Local(path) => path,
        };
        let path = path.trim_end_matches(['/', '\\']);
        let path = path.strip_suffix(".git").unwrap_or(path);
        path.trim_end_matches(['/', '\\'])
    }
}

/// A URL scheme or remote helper name, as git reads one: a letter or digit,
/// then letters, digits, `+`, `-` and `.`.
fn is_scheme(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphanumeric())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
}

/// An ssh user name and nothing else: no password after a `:` (or
/// percent-encoded, which git decodes), and nothing ssh reads as an option.
fn is_plain_user(user: &str) -> bool {
    !user.is_empty() && !user.starts_with('-') && !user.contains([':', '%', ' ', '\t'])
}

fn is_ssh_scheme(scheme: &str) -> bool {
    matches!(scheme, "ssh" | "git+ssh" | "ssh+git")
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
        url.into()
    }

    #[test]
    fn reads_each_form() {
        assert_eq!(
            parse("git@github.com:org/repo.git").parts(),
            Parts::Scp {
                user: Some("git"),
                host: "github.com",
                path: "org/repo.git"
            }
        );
        assert_eq!(
            parse("ssh://deploy@example.com:2222/org/repo").ssh(),
            Some((Some("deploy"), "example.com", 2222))
        );
        assert_eq!(parse("ssh://[::1]/repo").ssh(), Some((None, "[::1]", 22)));
        let v6 = parse("git@[::1]:repo.git");
        assert_eq!(v6.ssh(), Some((Some("git"), "[::1]", 22)));
        assert_eq!(v6.name(), Some("repo"));
        // brackets in the path are no host
        for (url, name) in [
            ("git@host:repo[1].git", "repo[1]"),
            ("git@host:[repo].git", "[repo]"),
        ] {
            let bracketed = parse(url);
            assert_eq!(bracketed.ssh().map(|s| s.1), Some("host"));
            assert_eq!(bracketed.name(), Some(name));
        }
        assert!(matches!(
            parse("https://user:token@github.com/org/repo").parts(),
            Parts::Url {
                userinfo: Some("user:token"),
                host: "github.com",
                ..
            }
        ));
        assert_eq!(
            parse("/srv/git/repo.git").parts(),
            Parts::Local("/srv/git/repo.git")
        );
        assert_eq!(parse("./a:b").parts(), Parts::Local("./a:b"));
        // what isn't a scheme is no URL
        assert_eq!(parse("/srv/a://b").parts(), Parts::Local("/srv/a://b"));
        // git fails on a port that isn't one; meta doesn't reach it over ssh
        assert!(parse("ssh://host:port/repo").ssh().is_none());
    }

    #[test]
    fn a_remote_helper_is_never_ssh() {
        for url in [
            "ext::sh -c sleep% 2;% git% %s% /srv/r1.git",
            "ext::ssh -p 2222 host %S 'repo'",
            "fd::7,8",
            "codecommit::us-east-1://profile@repo",
        ] {
            let url = parse(url);
            assert!(matches!(url.parts(), Parts::Helper(_)), "{url:?}");
            assert!(url.ssh().is_none(), "{url:?}");
        }
        assert_eq!(parse("ext::sh -c git% %s% /srv/r1.git").name(), Some("r1"));
        for url in [
            "persistent-https::https://example.invalid/",
            "codecommit::us-east-1://profile@repo",
        ] {
            assert_eq!(parse(url).name(), None, "{url}");
        }
        assert_eq!(
            parse("persistent-https::https://ghp_SECRET@h/org/api.git").name(),
            Some("api")
        );
        assert_eq!(parse("https://u:t@host?x=1").name(), None);
        assert_eq!(parse("/srv/#repo.git").name(), Some("#repo"));
        assert_eq!(parse("file:///srv/#repo.git").name(), Some("#repo"));
        assert_eq!(parse("file:///#repo.git").name(), Some("#repo"));
        assert_eq!(parse("persistent-https::https://h?x=1").name(), None);
        assert!(!parse("https://h?x=1").same_repository(&parse("https://h?y=2")));
        // git allows a helper name to start with a digit
        assert!(matches!(parse("9p::x").parts(), Parts::Helper(_)));
        // not a helper name: what git reads as scp-like or local
        assert!(parse("host:repo::x").ssh().is_some());
        assert_eq!(parse("./a::b").parts(), Parts::Local("./a::b"));
    }

    #[test]
    fn only_plain_ssh_hosts_are_ssh() {
        assert!(parse("git@github.com:org/repo").ssh().is_some());
        assert!(parse("ssh://git@github.com/org/repo").ssh().is_some());
        // git reads schemes case-sensitively: `SSH://` names a helper
        assert!(parse("SSH://git@github.com/org/repo").ssh().is_none());
        assert!(parse("host.local:repo").ssh().is_some());
        assert!(parse("https://github.com/org/repo").ssh().is_none());
        assert!(parse("/srv/repo").ssh().is_none());
        assert!(parse("ssh://user:pw@host/repo").ssh().is_none());
        // what ssh would read as an option
        assert!(parse("ssh://-oProxyCommand=x@host/repo").ssh().is_none());
        assert!(parse("ssh://-oProxyCommand/repo").ssh().is_none());
        assert!(parse("git@bad..host:repo").ssh().is_none());
    }

    #[test]
    fn credentials_never_print() {
        for (url, shown) in [
            (
                "https://alice:ghp_SECRET@github.com/org/repo.git",
                "https://***@github.com/org/repo.git",
            ),
            // a token as the user name
            (
                "https://ghp_SECRET@github.com/org/repo.git",
                "https://***@github.com/org/repo.git",
            ),
            (
                "http://u:ghp_SECRET@[::1]:8080/r",
                "http://***@[::1]:8080/r",
            ),
            ("ssh://git:ghp_SECRET@host/r", "ssh://***@host/r"),
            ("ssh://git%3Aghp_SECRET@host/r", "ssh://***@host/r"),
            ("foo://a:ghp_SECRET@h/r", "foo://***@h/r"),
            // the authority ends only at `/`, `?` or `#`
            ("https://a b:ghp_SECRET@h/r", "https://***@h/r"),
            // an `@` after the host is no userinfo
            ("https://host?a=b@c", "https://host?a=b@c"),
            ("https://host#x@y", "https://host#x@y"),
            ("https://h/a::b@c", "https://h/a::b@c"),
            // the URL a remote helper reads
            (
                "persistent-https::https://ghp_SECRET@host/r",
                "persistent-https::https://***@host/r",
            ),
            // nothing to hide: printed as configured
            ("ssh://git@host:2222/r", "ssh://git@host:2222/r"),
            ("git@github.com:org/repo.git", "git@github.com:org/repo.git"),
            ("https://github.com/org/repo", "https://github.com/org/repo"),
            ("/srv/a@b/repo", "/srv/a@b/repo"),
            (
                "codecommit::us-east-1://profile@repo",
                "codecommit::us-east-1://profile@repo",
            ),
            ("npm://@acme/ui", "npm://@acme/ui"),
            // ext::'s address is a command
            ("ext::sh -c git% %s% /srv/r", "ext::sh -c git% %s% /srv/r"),
        ] {
            let url = parse(url);
            assert_eq!(url.to_string(), shown);
            assert_eq!(url.has_credentials(), url.as_git_arg() != shown, "{shown}");
            assert_eq!(format!("{url:?}"), format!("GitUrl({shown:?})"));
            assert_eq!(serde_json::to_value(&url).unwrap(), shown);
        }
        // git still gets the URL as configured
        let url = parse("https://alice:ghp_SECRET@github.com/org/repo.git");
        assert_eq!(
            url.as_git_arg(),
            "https://alice:ghp_SECRET@github.com/org/repo.git"
        );
        // and a URL read from a config prints the same way
        let read: GitUrl = serde_json::from_str("\"https://t@h/r\"").unwrap();
        assert_eq!(read.to_string(), "https://***@h/r");
        // printed as text, a control character is escaped too
        assert_eq!(
            parse("https://h/r\u{1b}[2J").to_string(),
            "https://h/r\\u{1b}[2J"
        );
    }

    #[test]
    fn same_repository_despite_spelling() {
        let same = |a: &str, b: &str| parse(a).same_repository(&parse(b));
        assert!(same(
            "git@github.com:org/repo.git",
            "ssh://git@github.com/org/repo"
        ));
        assert!(same(
            "ssh://github.com:22/org/repo/",
            "git@github.com:org/repo"
        ));
        assert!(same("HTTPS://h/org/repo.git", "https://h/org/repo"));
        assert!(!same(
            "git@github.com:org/repo",
            "https://github.com/org/repo"
        ));
        assert!(!same("git@github.com:org/repo", "git@github.com:org/other"));
        assert!(!same("https://a:x@h/r", "https://a:y@h/r"));
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
        // git reads a `:` in the path as a separator too
        assert_eq!(parse("git@host:a:b.git").name(), Some("b"));
        assert_eq!(parse("./a:b").name(), Some("b"));
        assert_eq!(parse("git@host:a:b.git").owner_repo(), None);
    }
}
