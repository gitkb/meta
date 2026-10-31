//! Git remote URLs, read the way git reads them.

use crate::display::escape;
use std::borrow::Cow;
use std::ffi::OsStr;
use std::fmt;
use std::ops::Range;

/// A remote URL as configured. Everything that prints one (`Display`,
/// `Debug`, `Serialize`) hides the password or token it may carry
/// (`https://***@host/org/repo`); the URL itself reaches only git's argv,
/// through [`GitUrl::as_git_arg`].
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
        f.write_str(&escape(&self.redacted()))
    }
}

impl fmt::Debug for GitUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("GitUrl").field(&self.redacted()).finish()
    }
}

/// The URL with its credentials hidden.
impl serde::Serialize for GitUrl {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.redacted())
    }
}

impl<'de> serde::Deserialize<'de> for GitUrl {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(GitUrl)
    }
}

impl GitUrl {
    /// The URL as configured, for git's argv and nothing else.
    pub fn as_git_arg(&self) -> &OsStr {
        OsStr::new(&self.0)
    }

    /// The URL as configured, for meta_core's own checks of it.
    pub(crate) fn raw(&self) -> &str {
        &self.0
    }

    /// The URL without the credentials in it (see [`redact_url`]).
    fn redacted(&self) -> Cow<'_, str> {
        redact(&self.0, self.ends())
    }

    /// Where the URL's authority ends: ext::'s address is a command, read
    /// as text. Any other helper gets its address as typed, so it is one URL
    fn ends(&self) -> fn(char) -> bool {
        match is_ext(&self.0) {
            true => ends_in_text,
            false => ends_in_url,
        }
    }

    /// `text`, which git or a remote helper printed for this URL, without
    /// this URL's credentials wherever a URL shows them (`://<userinfo>@`):
    /// a helper may print its address without its `<transport>::`, which
    /// no scan of the text can read.
    pub fn hide_in<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut text = Cow::Borrowed(text);
        for userinfo in hidden(&self.0, self.ends()) {
            let shown = format!("://{}@", self.0.get(userinfo).unwrap_or_default());
            if text.contains(&shown) {
                text = Cow::Owned(text.replace(&shown, "://***@"));
            }
        }
        text
    }

    /// Whether printing this URL hides credentials it holds: a command
    /// shown with it won't run as printed.
    pub fn hides_credentials(&self) -> bool {
        self.redacted() != self.0
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
            // a name the printed URL lacks holds a credential (a URL in a
            // helper's command reads as an scp path)
            .filter(|name| self.redacted().contains(name))
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

/// `text` (git's errors, a config file) with the userinfo of every
/// `scheme://userinfo@host` in it hidden: `https://alice:token@host/repo`
/// becomes `https://***@host/repo`. An ssh user name alone stays
/// (`ssh://git@host/repo`): it is no secret. A URL's authority ends at
/// `/`, `?`, `#` or ASCII whitespace (which git's curl refuses in a URL;
/// quotes and Unicode spaces it sends), and runs to its last `@`. A remote
/// helper's URL (`foo::https://…`, but not ext::'s command) ends only at
/// `/`, `?`, `#` or a line break: the helper gets it as typed.
pub fn redact_urls(text: &str) -> Cow<'_, str> {
    redact(text, ends_in_text)
}

fn ends_in_text(c: char) -> bool {
    matches!(c, '/' | '?' | '#') || c.is_ascii_whitespace()
}

/// The most bytes git writes for one message, its newline included: git cuts
/// a longer one (`vreportf` in usage.c), possibly inside a URL.
pub const GIT_MESSAGE_BYTES: usize = 4096;

