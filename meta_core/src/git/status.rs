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

/// A git operation stopped half-way (on a conflict, or to edit), waiting to
/// be continued or aborted, as `git status` tells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operation {
    /// `branch`: the branch being rebased; `None` for a detached HEAD.
    Rebase {
        branch: Option<String>,
    },
    Am,
    Merge,
    CherryPick,
    Revert,
    Bisect,
}

impl Operation {
    /// The command's name, as git spells it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Rebase { .. } => "rebase",
            Self::Am => "am",
            Self::Merge => "merge",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
            Self::Bisect => "bisect",
        }
    }

    /// The operation under way in the checkout at `dir`, read the way
    /// `git status` reads it (up to four more git processes, so only
    /// callers that show it ask). git names the files in the checkout's own git directory
    /// (a linked worktree's, never the one it shares) and resolves the refs
    /// in whichever store the repository keeps them. A rebase comes first:
    /// the picks and merges it stops on are its own.
    pub fn read(dir: &Path) -> anyhow::Result<Option<Self>> {
        let out = super::run(
            dir,
            [
                "rev-parse",
                "--git-path",
                "rebase-merge",
                "--git-path",
                "rebase-apply",
                "--git-path",
                "sequencer/todo",
                "--git-path",
                "BISECT_LOG",
            ],
        )?;
        let lines: Vec<&str> = out.lines().collect();
        let &[rebase_merge, rebase_apply, todo, bisect] = lines.as_slice() else {
            anyhow::bail!(
                "git rev-parse --git-path in {} printed '{}'",
                crate::display::escape_path(dir),
                crate::display::escape(&out)
            );
        };
        let exists = |path: &Path| path.symlink_metadata().is_ok();
        for rebase in [rebase_merge, rebase_apply].map(|p| dir.join(p)) {
            if !exists(&rebase) {
                continue;
            }
            if exists(&rebase.join("applying")) {
                return Ok(Some(Self::Am));
            }
            let branch = std::fs::read_to_string(rebase.join("head-name"))
                .ok()
                .and_then(|name| {
                    name.trim_end()
                        .strip_prefix("refs/heads/")
                        .map(str::to_string)
                });
            return Ok(Some(Self::Rebase { branch }));
        }
        // a cherry-pick or revert of several commits whose stopped one was
        // committed by hand leaves only its todo list
        let sequencer = || {
            let todo = std::fs::read_to_string(dir.join(todo)).ok()?;
            match todo.split_whitespace().next()? {
                "pick" | "p" => Some(Self::CherryPick),
                "revert" => Some(Self::Revert),
                _ => None,
            }
        };
        // git's own state, never a branch or tag called so: those resolve
        // to their full name
        for (head, operation) in [
            ("MERGE_HEAD", Self::Merge),
            ("CHERRY_PICK_HEAD", Self::CherryPick),
            ("REVERT_HEAD", Self::Revert),
        ] {
            let name = super::lookup(
                dir,
                [
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    "--symbolic-full-name",
                    head,
                ],
            )?;
            if name.as_deref() == Some(head) {
                return Ok(Some(operation));
            }
        }
        Ok(sequencer().or_else(|| exists(&dir.join(bisect)).then_some(Self::Bisect)))
    }
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
            Err(e) if Self::is_bare(dir) => Self::refs(dir)?.ok_or(e),
            Err(e) => Err(e),
        }
    }

    /// Whether `dir` is itself the git directory of a bare repository.
    fn is_bare(dir: &Path) -> bool {
        super::run(dir, ["rev-parse", "--is-bare-repository", "--git-dir"])
            .is_ok_and(|answer| answer == "true\n.")
    }

    /// What git says about the repository at `dir` without its work tree:
    /// branch, commit and upstream, and no entries. For a bare repository,
    /// or a checkout whose work tree git refuses (`core.bare=true`), for
    /// callers that show what they can. `None` when `dir` has no repository
    /// of its own; an error when git can't read the one it has.
    pub fn refs(dir: &Path) -> anyhow::Result<Option<Self>> {
        if crate::gitdir::common_dir(dir)?.is_none() {
            return Ok(None);
        }
        let branch = super::current_branch(dir)?;
        // the upstream as configured, there or gone: `@{upstream}` doesn't
        // resolve a gone one
        let upstream = match &branch {
            Some(branch) => super::lookup(
                dir,
                [
                    "for-each-ref",
                    "--format=%(upstream:short)",
                    &format!("refs/heads/{branch}"),
                ],
            )?,
            None => None,
        };
        let ahead_behind = match upstream {
            Some(_)
                if super::lookup(dir, ["rev-parse", "--verify", "--quiet", "@{upstream}"])?
                    .is_some() =>
            {
                let counts = super::run(
                    dir,
                    ["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
                )?;
                let parsed = counts
                    .split_once('\t')
                    .and_then(|(ahead, behind)| Some((ahead.parse().ok()?, behind.parse().ok()?)));
                Some(parsed.ok_or_else(|| {
                    anyhow::anyhow!(
                        "git rev-list --count in {} printed '{}'",
                        crate::display::escape_path(dir),
                        crate::display::escape(&counts)
                    )
                })?)
            }
            _ => None,
        };
        Ok(Some(Self {
            branch,
            oid: super::lookup(dir, ["rev-parse", "--verify", "--quiet", "HEAD"])?,
            upstream,
            ahead_behind,
            entries: Vec::new(),
        }))
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

    /// Whether the branch's upstream is set but its branch isn't there
    /// (deleted on the remote and pruned, or never fetched): `git status`
    /// then has no counts for a branch with commits, and `git status -sb`
    /// says `[gone]`.
    pub fn upstream_gone(&self) -> bool {
        self.upstream.is_some() && self.oid.is_some() && self.ahead_behind.is_none()
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
        commit_text(dir, file, file);
    }

    fn commit_text(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        run(dir, ["add", file]).unwrap();
        as_t(dir, &["commit", "-qm", text]).unwrap();
    }

    /// git, committing as `t`
    fn as_t(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
        let id = [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ];
        run(dir, id.iter().chain(args))
    }

    /// `main` and `side` each change `f` from `base`, so bringing one onto
    /// the other stops on a conflict
    fn conflicting() -> tempfile::TempDir {
        let dir = repo();
        make_conflicting(dir.path());
        dir
    }

    fn make_conflicting(dir: &Path) {
        commit_text(dir, "f", "base");
        run(dir, ["switch", "-qc", "side"]).unwrap();
        commit_text(dir, "f", "side");
        run(dir, ["switch", "-q", "main"]).unwrap();
        commit_text(dir, "f", "main");
    }

    fn operation(dir: &Path) -> Option<Operation> {
        Operation::read(dir).unwrap()
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
        // an upstream that isn't there is named, and gone
        run(&bare, ["config", "branch.main.merge", "refs/heads/gone"]).unwrap();
        let status = RepoStatus::read(&bare).unwrap();
        assert_eq!(status.upstream.as_deref(), Some("origin/gone"));
        assert!(status.upstream_gone());
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
        let refs = RepoStatus::refs(checkout.path()).unwrap().unwrap();
        assert_eq!(refs.branch.as_deref(), Some("main"));
        assert!(RepoStatus::refs(&inside).unwrap().is_none());
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

    #[test]
    fn each_stopped_operation_is_named() {
        let dir = conflicting();
        let dir = dir.path();
        assert_eq!(operation(dir), None);
        for (args, stopped, abort) in [
            (&["merge", "side"][..], Operation::Merge, "merge"),
            (
                &["cherry-pick", "side"],
                Operation::CherryPick,
                "cherry-pick",
            ),
            // undoing the commit that made `f`, which `main` changed since
            (
                &["revert", "--no-edit", "HEAD~"],
                Operation::Revert,
                "revert",
            ),
        ] {
            assert!(as_t(dir, args).is_err(), "{args:?} stopped");
            assert_eq!(operation(dir), Some(stopped), "{args:?}");
            as_t(dir, &[abort, "--abort"]).unwrap();
        }

        let dir = conflicting();
        let dir = dir.path();
        run(dir, ["switch", "-q", "side"]).unwrap();
        assert!(as_t(dir, &["rebase", "main"]).is_err());
        assert_eq!(RepoStatus::read(dir).unwrap().branch, None);
        assert_eq!(
            operation(dir),
            Some(Operation::Rebase {
                branch: Some("side".to_string())
            })
        );
        as_t(dir, &["rebase", "--abort"]).unwrap();

        let patch = run(dir, ["format-patch", "-1", "--stdout", "side"]).unwrap();
        std::fs::write(dir.join("side.patch"), patch + "\n").unwrap();
        run(dir, ["switch", "-q", "main"]).unwrap();
        assert!(as_t(dir, &["am", "side.patch"]).is_err());
        assert_eq!(operation(dir), Some(Operation::Am));
        as_t(dir, &["am", "--abort"]).unwrap();

        run(dir, ["bisect", "start"]).unwrap();
        assert_eq!(operation(dir), Some(Operation::Bisect));
        run(dir, ["bisect", "reset"]).unwrap();
        assert_eq!(operation(dir), None);
        // a branch or tag named like git's state is no operation, and
        // hides none
        run(dir, ["tag", "MERGE_HEAD", "side"]).unwrap();
        run(dir, ["branch", "REVERT_HEAD"]).unwrap();
        assert_eq!(operation(dir), None);
        assert!(as_t(dir, &["cherry-pick", "side"]).is_err());
        assert_eq!(operation(dir), Some(Operation::CherryPick));
    }

    #[test]
    fn a_cherry_pick_sequence_stays_named_after_its_stopped_pick_is_committed() {
        let dir = conflicting();
        let dir = dir.path();
        run(dir, ["switch", "-q", "side"]).unwrap();
        commit(dir, "g");
        run(dir, ["switch", "-q", "main"]).unwrap();
        assert!(as_t(dir, &["cherry-pick", "main..side"]).is_err());
        std::fs::write(dir.join("f"), "resolved").unwrap();
        run(dir, ["add", "f"]).unwrap();
        // committing by hand ends the pick, not the sequence
        as_t(dir, &["commit", "-qm", "resolved"]).unwrap();
        assert_eq!(operation(dir), Some(Operation::CherryPick));
    }

    #[test]
    fn a_stopped_cherry_pick_is_found_in_a_reftable_repository() {
        let dir = tempfile::tempdir().unwrap();
        // git before 2.45 has no reftable: nothing to check there
        if run(
            dir.path(),
            ["init", "-q", "-b", "main", "--ref-format=reftable"],
        )
        .is_err()
        {
            return;
        }
        make_conflicting(dir.path());
        assert!(as_t(dir.path(), &["cherry-pick", "side"]).is_err());
        assert!(!dir.path().join(".git/CHERRY_PICK_HEAD").exists());
        assert_eq!(operation(dir.path()), Some(Operation::CherryPick));
    }

    #[test]
    fn an_operation_belongs_to_the_worktree_it_stopped_in() {
        let dir = conflicting();
        let linked = dir.path().join("linked");
        run(
            dir.path(),
            ["worktree", "add", "-q", "-b", "other", "linked"],
        )
        .unwrap();
        assert!(as_t(&linked, &["merge", "side"]).is_err());
        assert_eq!(operation(&linked), Some(Operation::Merge));
        assert_eq!(operation(dir.path()), None);
    }

    #[test]
    fn an_upstream_is_gone_only_when_its_branch_is() {
        let upstream = repo();
        commit(upstream.path(), "a");
        run(upstream.path(), ["branch", "topic"]).unwrap();
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
        let read = || RepoStatus::read(&clone).unwrap();
        assert!(!read().upstream_gone());
        run(&clone, ["switch", "-q", "topic"]).unwrap();
        run(upstream.path(), ["branch", "-qD", "topic"]).unwrap();
        run(&clone, ["fetch", "-q", "--prune"]).unwrap();
        let status = read();
        assert_eq!(status.upstream.as_deref(), Some("origin/topic"));
        assert!(status.upstream_gone());
        // a branch with no commit yet has no counts either, and its
        // upstream is there
        run(&clone, ["switch", "-q", "--orphan", "fresh"]).unwrap();
        run(&clone, ["config", "branch.fresh.remote", "origin"]).unwrap();
        run(&clone, ["config", "branch.fresh.merge", "refs/heads/main"]).unwrap();
        let status = read();
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert!(!status.upstream_gone());
    }
}
