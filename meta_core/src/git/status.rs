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
    /// `git status` reads it (one more git process, three in a reftable
    /// repository, so only callers that show it ask). git names the files
    /// in the checkout's own git directory (a linked worktree's, never the
    /// one it shares); the refs git keeps for an operation are read by
    /// their exact names, never resolved as revisions, which a branch or
    /// tag of the same name would answer. A rebase comes first: the picks
    /// and merges it stops on are its own.
    pub fn read(dir: &Path) -> anyhow::Result<Option<Self>> {
        // git before 2.45 knows no `--show-ref-format`, echoes it, and has
        // only the files backend
        let out = super::run(
            dir,
            [
                "rev-parse",
                "--show-ref-format",
                "--git-path",
                "rebase-merge",
                "--git-path",
                "rebase-apply",
                "--git-path",
                "sequencer/todo",
                "--git-path",
                "BISECT_LOG",
                "--git-path",
                "MERGE_HEAD",
                "--git-path",
                "CHERRY_PICK_HEAD",
                "--git-path",
                "REVERT_HEAD",
            ],
        )?;
        let lines: Vec<&str> = out.lines().collect();
        let &[format, rebase_merge, rebase_apply, todo, bisect, merge_head, cherry_pick_head, revert_head] =
            lines.as_slice()
        else {
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
        // MERGE_HEAD is a file in every ref store; the others are in a
        // reftable one when the repository has one
        let files = matches!(format, "files" | "--show-ref-format");
        let head = |name: &str, path: &str| {
            if files || name == "MERGE_HEAD" {
                Ok(exists(&dir.join(path)))
            } else {
                ref_exists(dir, name)
            }
        };
        for (name, path, operation) in [
            ("MERGE_HEAD", merge_head, Self::Merge),
            ("CHERRY_PICK_HEAD", cherry_pick_head, Self::CherryPick),
            ("REVERT_HEAD", revert_head, Self::Revert),
        ] {
            if head(name, path)? {
                return Ok(Some(operation));
            }
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
        Ok(sequencer().or_else(|| exists(&dir.join(bisect)).then_some(Self::Bisect)))
    }
}

/// Whether the ref `name` (a full name, such as `CHERRY_PICK_HEAD`) is
/// there: `git show-ref --exists`, which exits 2 for one that isn't.
fn ref_exists(dir: &Path, name: &str) -> anyhow::Result<bool> {
    let output = crate::children::output(super::git(dir).args(["show-ref", "--exists", name]))
        .map_err(|e| {
            anyhow::anyhow!(
                "Cannot run git show-ref in {}: {e}",
                crate::display::escape_path(dir)
            )
        })?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(2) => Ok(false),
        _ => anyhow::bail!(
            "git show-ref --exists {name} failed in {}:{}",
            crate::display::escape_path(dir),
            crate::display::program_output(&output.stderr)
        ),
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
    use meta_test_support::{path, Sandbox};

    fn commit(sandbox: &Sandbox, dir: &Path, file: &str) {
        commit_text(sandbox, dir, file, file);
    }

    fn commit_text(sandbox: &Sandbox, dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        sandbox.git(dir, &["add", file]);
        sandbox.git(dir, &["commit", "-qm", text]);
    }

    /// `main` and `side` each change `f` from `base`, so bringing one onto
    /// the other stops on a conflict
    fn make_conflicting(sandbox: &Sandbox, dir: &Path) {
        commit_text(sandbox, dir, "f", "base");
        sandbox.git(dir, &["switch", "-qc", "side"]);
        commit_text(sandbox, dir, "f", "side");
        sandbox.git(dir, &["switch", "-q", "main"]);
        commit_text(sandbox, dir, "f", "main");
    }

    fn operation(dir: &Path) -> Option<Operation> {
        Operation::read(dir).unwrap()
    }

    #[test]
    fn reads_branch_changes_and_upstream() {
        let Some(sandbox) = Sandbox::isolated() else {
            return;
        };
        let upstream = sandbox.repo(&sandbox.work().join("upstream"));
        commit(&sandbox, &upstream, "a");
        let dir = sandbox.work();
        let clone = dir.join("clone");
        sandbox.git(
            dir.as_path(),
            &["clone", "-q", &path(&upstream), &path(&clone)],
        );
        commit(&sandbox, &clone, "b");
        commit(&sandbox, &upstream, "c");
        sandbox.git(&clone, &["fetch", "-q"]);
        sandbox.git(&clone, &["config", "status.aheadBehind", "false"]);
        std::fs::write(clone.join("a"), "changed").unwrap();
        std::fs::write(clone.join("new file"), "").unwrap();
        sandbox.git(&clone, &["mv", "b", "b renamed"]);

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
        let Some(sandbox) = Sandbox::isolated() else {
            return;
        };
        let dir = sandbox.repo(&sandbox.work().join("repo"));
        commit(&sandbox, dir.as_path(), "a");
        let status = |dir: &Path| RepoStatus::read(dir).unwrap();
        let kinds = |s: &RepoStatus| {
            (
                s.has_staged(),
                s.has_unstaged(),
                s.has_untracked(),
                s.is_dirty(),
            )
        };

        std::fs::write(dir.join("new"), "").unwrap();
        // what a config hides is still there
        sandbox.git(
            dir.as_path(),
            &["config", "status.showUntrackedFiles", "no"],
        );
        sandbox.git(dir.as_path(), &["config", "status.aheadBehind", "false"]);
        assert_eq!(kinds(&status(dir.as_path())), (false, false, true, true));
        std::fs::remove_file(dir.join("new")).unwrap();

        std::fs::write(dir.join("a"), "changed").unwrap();
        assert_eq!(kinds(&status(dir.as_path())), (false, true, false, true));
        sandbox.git(dir.as_path(), &["add", "a"]);
        assert_eq!(kinds(&status(dir.as_path())), (true, false, false, true));
    }

    #[test]
    fn a_bare_repository_has_a_branch_commit_and_upstream() {
        let Some(sandbox) = Sandbox::isolated() else {
            return;
        };
        let upstream = sandbox.repo(&sandbox.work().join("upstream"));
        commit(&sandbox, &upstream, "a");
        let dir = sandbox.work();
        let bare = dir.join("bare.git");
        sandbox.git(
            dir.as_path(),
            &["clone", "-q", "--bare", &path(&upstream), &path(&bare)],
        );
        for args in [
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/remotes/origin/*",
            ][..],
            &["config", "branch.main.remote", "origin"],
            &["config", "branch.main.merge", "refs/heads/main"],
        ] {
            sandbox.git(&bare, args);
        }
        commit(&sandbox, &upstream, "b");
        sandbox.git(&bare, &["fetch", "-q"]);

        let status = RepoStatus::read(&bare).unwrap();
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert!(status.oid.is_some());
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!(status.ahead_behind, Some((0, 1)));
        assert!(!status.is_dirty());
        // an upstream that isn't there is named, and gone
        sandbox.git(&bare, &["config", "branch.main.merge", "refs/heads/gone"]);
        let status = RepoStatus::read(&bare).unwrap();
        assert_eq!(status.upstream.as_deref(), Some("origin/gone"));
        assert!(status.upstream_gone());
        // what isn't a repository is still an error, even inside one
        assert!(RepoStatus::read(dir.as_path()).is_err());
        let inside = upstream.join("not-cloned");
        std::fs::create_dir(&inside).unwrap();
        assert!(RepoStatus::read(&inside).is_err());
        // and so is a checkout whose config says it is bare: it may hold
        // changes git can't report
        let checkout = sandbox.repo(&sandbox.work().join("checkout"));
        commit(&sandbox, &checkout, "a");
        std::fs::write(checkout.join("dirty"), "").unwrap();
        sandbox.git(&checkout, &["config", "core.bare", "true"]);
        assert!(RepoStatus::read(&checkout).is_err());
        // whose refs can still be shown
        let refs = RepoStatus::refs(&checkout).unwrap().unwrap();
        assert_eq!(refs.branch.as_deref(), Some("main"));
        assert!(RepoStatus::refs(&inside).unwrap().is_none());
    }

    #[test]
    fn a_new_repo_has_no_commit_or_upstream() {
        let Some(sandbox) = Sandbox::isolated() else {
            return;
        };
        let dir = sandbox.work();
        sandbox.git(&dir, &["init", "-q", "-b", "main"]);
        let status = RepoStatus::read(dir.as_path()).unwrap();
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!((&status.oid, status.ahead_behind), (&None, None));
        assert!(!status.is_dirty());
    }

    #[test]
    fn detached_head_has_no_branch() {
        let Some(sandbox) = Sandbox::isolated() else {
            return;
        };
        let dir = sandbox.repo(&sandbox.work().join("repo"));
        commit(&sandbox, dir.as_path(), "a");
        sandbox.git(dir.as_path(), &["checkout", "-q", "--detach"]);
        assert_eq!(RepoStatus::read(dir.as_path()).unwrap().branch, None);
    }

    #[test]
    fn each_stopped_operation_is_named() {
        let Some(sandbox) = Sandbox::isolated() else {
            return;
        };
        let dir = sandbox.repo(&sandbox.work().join("main-conflict"));
        make_conflicting(&sandbox, &dir);
        let dir = dir.as_path();
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
            assert!(
                !sandbox
                    .command("git")
                    .current_dir(dir)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success(),
                "{args:?} stopped"
            );
            assert_eq!(operation(dir), Some(stopped), "{args:?}");
            sandbox.git(dir, &[abort, "--abort"]);
        }

        let dir = sandbox.repo(&sandbox.work().join("side-conflict"));
        make_conflicting(&sandbox, &dir);
        let dir = dir.as_path();
        sandbox.git(dir, &["switch", "-q", "side"]);
        assert!(!sandbox
            .command("git")
            .current_dir(dir)
            .args(["rebase", "main"])
            .output()
            .unwrap()
            .status
            .success());
        assert_eq!(RepoStatus::read(dir).unwrap().branch, None);
        assert_eq!(
            operation(dir),
            Some(Operation::Rebase {
                branch: Some("side".to_string())
            })
        );
        sandbox.git(dir, &["rebase", "--abort"]);

        let patch = sandbox.git(dir, &["format-patch", "-1", "--stdout", "side"]);
        std::fs::write(dir.join("side.patch"), patch + "\n").unwrap();
        sandbox.git(dir, &["switch", "-q", "main"]);
        assert!(!sandbox
            .command("git")
            .current_dir(dir)
            .args(["am", "side.patch"])
            .output()
            .unwrap()
            .status
            .success());
        assert_eq!(operation(dir), Some(Operation::Am));
        sandbox.git(dir, &["am", "--abort"]);

        sandbox.git(dir, &["bisect", "start"]);
        assert_eq!(operation(dir), Some(Operation::Bisect));
        sandbox.git(dir, &["bisect", "reset"]);
        assert_eq!(operation(dir), None);
        // a branch or tag named like git's state is no operation, and
        // hides none
        sandbox.git(dir, &["tag", "MERGE_HEAD", "side"]);
        sandbox.git(dir, &["branch", "REVERT_HEAD"]);
        assert_eq!(operation(dir), None);
        assert!(!sandbox
            .command("git")
            .current_dir(dir)
            .args(["cherry-pick", "side"])
            .output()
            .unwrap()
            .status
            .success());
        assert_eq!(operation(dir), Some(Operation::CherryPick));
        sandbox.git(dir, &["cherry-pick", "--abort"]);
        assert!(!sandbox
            .command("git")
            .current_dir(dir)
            .args(["merge", "side"])
            .output()
            .unwrap()
            .status
            .success());
        assert_eq!(operation(dir), Some(Operation::Merge));
    }

    #[test]
    fn a_cherry_pick_sequence_stays_named_after_its_stopped_pick_is_committed() {
        let Some(sandbox) = Sandbox::isolated() else {
            return;
        };
        let dir = sandbox.repo(&sandbox.work().join("repo"));
        make_conflicting(&sandbox, &dir);
        let dir = dir.as_path();
        sandbox.git(dir, &["switch", "-q", "side"]);
        commit(&sandbox, dir, "g");
        sandbox.git(dir, &["switch", "-q", "main"]);
        assert!(!sandbox
            .command("git")
            .current_dir(dir)
            .args(["cherry-pick", "main..side"])
            .output()
            .unwrap()
            .status
            .success());
        std::fs::write(dir.join("f"), "resolved").unwrap();
        sandbox.git(dir, &["add", "f"]);
        // committing by hand ends the pick, not the sequence
        sandbox.git(dir, &["commit", "-qm", "resolved"]);
        assert_eq!(operation(dir), Some(Operation::CherryPick));
    }

    #[test]
    fn a_stopped_cherry_pick_is_found_in_a_reftable_repository() {
        let Some(sandbox) = Sandbox::isolated() else {
            return;
        };
        let dir = sandbox.work();
        if !meta_test_support::git_has_reftable() {
            return;
        }
        sandbox.git(
            dir.as_path(),
            &["init", "-q", "-b", "main", "--ref-format=reftable"],
        );
        make_conflicting(&sandbox, dir.as_path());
        assert!(!sandbox
            .command("git")
            .current_dir(dir.as_path())
            .args(["cherry-pick", "side"])
            .output()
            .unwrap()
            .status
            .success());
        assert!(!dir.join(".git/CHERRY_PICK_HEAD").exists());
        // read by its name, never as a revision a tag also answers
        sandbox.git(dir.as_path(), &["tag", "REVERT_HEAD"]);
        assert_eq!(operation(dir.as_path()), Some(Operation::CherryPick));
        sandbox.git(dir.as_path(), &["cherry-pick", "--abort"]);
        assert_eq!(operation(dir.as_path()), None);
    }

    #[test]
    fn an_operation_belongs_to_the_worktree_it_stopped_in() {
        let Some(sandbox) = Sandbox::isolated() else {
            return;
        };
        let dir = sandbox.repo(&sandbox.work().join("repo"));
        make_conflicting(&sandbox, &dir);
        let linked = dir.join("linked");
        sandbox.git(
            dir.as_path(),
            &["worktree", "add", "-q", "-b", "other", "linked"],
        );
        assert!(!sandbox
            .command("git")
            .current_dir(&linked)
            .args(["merge", "side"])
            .output()
            .unwrap()
            .status
            .success());
        assert_eq!(operation(&linked), Some(Operation::Merge));
        assert_eq!(operation(dir.as_path()), None);
    }

    #[test]
    fn an_upstream_is_gone_only_when_its_branch_is() {
        let Some(sandbox) = Sandbox::isolated() else {
            return;
        };
        let upstream = sandbox.repo(&sandbox.work().join("upstream"));
        commit(&sandbox, &upstream, "a");
        sandbox.git(&upstream, &["branch", "topic"]);
        let dir = sandbox.work();
        let clone = dir.join("clone");
        sandbox.git(
            dir.as_path(),
            &["clone", "-q", &path(&upstream), &path(&clone)],
        );
        let read = || RepoStatus::read(&clone).unwrap();
        assert!(!read().upstream_gone());
        sandbox.git(&clone, &["switch", "-q", "topic"]);
        sandbox.git(&upstream, &["branch", "-qD", "topic"]);
        sandbox.git(&clone, &["fetch", "-q", "--prune"]);
        let status = read();
        assert_eq!(status.upstream.as_deref(), Some("origin/topic"));
        assert!(status.upstream_gone());
        // a branch with no commit yet has no counts either, and its
        // upstream is there
        sandbox.git(&clone, &["switch", "-q", "--orphan", "fresh"]);
        sandbox.git(&clone, &["config", "branch.fresh.remote", "origin"]);
        sandbox.git(&clone, &["config", "branch.fresh.merge", "refs/heads/main"]);
        let status = read();
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert!(!status.upstream_gone());
    }
}