/// git's own output without the credentials in it (see [`redact_urls`]). A
/// line git may have cut ([`GIT_MESSAGE_BYTES`] less its newline, or more;
/// so `text` as git wrote it, untrimmed) loses its last word, and then a URL
/// whose authority runs to its end, spaces after it aside: either may be a
/// URL whose `@` was cut off.
pub fn redact_git_output(text: &str) -> String {
    text.lines()
        .map(|line| {
            let line = match line.len() >= GIT_MESSAGE_BYTES - 1 {
                true => {
                    let line = line
                        .rsplit_once(|c: char| c.is_ascii_whitespace())
                        .map_or("", |(kept, _)| kept)
                        .trim_ascii_end();
                    authorities(line, ends_in_text)
                        .into_iter()
                        .find(|(_, authority)| authority.end == line.len())
                        .and_then(|(_, authority)| line.get(..authority.start))
                        .unwrap_or(line)
                }
                false => line,
            };
            redact_urls(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_ext(url: &str) -> bool {
    url.trim_start().starts_with("ext::")
}

/// One URL without the credentials in it (see [`redact_urls`]): its
/// authority ends only at `/`, `?` or `#`.
pub fn redact_url(url: &str) -> Cow<'_, str> {
    redact(url, ends_in_url)
}

fn ends_in_url(c: char) -> bool {
    matches!(c, '/' | '?' | '#')
}

/// [`redact_urls`], with the authority ending where `ends` says.
fn redact(text: &str, ends: fn(char) -> bool) -> Cow<'_, str> {
    let mut shown = String::new();
    let mut copied = 0;
    for userinfo in hidden(text, ends) {
        shown.push_str(text.get(copied..userinfo.start).unwrap_or_default());
        shown.push_str("***");
        copied = userinfo.end;
    }
    if copied == 0 {
        return Cow::Borrowed(text);
    }
    shown.push_str(text.get(copied..).unwrap_or_default());
    Cow::Owned(shown)
}

/// Where in `text` the userinfo [`redact`] hides lies.
fn hidden(text: &str, ends: fn(char) -> bool) -> Vec<Range<usize>> {
    authorities(text, ends)
        .into_iter()
        .filter_map(|(scheme, authority)| {
            let (userinfo, _) = text.get(authority.clone())?.rsplit_once('@')?;
            // nothing to hide: no scheme, no userinfo (`npm://@scope/pkg`),
            // an ssh user name, or hidden already
            let plain = scheme.is_empty()
                || userinfo.is_empty()
                || userinfo == "***"
                || (is_ssh_scheme(scheme) && is_plain_user(userinfo));
            (!plain).then(|| authority.start..authority.start + userinfo.len())
        })
        .collect()
}

/// Each `scheme://` in `text`: its scheme, and where its authority lies,
/// which ends where `ends` says; a remote helper's URL holds what the
/// helper takes, a space too (see [`redact_urls`]).
fn authorities(text: &str, ends: fn(char) -> bool) -> Vec<(&str, Range<usize>)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(separator) = text.get(from..).and_then(|rest| rest.find("://")) {
        let separator = from + separator;
        let host_start = separator + 3;
        from = host_start;
        let before = text.get(..separator).unwrap_or_default();
        let scheme_len = before
            .bytes()
            .rev()
            .take_while(|&b| is_scheme_byte(b))
            .count();
        let scheme = before.get(before.len() - scheme_len..).unwrap_or_default();
        let transport = before
            .get(..before.len() - scheme_len)
            .and_then(|b| b.strip_suffix("::"))
            .map(|b| {
                let len = b.bytes().rev().take_while(|&b| is_scheme_byte(b)).count();
                b.get(b.len() - len..).unwrap_or_default()
            });
        let helper = transport.is_some_and(|t| is_scheme(t) && t != "ext");
        let rest = text.get(host_start..).unwrap_or_default();
        let len = rest
            .find(|c| ends(c) && !(helper && matches!(c, ' ' | '\t' | '\x0C')))
            .unwrap_or(rest.len());
        found.push((scheme, host_start..host_start + len));
    }
    found
}

/// A URL scheme or remote helper name, as git reads one: a letter or digit,
/// then letters, digits, `+`, `-` and `.`.
fn is_scheme(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_alphanumeric()) && s.bytes().all(is_scheme_byte)
}

