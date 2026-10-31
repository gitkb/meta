//! A repository's state from one `git status --porcelain=v2 --branch -z`.

use std::path::Path;

/// What `git status` says about a checkout: its branch, how it stands
/// against its upstream, and each changed path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoStatus {
    /// The branch checked out; `None` when HEAD is detached.
    pub branch: Option<String>,
    /// The commit HEAD names; `None` before the first commit.
    pub oid: Option<String>,
    /// The upstream branch, short (`origin/main`); `None` without one.
    pub upstream: Option<String>,
    /// Commits ahead of and behind the upstream; `None` without one (or
    /// when the upstream branch is gone).
    pub ahead_behind: Option<(usize, usize)>,
    pub entries: Vec<StatusEntry>,
}

/// One path `git status` lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusEntry {
    /// A tracked path (for a rename, the new one) that differs in the index
    /// (`staged`), in the work tree (`unstaged`), or both. A conflicted path
    /// counts as both.
    Changed {
        path: String,
        staged: bool,
        unstaged: bool,
    },
    Untracked(String),
}

impl RepoStatus {
    /// The status of the checkout at `dir`. Untracked files and the
    /// upstream counts are always read: a config that hides them
    /// (`status.showUntrackedFiles=no`, `status.aheadBehind=false`) must not
    /// make a checkout look clean.
    pub fn read(dir: &Path) -> anyhow::Result<Self> {
        // git would answer for the repository around a directory that has
        // none of its own (a project not cloned yet, say)
        if crate::gitdir::common_dir(dir)?.is_none() {
            anyhow::bail!(
                "{} is not a git repository",
                crate::display::escape_path(dir)
            );
        }
        let status = super::run(
            dir,
            [
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=normal",
                "--ahead-behind",
            ],
        );
        match status {
            Ok(output) => Ok(Self::parse(&output)),
            // a bare repository has no work tree for `git status`, so
            // nothing in it is changed; its branch and upstream still count.
            // A checkout whose config says `core.bare` is no bare repository:
            // its work tree may hold changes git can't report.
            Err(e) if Self::is_bare(dir) => Self::refs(dir).ok_or(e),
            Err(e) => Err(e),
        }
    }

    /// Whether `dir` is itself the git directory of a bare repository.
    fn is_bare(dir: &Path) -> bool {
        super::query(dir, ["rev-parse", "--is-bare-repository", "--git-dir"]).as_deref()
            == Some("true\n.")
    }