fn is_scheme_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')
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
            "ext::sh -c curl% https://alice:ghp_SECRET@host",
        ] {
            let url = parse(url);
            assert!(matches!(url.parts(), Parts::Helper(_)), "{url:?}");
            assert!(url.ssh().is_none(), "{url:?}");
        }
        assert_eq!(parse("ext::sh -c git% %s% /srv/r1.git").name(), Some("r1"));
        // the name of a URL a helper reads never holds its credentials
        for url in [
            "persistent-https::https://alice:ghp_SECRET@example.invalid/",
            "codecommit::us-east-1://profile@repo",
            "ext::sh -c curl% https://alice:ghp_SECRET@host",
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
            // one URL's authority ends only at `/`, `?` or `#`
            ("https://a b:ghp_SECRET@h/r", "https://***@h/r"),
            ("https://a'b:ghp_SECRET@h/r", "https://***@h/r"),
            // an `@` after the host is no userinfo
            ("https://host?a=b@c", "https://host?a=b@c"),
            ("https://host#x@y", "https://host#x@y"),
            (
                "persistent-https::https://ghp_SECRET@host/r",
                "persistent-https::https://***@host/r",
            ),
            // nothing to hide: printed as configured
            ("ssh://git@host:2222/r", "ssh://git@host:2222/r"),
            ("git@github.com:org/repo.git", "git@github.com:org/repo.git"),
            ("https://github.com/org/repo", "https://github.com/org/repo"),
            ("/srv/a@b/repo", "/srv/a@b/repo"),
            ("ext::sh -c git% %s% /srv/r", "ext::sh -c git% %s% /srv/r"),
            (
                "ext::env SITE=https://h.example AUTHOR=bob@h.example git-upload-pack /r.git",
                "ext::env SITE=https://h.example AUTHOR=bob@h.example git-upload-pack /r.git",
            ),
            // any other helper gets one URL, a space in its password included
            ("foo::https://alice:p w@h/r", "foo::https://***@h/r"),
            ("npm://@acme/ui", "npm://@acme/ui"),
            // a URL inside a helper's command
            (
                "ext::sh -c curl% https://alice:ghp_SECRET@host/r",
                "ext::sh -c curl% https://***@host/r",
            ),
        ] {
            let url = parse(url);
            assert_eq!(url.to_string(), shown);
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
        assert!(url.hides_credentials());
        assert!(!parse("ssh://git@host/r").hides_credentials());
        // printed as text, a control character is escaped too
        assert_eq!(
            parse("https://h/r\u{1b}[2J").to_string(),
            "https://h/r\\u{1b}[2J"
        );
    }

    #[test]
    fn credentials_in_text_are_hidden() {
        assert_eq!(
            redact_urls(
                "fatal: unable to access 'https://a:ghp_SECRET%0A@h/r/': x\n\
                 and 'git+https://ghp_SECRET@h' or ssh://git@h/r"
            ),
            "fatal: unable to access 'https://***@h/r/': x\n\
             and 'git+https://***@h' or ssh://git@h/r"
        );
        // in text, a URL ends at whitespace, but not at a quote, which a
        // password may hold
        assert_eq!(
            redact_urls("a: https://h\nb: git@h:r\nc: 'https://t@h'"),
            "a: https://h\nb: git@h:r\nc: 'https://***@h'"
        );
        assert_eq!(
            redact_urls(r#"git fetch "http://a"b:t@h/r" x"#),
            r#"git fetch "http://***@h/r" x"#
        );
        // nor at a Unicode space, which git sends too
        assert_eq!(
            redact_urls("http://a\u{a0}b\u{2003}c:t@h/r"),
            "http://***@h/r"
        );
        assert_eq!(
            redact_urls("x 'https://a'b:t@h/r' y"),
            "x 'https://***@h/r' y"
        );
        assert_eq!(redact_urls("-+https://a:t@h"), "-+https://***@h");
        // a helper gets its URL as typed, but ext:: a command
        assert_eq!(
            redact_urls("src=foo::https://a:p w@h/r x@y"),
            "src=foo::https://***@h/r x@y"
        );
        assert_eq!(
            redact_urls("ext::https://h a@b\nfoo::https://h\nc@d"),
            "ext::https://h a@b\nfoo::https://h\nc@d"
        );
        assert_eq!(
            redact_urls("note:: https://h see @bob"),
            "note:: https://h see @bob"
        );
        assert!(matches!(redact_urls("https://***@h/r"), Cow::Borrowed(_)));
        assert!(matches!(
            redact_urls("no URL: a@b, ://x@y"),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn git_output_shows_no_credentials() {
        // a message git cut inside a URL, a Unicode space in its password
        let cut = format!(
            "warning: https://alice:ghp_SECRET\u{2003}{}",
            "x".repeat(4100)
        );
        let cut = cut.get(..GIT_MESSAGE_BYTES - 1).unwrap();
        assert_eq!(
            redact_git_output(&format!("{cut}\nfatal: 'https://a:ghp_SECRET@h/r'")),
            "warning:\nfatal: 'https://***@h/r'"
        );
        // a helper's URL, a space in its password, cut before its `@`
        let cut = format!("e: foo::https://a:ghp_SECRET {}@h/r", "x".repeat(4100));
        let cut = cut.get(..GIT_MESSAGE_BYTES - 1).unwrap();
        assert_eq!(redact_git_output(cut), "e: foo::https://");
        // or among the spaces in it
        let cut = format!("e: https://a:ghp_SECRET{}", " ".repeat(4100));
        let cut = cut.get(..GIT_MESSAGE_BYTES - 1).unwrap();
        assert_eq!(redact_git_output(cut), "e: https://");
    }

    #[test]
    fn a_known_url_is_hidden_however_it_shows() {
        let url = parse("foo::https://alice:ghp_SECRET w@h/r");
        // as the helper prints its address: no scan of the text reads it
        assert_eq!(
            url.hide_in("fatal: cannot open https://alice:ghp_SECRET w@h/r"),
            "fatal: cannot open https://***@h/r"
        );
        // a lone user name is no secret to look for in text
        let text = "data@h: https://bob@h/r";
        assert!(matches!(
            parse("https://a@h/r").hide_in(text),
            Cow::Borrowed(_)
        ));
        assert_eq!(
            parse("https://bob@h/r").hide_in(text),
            "data@h: https://***@h/r"
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