    /// What git says about the repository at `dir` without its work tree:
    /// branch, commit and upstream, and no entries. For a bare repository,
    /// or a checkout whose work tree git refuses (`core.bare=true`), for
    /// callers that show what they can. `None` when `dir` has no repository
    /// of its own.
    pub fn refs(dir: &Path) -> Option<Self> {
        crate::gitdir::common_dir(dir).ok()??;
        // nor when git can't read it at all (ownership, a broken `.git`)
        super::query(dir, ["rev-parse", "--git-dir"])?;
        let upstream = super::query(dir, ["rev-parse", "--abbrev-ref", "@{upstream}"]);
        let ahead_behind = upstream
            .as_ref()
            .and_then(|_| {
                super::query(
                    dir,
                    ["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
                )
            })
            .and_then(|counts| {
                let (ahead, behind) = counts.split_once('\t')?;
                Some((ahead.parse().ok()?, behind.parse().ok()?))
            });
        Some(Self {
            branch: super::current_branch(dir),
            oid: super::query(dir, ["rev-parse", "--verify", "--quiet", "HEAD"]),
            upstream,
            ahead_behind,
            entries: Vec::new(),
        })
    }

    fn parse(output: &str) -> Self {
        let mut status = Self::default();
        let mut records = output.split('\0').filter(|r| !r.is_empty());
        while let Some(record) = records.next() {
            let (kind, rest) = record.split_once(' ').unwrap_or((record, ""));
            match kind {
                "#" => status.header(rest),
                "?" => status
                    .entries
                    .push(StatusEntry::Untracked(rest.to_string())),
                // `1 XY sub mH mI mW hH hI path`, `2 ... Xscore path` then
                // the old path as its own record, `u XY sub m1 m2 m3 mW h1
                // h2 h3 path`; the path may hold spaces, so it is what is
                // left after the fixed fields
                "1" | "2" | "u" => {
                    let fields = match kind {
                        "1" => 7,
                        "2" => 8,
                        _ => 9,
                    };
                    let mut parts = rest.splitn(fields + 1, ' ');
                    let xy = parts.next().unwrap_or_default();
                    let path = parts.nth(fields - 1).unwrap_or_default().to_string();
                    let mut codes = xy.chars();
                    let mut changed = || codes.next().is_some_and(|c| c != '.');
                    status.entries.push(StatusEntry::Changed {
                        path,
                        staged: changed(),
                        unstaged: changed(),
                    });
                    if kind == "2" {
                        records.next();
                    }
                }
                _ => {}
            }
        }
        status
    }

    fn header(&mut self, line: &str) {
        let Some((key, value)) = line.split_once(' ') else {
            return;
        };
        match key {
            "branch.oid" if value != "(initial)" => self.oid = Some(value.to_string()),
            "branch.head" if value != "(detached)" => self.branch = Some(value.to_string()),
            "branch.upstream" => self.upstream = Some(value.to_string()),
            "branch.ab" => {
                let mut counts = value
                    .split(' ')
                    .map(|n| n.trim_start_matches(['+', '-']).parse::<usize>());
                if let (Some(Ok(ahead)), Some(Ok(behind))) = (counts.next(), counts.next()) {
                    self.ahead_behind = Some((ahead, behind));
                }
            }
            _ => {}
        }
    }

    /// Whether anything is changed or untracked.
    pub fn is_dirty(&self) -> bool {
        !self.entries.is_empty()
    }

    /// The changed tracked paths (for a rename, the new one).
    pub fn changed_paths(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().filter_map(|e| match e {
            StatusEntry::Changed { path, .. } => Some(path.as_str()),
            StatusEntry::Untracked(_) => None,
        })
    }

    pub fn untracked_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e, StatusEntry::Untracked(_)))
            .count()
    }

    pub fn has_staged(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e, StatusEntry::Changed { staged: true, .. }))
    }

    pub fn has_unstaged(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e, StatusEntry::Changed { unstaged: true, .. }))
    }

    pub fn has_untracked(&self) -> bool {
        self.untracked_count() > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::run;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        run(dir.path(), ["init", "-q", "-b", "main"]).unwrap();
        dir
    }

    fn commit(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        run(dir, ["add", file]).unwrap();
        run(
            dir,
            [
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                file,
            ],
        )
        .unwrap();
    }

    #[test]
    fn reads_branch_changes_and_upstream() {
        let upstream = repo();
        commit(upstream.path(), "a");
        let dir = tempfile::tempdir().unwrap();
        let clone = dir.path().join("clone");
        run(
            dir.path(),
            [
                "clone".as_ref(),
                "-q".as_ref(),
                upstream.path().as_os_str(),
                clone.as_os_str(),
            ],
        )
        .unwrap();
        commit(&clone, "b");
        commit(upstream.path(), "c");
        run(&clone, ["fetch", "-q"]).unwrap();
        run(&clone, ["config", "status.aheadBehind", "false"]).unwrap();
        std::fs::write(clone.join("a"), "changed").unwrap();
        std::fs::write(clone.join("new file"), "").unwrap();
        run(&clone, ["mv", "b", "b renamed"]).unwrap();

        let status = RepoStatus::read(&clone).unwrap();
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.ahead_behind, Some((1, 1)));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert!(status.oid.is_some());
        let mut changed: Vec<_> = status.changed_paths().collect();
        changed.sort_unstable();
        assert_eq!(changed, ["a", "b renamed"]);
        assert_eq!(status.untracked_count(), 1);
        assert!(status.has_staged() && status.has_unstaged() && status.is_dirty());
    }

    #[test]
    fn each_kind_of_change_is_told_apart() {
        let dir = repo();
        commit(dir.path(), "a");
        let status = |dir: &Path| RepoStatus::read(dir).unwrap();
        let kinds = |s: &RepoStatus| {
            (
                s.has_staged(),
                s.has_unstaged(),
                s.has_untracked(),
                s.is_dirty(),
            )
        };

        std::fs::write(dir.path().join("new"), "").unwrap();
        // what a config hides is still there
        run(dir.path(), ["config", "status.showUntrackedFiles", "no"]).unwrap();
        run(dir.path(), ["config", "status.aheadBehind", "false"]).unwrap();
        assert_eq!(kinds(&status(dir.path())), (false, false, true, true));
        std::fs::remove_file(dir.path().join("new")).unwrap();

        std::fs::write(dir.path().join("a"), "changed").unwrap();
        assert_eq!(kinds(&status(dir.path())), (false, true, false, true));
        run(dir.path(), ["add", "a"]).unwrap();
        assert_eq!(kinds(&status(dir.path())), (true, false, false, true));
    }

    #[test]
    fn a_bare_repository_has_a_branch_commit_and_upstream() {
        let upstream = repo();
        commit(upstream.path(), "a");
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("bare.git");
        run(
            dir.path(),
            [
                "clone".as_ref(),
                "-q".as_ref(),
                "--bare".as_ref(),
                upstream.path().as_os_str(),
                bare.as_os_str(),
            ],
        )
        .unwrap();
        for args in [
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ][..],
            &["config", "branch.main.remote", "origin"],
            &["config", "branch.main.merge", "refs/heads/main"],
        ] {
            run(&bare, args).unwrap();
        }
        commit(upstream.path(), "b");
        run(&bare, ["fetch", "-q"]).unwrap();

        let status = RepoStatus::read(&bare).unwrap();
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert!(status.oid.is_some());
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!(status.ahead_behind, Some((0, 1)));
        assert!(!status.is_dirty());
        // what isn't a repository is still an error, even inside one
        assert!(RepoStatus::read(dir.path()).is_err());
        let inside = upstream.path().join("not-cloned");
        std::fs::create_dir(&inside).unwrap();
        assert!(RepoStatus::read(&inside).is_err());
        // and so is a checkout whose config says it is bare: it may hold
        // changes git can't report
        let checkout = repo();
        commit(checkout.path(), "a");
        std::fs::write(checkout.path().join("dirty"), "").unwrap();
        run(checkout.path(), ["config", "core.bare", "true"]).unwrap();
        assert!(RepoStatus::read(checkout.path()).is_err());
        // whose refs can still be shown
        let refs = RepoStatus::refs(checkout.path()).unwrap();
        assert_eq!(refs.branch.as_deref(), Some("main"));
        assert!(RepoStatus::refs(&inside).is_none());
    }

    #[test]
    fn a_new_repo_has_no_commit_or_upstream() {
        let dir = repo();
        let status = RepoStatus::read(dir.path()).unwrap();
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!((&status.oid, status.ahead_behind), (&None, None));
        assert!(!status.is_dirty());
    }

    #[test]
    fn detached_head_has_no_branch() {
        let dir = repo();
        commit(dir.path(), "a");
        run(dir.path(), ["checkout", "-q", "--detach"]).unwrap();
        assert_eq!(RepoStatus::read(dir.path()).unwrap().branch, None);
    }
}
