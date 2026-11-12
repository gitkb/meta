//! Git operations for worktree management.

use anyhow::{Context, Result};
use meta_core::git::{lookup, run, RepoStatus};
use meta_core::gitdir::{common_dir, Checkout};
use meta_plugin::display::{escape, escape_path};
use meta_plugin::errln;
use std::ffi::{OsStr, OsString};
use std::path::Path;

/// A git command that failed in a repo: its error says what failed where,
/// with git's stderr, as any of meta's git errors does; a command's answer
/// gives the repo a row of its own with that stderr (see [`failure_rows`]).
#[derive(Debug)]
pub struct GitFailed {
    /// The repo, as its project is checked out
    pub repo: std::path::PathBuf,
    /// The command, as shown
    pub command: String,
    /// The folder git ran in
    pub dir: std::path::PathBuf,
    pub stderr: String,
}

impl std::fmt::Display for GitFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "git {} failed in {}:{}",
            escape(&self.command),
            escape_path(&self.dir),
            meta_plugin::display::program_output(self.stderr.as_bytes())
        )
    }
}

impl std::error::Error for GitFailed {}

/// `git <args>` in `dir`, a command that changes the repo of `repo`: when
/// it fails, a [`GitFailed`] (its stderr kept, bounded as every captured
/// output is).
pub(super) fn change<I, S>(repo: &Path, dir: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<S> = args.into_iter().collect();
    let command = args
        .iter()
        .map(|a| a.as_ref().to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let output =
        meta_plugin::process::output(meta_core::git::git(dir).args(&args)).map_err(|e| {
            anyhow::anyhow!(
                "Cannot run git {} in {}: {e}",
                escape(&command),
                escape_path(dir)
            )
        })?;
    if !output.status.success() {
        return Err(GitFailed {
            repo: repo.to_path_buf(),
            command,
            dir: dir.to_path_buf(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
        .into());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end_matches(['\n', '\r'])
        .to_string())
}

/// A row for each git command that failed in `error`'s chain, named by its
/// repo's path from `root`, with git's stderr: as `meta --json exec` gives a
/// failed step's.
pub fn failure_rows(error: &anyhow::Error, root: &Path) -> Vec<meta_plugin::ProjectResult> {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<GitFailed>())
        .map(|failed| {
            let project = match failed.repo.strip_prefix(root) {
                Ok(inside) if inside.as_os_str().is_empty() => ".".to_string(),
                Ok(inside) => inside.to_string_lossy().into_owned(),
                Err(_) => failed.repo.to_string_lossy().into_owned(),
            };
            let last = failed
                .stderr
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("no stderr");
            meta_plugin::ProjectResult {
                stderr: Some(failed.stderr.clone()),
                ..meta_plugin::ProjectResult::error(
                    project,
                    meta_plugin::ErrorInfo::new(
                        meta_plugin::ErrorCode::ToolFailed,
                        format!("git {} failed: {}", escape(&failed.command), escape(last)),
                    ),
                )
                .at(&failed.repo)
            }
        })
        .collect()
}

/// An error if the checkout at `repo_path` has `core.bare=true` in its
/// config: git would make no worktree of it.
pub fn ensure_checkout_config_is_not_bare(repo_path: &Path) -> Result<()> {
    let checkout = Checkout::of(repo_path);
    if let Some(problem) = checkout.problem() {
        anyhow::bail!("{}: {problem}", escape_path(repo_path));
    }
    // `config --get` exits 1 when core.bare isn't configured.
    if lookup(
        repo_path,
        ["config", "--local", "--bool", "--get", "core.bare"],
    )?
    .as_deref()
        == Some("true")
    {
        if let Checkout::Repo { git_dir } = checkout {
            if repo_path.canonicalize()? == git_dir.canonicalize()? {
                return Ok(()); // The bare repository itself has no checkout.
            }
            anyhow::bail!(
                "Repository '{}' is a checked-out work tree but its shared Git config has \
                 core.bare=true.\nRepair it with:\n  git -C {} config --local core.bare false",
                escape_path(repo_path),
                escape(&meta_plugin::display::shell_quote(
                    &repo_path.to_string_lossy()
                ))
            );
        }
    }

    Ok(())
}

/// Where `git worktree add` starts a worktree of a repo on a branch.
pub enum WorktreeStart {
    /// A new branch at the commit a commit-ish names (the name is passed
    /// to git, so a remote-tracking ref still sets the upstream)
    Ref { name: String, commit: String },
    /// The existing local branch
    Existing,
    /// A new branch tracking `origin/<branch>`, at this commit
    Remote { commit: String },
    /// A new branch, tracking nothing, at the project's own branch
    /// (`branch:`) or else the remote's default branch (`origin/HEAD`),
    /// named here as `origin/main`
    Default { name: String, commit: String },
    /// A new branch at HEAD's commit, in a repo whose remote names no
    /// default branch; `branch`: the one HEAD is on, if any
    Head {
        commit: String,
        branch: Option<String>,
    },
}

impl WorktreeStart {
    /// The commit a new branch is made at; none for an existing branch.
    pub fn new_branch_at(self) -> Option<String> {
        match self {
            Self::Ref { commit, .. }
            | Self::Remote { commit }
            | Self::Default { commit, .. }
            | Self::Head { commit, .. } => Some(commit),
            Self::Existing => None,
        }
    }

    /// Whether it makes a new branch.
    pub fn is_new_branch(&self) -> bool {
        !matches!(self, Self::Existing)
    }

    /// What a new branch starts from, as said to the user (`origin/main`,
    /// `HEAD (topic)`); none for an existing branch.
    pub fn base(&self, branch: &str) -> Option<String> {
        match self {
            Self::Ref { name, .. } | Self::Default { name, .. } => Some(name.clone()),
            Self::Remote { .. } => Some(format!("origin/{branch}")),
            Self::Head {
                branch: Some(on), ..
            } => Some(format!("HEAD ({on})")),
            Self::Head { branch: None, .. } => Some("HEAD".to_string()),
            Self::Existing => None,
        }
    }
}

/// How a worktree of `repo_path` on `branch` would start, from `from_ref`
/// if given: every check meta makes before running `git worktree add`, so
/// a preview makes them too. A new branch without `from_ref` starts from
/// the project's own branch (`start` reads its `branch:` in .meta.yaml,
/// only then, so a bad one stops only a branch that would start there) if
/// it names one, else from the remote's default branch, never from whatever
/// the checkout has checked out; only a repo whose remote names none (no
/// `origin/HEAD`) starts from its HEAD, with a warning when it has a remote.
pub fn worktree_start(
    repo_path: &Path,
    branch: &str,
    from_ref: Option<&str>,
    start: &dyn Fn() -> Result<Option<String>>,
) -> Result<WorktreeStart> {
    ensure_checkout_config_is_not_bare(repo_path)?;
    if let Some(ref_name) = from_ref {
        // a commit-ish starts a new branch: git would refuse one that exists
        if git_rev_parse(repo_path, &format!("refs/heads/{branch}"))?.is_some() {
            anyhow::bail!(
                "Branch '{}' already exists in {}, and a commit-ish ('{}') starts a new branch. \
                 Drop the commit-ish to check out the existing branch, or name a new branch \
                 with --branch <name> (or --repo <repo>:<name>).",
                escape(branch),
                escape_path(repo_path),
                escape(ref_name)
            );
        }
        let commit =
            git_rev_parse(repo_path, &format!("{ref_name}^{{commit}}"))?.ok_or_else(|| {
                anyhow::anyhow!(
                    "Ref '{}' not found in repo '{}'",
                    ref_name,
                    escape_path(repo_path)
                )
            })?;
        return Ok(WorktreeStart::Ref {
            name: ref_name.to_string(),
            commit,
        });
    }
    if git_rev_parse(repo_path, &format!("refs/heads/{branch}"))?.is_some() {
        worktree_branch_is_free(repo_path, branch)?;
        return Ok(WorktreeStart::Existing);
    }
    if let Some(commit) = git_rev_parse(repo_path, &format!("refs/remotes/origin/{branch}"))? {
        return Ok(WorktreeStart::Remote { commit });
    }
    if let Some(start) = start()? {
        let start = start.as_str();
        let name = format!("origin/{start}");
        let commit = git_rev_parse(repo_path, &format!("refs/remotes/{name}^{{commit}}"))?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}: its .meta.yaml names branch '{}', and there is no {} to start '{}' \
                     from; fetch it with: git -C {} fetch origin {}",
                    escape_path(repo_path),
                    escape(start),
                    escape(&name),
                    escape(branch),
                    escape(&meta_plugin::display::shell_quote(
                        &repo_path.to_string_lossy()
                    )),
                    // named in full: a single-branch or shallow clone's
                    // fetch refspec doesn't cover it
                    escape(&meta_plugin::display::shell_quote(&format!(
                        "+refs/heads/{start}:refs/remotes/origin/{start}"
                    )))
                )
            })?;
        return Ok(WorktreeStart::Default { name, commit });
    }
    let default = lookup(
        repo_path,
        [
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )?;
    if let Some(name) = default {
        let commit = git_rev_parse(repo_path, "refs/remotes/origin/HEAD^{commit}")?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}: origin/HEAD names {}, which doesn't exist; run: git -C {} remote set-head origin --auto",
                    escape_path(repo_path),
                    escape(&name),
                    escape(&meta_plugin::display::shell_quote(&repo_path.to_string_lossy()))
                )
            })?;
        return Ok(WorktreeStart::Default { name, commit });
    }
    let commit = git_rev_parse(repo_path, "HEAD")?.ok_or_else(|| {
        anyhow::anyhow!("{} has no commit to branch from", escape_path(repo_path))
    })?;
    let on = meta_core::git::current_branch(repo_path)?;
    if lookup(repo_path, ["config", "--get", "remote.origin.url"])?.is_some() {
        meta_plugin::out::warn(format!(
            "{}: origin names no default branch (origin/HEAD), so '{}' starts from HEAD{}; \
             give the commit-ish to start from, or set it with: git -C {} remote set-head origin --auto",
            escape_path(repo_path),
            escape(branch),
            on.as_deref()
                .map(|on| format!(" ({})", escape(on)))
                .unwrap_or_default(),
            escape(&meta_plugin::display::shell_quote(&repo_path.to_string_lossy()))
        ));
    }
    Ok(WorktreeStart::Head { commit, branch: on })
}

/// Make a worktree of `repo_path` at `worktree_dest` on `branch` (see
/// [`worktree_start`]). Where it started: a new branch's commit (`-b`
/// refuses an existing branch, so success proves this call made it), or
/// [`WorktreeStart::Existing`].
pub fn git_worktree_add(
    repo_path: &Path,
    worktree_dest: &Path,
    branch: &str,
    from_ref: Option<&str>,
    start_branch: &dyn Fn() -> Result<Option<String>>,
) -> Result<WorktreeStart> {
    let start = worktree_start(repo_path, branch, from_ref, start_branch)?;
    leftover_entry(repo_path, worktree_dest)?;
    change(
        repo_path,
        repo_path,
        worktree_add_args(&start, worktree_dest, branch),
    )?;
    Ok(start)
}

/// The arguments of the `git worktree add` that starts a worktree at `dest`
/// on `branch` as `start` says.
pub fn worktree_add_args(start: &WorktreeStart, dest: &Path, branch: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["worktree".into(), "add".into()];
    match start {
        WorktreeStart::Remote { .. } => args.push("--track".into()),
        // a branch of its own: pushing or pulling it must not reach main
        WorktreeStart::Default { .. } => args.push("--no-track".into()),
        _ => {}
    }
    if !matches!(start, WorktreeStart::Existing) {
        args.extend(["-b".into(), branch.into()]);
    }
    args.push(dest.into());
    match start {
        WorktreeStart::Ref { name, .. } | WorktreeStart::Default { name, .. } => {
            args.push(name.into());
        }
        WorktreeStart::Existing => args.push(branch.into()),
        WorktreeStart::Remote { .. } => args.push(format!("origin/{branch}").into()),
        WorktreeStart::Head { .. } => {}
    }
    args
}

/// Fail, saying what to do, if git still has an entry for a worktree of
/// `repo_path` at `dest` whose folder is gone (deleted by hand): git refuses
/// to add one there. meta never drops git's entries itself: one can hold
/// what its folder didn't (staged changes, a rebase or merge under way,
/// commits only its reflog knows), and a worktree that was moved can still
/// be repaired.
fn leftover_entry(repo_path: &Path, dest: &Path) -> Result<()> {
    let Some(git_dir) = meta_core::gitdir::common_dir(repo_path)? else {
        return Ok(());
    };
    // the set's folder exists by now; compare where it resolves
    let resolved = |path: &Path| match (path.parent().map(Path::canonicalize), path.file_name()) {
        (Some(Ok(parent)), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    };
    let dest = resolved(dest);
    let entries = match std::fs::read_dir(git_dir.join("worktrees")) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let admin = entry?.path();
        let Some(checkout) = meta_core::set_record::checkout(&admin) else {
            continue;
        };
        if resolved(&checkout) == dest && meta_core::set_record::checkout_gone(&admin) {
            anyhow::bail!(
                "{} still has git's entry for a worktree at {}, whose folder is gone. git may \
                 hold work there that the folder didn't: staged changes, a rebase or merge \
                 under way, commits only its reflog knows (its git directory is {}). Once \
                 checked, drop it with: git -C {} worktree remove {}",
                escape_path(repo_path),
                escape_path(&dest),
                escape_path(&admin),
                // quoted for a shell, then escaped for the terminal
                escape(&meta_plugin::display::shell_quote(
                    &repo_path.to_string_lossy()
                )),
                escape(&meta_plugin::display::shell_quote(&dest.to_string_lossy()))
            );
        }
    }
    Ok(())
}

pub fn git_worktree_remove(repo_path: &Path, worktree_path: &Path, force: bool) -> Result<()> {
    let mut args: Vec<&OsStr> = vec!["worktree".as_ref(), "remove".as_ref()];
    if force {
        args.push("--force".as_ref());
    }
    args.push(worktree_path.as_os_str());
    // the repo: its checkout, above its git directory
    let repo = match repo_path.file_name() {
        Some(name) if name == ".git" => repo_path.parent().unwrap_or(repo_path),
        _ => repo_path,
    };
    change(repo, repo_path, args)?;
    Ok(())
}

/// The status of the checkout at `repo_path`, refusing one whose shared
/// config says it is bare (git would misreport it).
pub fn repo_status(repo_path: &Path) -> Result<RepoStatus> {
    ensure_checkout_config_is_not_bare(repo_path)?;
    RepoStatus::read(repo_path)
}

pub fn git_diff_stat(
    worktree_path: &Path,
    base_ref: &str,
) -> Result<(usize, usize, usize, Vec<String>)> {
    let range = diff_range(worktree_path, base_ref)?;
    let numstat_text = run(worktree_path, ["diff", "--numstat", &range, "--"])?;

    let mut files_changed = 0;
    let mut insertions = 0usize;
    let mut deletions = 0usize;
    let mut files = Vec::new();
    let count = |value: &str| -> Result<usize> {
        match value {
            "-" => Ok(0), // Binary files have no line counts.
            _ => value.parse().with_context(|| {
                format!(
                    "Invalid git numstat count '{}' in {}",
                    escape(value),
                    escape_path(worktree_path)
                )
            }),
        }
    };

    for line in numstat_text.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let (Some(added), Some(removed), Some(file)) = (parts.next(), parts.next(), parts.next())
        else {
            anyhow::bail!(
                "Invalid git numstat in {}: {}",
                escape_path(worktree_path),
                escape(line)
            );
        };
        files_changed += 1;
        insertions = insertions
            .checked_add(count(added)?)
            .context("git numstat insertions overflow")?;
        deletions = deletions
            .checked_add(count(removed)?)
            .context("git numstat deletions overflow")?;
        files.push(file.to_string());
    }

    Ok((files_changed, insertions, deletions, files))
}

/// The patch [`git_diff_stat`] counts: git's own, whatever external diff or
/// text conversion the user set.
pub fn git_diff_patch(worktree_path: &Path, base_ref: &str) -> Result<String> {
    let range = diff_range(worktree_path, base_ref)?;
    run(
        worktree_path,
        [
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            &range,
            "--",
        ],
    )
}

/// What HEAD changed since it left `base_ref`: `merge-base` exits 1 only when
/// there is no common ancestor, and only then is it a two-dot diff; a bad
/// ref or a failed read is still an error.
fn diff_range(worktree_path: &Path, base_ref: &str) -> Result<String> {
    Ok(
        if lookup(worktree_path, ["merge-base", base_ref, "HEAD"])?.is_some() {
            format!("{base_ref}...HEAD")
        } else {
            format!("{base_ref}..HEAD")
        },
    )
}

/// Unregister and delete the checked worktrees of a set, deepest first
/// (nested checkouts before the checkouts that contain them).
/// Stops at the first failure, so the caller never deletes a set that git
/// still has registered (a locked worktree, say).
pub fn remove_worktree_repos(
    members: &[super::safety::Member],
    force: bool,
    verbose: bool,
) -> Result<()> {
    let mut ordered: Vec<_> = members.iter().collect();
    ordered.sort_by_key(|m| std::cmp::Reverse(m.path.components().count()));

    for m in ordered {
        if verbose {
            errln!("Removing worktree at {}", escape_path(&m.path));
        }
        git_worktree_remove(&m.repo_git_dir, &m.path, force)
            .map_err(|e| e.context(format!("Cannot remove {}", escape_path(&m.path))))?;
    }
    Ok(())
}

/// The commit `rev` names in `repo_path`, or `None` if git says no such name
/// exists. Any other failure (a broken repository, say) is an error with
/// git's message, never mistaken for a missing name.
pub fn git_rev_parse(repo_path: &Path, rev: &str) -> Result<Option<String>> {
    lookup(repo_path, ["rev-parse", "--verify", "--quiet", rev])
}

/// Fail if any worktree of the repository has `branch` checked out, or is
/// rebasing or bisecting it: the reservations git itself enforces. Checked-out
/// branches come from `git worktree list` (git resolves each HEAD through its
/// ref store, files or reftable); rebase and bisect state comes from each
/// worktree's state files (`rebase-merge/head-name`, `rebase-apply/head-name`,
/// `rebase-merge/update-refs`, `BISECT_START`). Changes nothing; anything it
/// can't read is an error, not "free".
pub fn git_branch_is_free(repo_path: &Path, branch: &str) -> Result<()> {
    branch_is_free(repo_path, branch, false)
}

/// The create/add branch check: the same reservations as branch deletion,
/// with a typed conflict naming the holding set and a branch-choice hint.
pub fn worktree_branch_is_free(repo_path: &Path, branch: &str) -> Result<()> {
    branch_is_free(repo_path, branch, true)
}

fn checkout_path_bytes(bytes: &[u8]) -> Result<std::path::PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(std::path::PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        Ok(std::path::PathBuf::from(
            std::str::from_utf8(bytes).context("Git checkout path is not UTF-8")?,
        ))
    }
}

// Git's backlink is raw path bytes followed by one line ending.
fn holder_checkout(admin: &Path) -> Result<Option<std::path::PathBuf>> {
    let file = admin.join("gitdir");
    let bytes = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("Cannot read {}", escape_path(&file))),
    };
    // Git rejects a zero-byte registration before trimming its path.
    if bytes.is_empty() {
        return Ok(None);
    }
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    Ok(admin
        .join(checkout_path_bytes(bytes)?)
        .parent()
        .map(Path::to_path_buf))
}

// Worktree conflicts preserve undecodable path bytes; other commands retain escape_path.
fn holder_path(path: &Path) -> String {
    match path.to_str() {
        Some(_) => escape_path(path),
        None => escape(&format!("{:?}", path.as_os_str())).into_owned(),
    }
}

fn branch_is_free(repo_path: &Path, branch: &str, describe_holder: bool) -> Result<()> {
    let full = format!("refs/heads/{branch}");
    let in_use = |by: &str| {
        if !describe_holder {
            return anyhow::anyhow!("'{}' is {by}", escape(branch));
        }
        anyhow::Error::from(
            meta_plugin::ErrorInfo::new(
                meta_plugin::ErrorCode::Conflict,
                format!("'{}' is {by}", escape(branch)),
            )
            .hint("Use another branch with --branch <name> (create) or --repo <repo>:<name>."),
        )
    };

    // A symbolic branch would carry a ref transaction to the branch it points
    // at, which this check never looked at.
    // `symbolic-ref -q` exits 1 for a plain or absent ref.
    if lookup(repo_path, ["symbolic-ref", "-q", &full])?.is_some() {
        return Err(in_use("a symbolic ref; meta only moves plain branches"));
    }

    let common = common_dir(repo_path)?.with_context(|| {
        format!(
            "Cannot find the git directory of {}",
            escape_path(repo_path)
        )
    })?;

    // Cleanup keeps its existing check and message. Create/add identify
    // the owner through Git's ref store and raw admin backlinks instead
    // of parsing porcelain paths (which were raw before Git 2.36).
    if !describe_holder {
        let listed = run(repo_path, ["worktree", "list", "--porcelain"])?;
        let wanted = format!("branch {full}");
        if listed.lines().any(|line| line == wanted) {
            return Err(in_use("checked out in a worktree"));
        }
    }

    let mut admin_dirs = vec![common.clone()];
    match std::fs::read_dir(common.join("worktrees")) {
        Ok(entries) => {
            for entry in entries {
                let admin = entry?.path();
                if describe_holder && holder_checkout(&admin)?.is_none() {
                    continue;
                }
                admin_dirs.push(admin);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }

    let primary = if describe_holder {
        let bare = lookup(
            repo_path,
            [
                std::ffi::OsStr::new("--git-dir"),
                common.as_os_str(),
                std::ffi::OsStr::new("rev-parse"),
                std::ffi::OsStr::new("--is-bare-repository"),
            ],
        )?;
        if bare.as_deref() == Some("true") {
            None
        } else {
            // Git's main-worktree registration is the real common dir,
            // with the trailing /.git removed (also for linked callers).
            let real = common.canonicalize()?;
            Some(if real.file_name() == Some(std::ffi::OsStr::new(".git")) {
                real.parent().unwrap_or(&real).to_path_buf()
            } else {
                real
            })
        }
    } else {
        None
    };
    if describe_holder {
        for admin in &admin_dirs {
            let path = if admin == &common {
                primary.clone()
            } else {
                holder_checkout(admin)?
            };
            // As Git does, ignore directories without a worktree registration.
            let Some(path) = path else { continue };
            let on = lookup(
                repo_path,
                [
                    std::ffi::OsStr::new("--git-dir"),
                    admin.as_os_str(),
                    std::ffi::OsStr::new("symbolic-ref"),
                    std::ffi::OsStr::new("-q"),
                    std::ffi::OsStr::new("HEAD"),
                ],
            )?;
            if on.as_deref() != Some(&full) {
                continue;
            }
            let record = meta_core::set_record::read(admin)?;
            let mut holder = match record {
                Some(record) => format!(
                    "held by worktree set '{}' at {} (checkout {})",
                    escape(&record.name()),
                    holder_path(&record.set),
                    holder_path(&path)
                ),
                None => format!("checked out in worktree {}", holder_path(&path)),
            };
            if let Err(leftover) = leftover_entry(repo_path, &path) {
                holder.push_str(&format!("; {leftover:#}"));
            }
            return Err(in_use(&holder));
        }
    }

    let read = |path: std::path::PathBuf| -> Result<String> {
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => {
                Err(anyhow::Error::from(e).context(format!("Cannot read {}", escape_path(&path))))
            }
        }
    };
    let reserved = |kind: &str, admin: &Path| -> Result<anyhow::Error> {
        let holder = if !describe_holder {
            format!("the worktree whose git dir is {}", escape_path(admin))
        } else if let Some(record) = meta_core::set_record::read(admin)? {
            format!(
                "worktree set '{}' at {}",
                escape(&record.name()),
                holder_path(&record.set)
            )
        } else {
            let path = if admin == common {
                primary.clone()
            } else {
                holder_checkout(admin)?
            }
            .unwrap_or_else(|| repo_path.to_path_buf());
            format!("worktree {}", holder_path(&path))
        };
        Ok(in_use(&format!("{kind} by {holder}")))
    };
    for dir in admin_dirs {
        let rebasing = [
            read(dir.join("rebase-merge/head-name"))?,
            read(dir.join("rebase-apply/head-name"))?,
        ];
        let updating = read(dir.join("rebase-merge/update-refs"))?;
        if rebasing.iter().any(|name| name.trim() == full)
            || updating.lines().any(|line| line.trim() == full)
        {
            return Err(reserved("being rebased", &dir)?);
        }
        let bisect = read(dir.join("BISECT_START"))?;
        if [branch, full.as_str()].contains(&bisect.trim()) {
            return Err(reserved("being bisected", &dir)?);
        }
    }
    Ok(())
}

/// Delete a branch, but only if git would let it go (see
/// [`git_branch_is_free`]) and it still points at `expected`, atomically
/// through git's ref transaction.
pub fn git_delete_branch_at(repo_path: &Path, branch: &str, expected: &str) -> Result<()> {
    git_branch_is_free(repo_path, branch)
        .map_err(|e| anyhow::anyhow!("Branch '{}' is in use; kept ({e})", escape(branch)))?;
    change(
        repo_path,
        repo_path,
        [
            "update-ref",
            "--no-deref",
            "-d",
            &format!("refs/heads/{branch}"),
            expected,
        ],
    )
    .with_context(|| {
        format!(
            "Branch '{}' no longer points where it was created; kept",
            escape(branch)
        )
    })?;
    // Only remove settings that exist: `--remove-section` treats an absent
    // section as fatal, so its exit status can't distinguish a failed write.
    let section = format!("branch.{branch}");
    if run(repo_path, ["config", "--local", "--name-only", "--list"])?
        .lines()
        .any(|key| {
            key.rsplit_once('.')
                .is_some_and(|(name, _)| name == section)
        })
    {
        run(
            repo_path,
            ["config", "--local", "--remove-section", &section],
        )?;
    }
    Ok(())
}

/// Fetch a branch from origin if not locally available.
pub fn git_fetch_branch(repo_path: &Path, branch: &str) -> Result<()> {
    change(repo_path, repo_path, ["fetch", "origin", branch])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_guard_rejects_missing_plain_and_broken_repositories() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.work();
        sandbox.git(&tmp, &["init", "-q", "-b", "main"]);
        let plain = tmp.join("plain");
        std::fs::create_dir(&plain).unwrap();
        for path in [&tmp.join("missing"), &plain] {
            let err = ensure_checkout_config_is_not_bare(path).unwrap_err();
            assert!(
                err.to_string().contains(&path.display().to_string()),
                "{err}"
            );
        }
        std::fs::write(plain.join(".git"), "gitdir: nowhere\n").unwrap();
        assert!(ensure_checkout_config_is_not_bare(&plain).is_err());
    }

    // macOS and Windows file systems refuse a name that isn't UTF-8
    #[cfg(target_os = "linux")]
    #[test]
    fn worktree_add_preserves_non_utf8_destination() {
        use std::os::unix::ffi::OsStrExt;

        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.repo(&sandbox.work());
        let dest = tmp.join(std::ffi::OsStr::from_bytes(b"wt-\xff"));
        git_worktree_add(&tmp, &dest, "topic", None, &|| Ok(None)).unwrap();
        assert!(dest.join(".git").is_file());
        assert!(!tmp.join("wt-\u{fffd}").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn worktree_remove_preserves_non_utf8_destination() {
        use std::os::unix::ffi::OsStrExt;

        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.repo(&sandbox.work());
        let dest = tmp.join(std::ffi::OsStr::from_bytes(b"wt-\xff"));
        let out = sandbox
            .command("git")
            .current_dir(&tmp)
            .args(["worktree", "add", "-b", "topic"])
            .arg(&dest)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        git_worktree_remove(&tmp.join(".git"), &dest, false).unwrap();
        assert!(!dest.exists());
    }

    #[test]
    fn delete_branch_reports_a_failed_config_cleanup() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.repo(&sandbox.work());
        let head = git_rev_parse(&tmp, "HEAD").unwrap().unwrap();
        sandbox.git(&tmp, &["branch", "t"]);
        sandbox.git(&tmp, &["config", "branch.t.remote", "origin"]);
        std::fs::write(tmp.join(".git/config.lock"), "locked").unwrap();

        let err = git_delete_branch_at(&tmp, "t", &head).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("git config --local --remove-section branch.t"),
            "{message}"
        );
        assert!(message.contains(&tmp.display().to_string()), "{message}");
        assert!(message.contains("lock"), "{message}");
    }

    #[test]
    fn diff_handles_unrelated_histories_but_rejects_a_bad_ref() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.repo(&sandbox.work());
        sandbox.git(&tmp, &["branch", "base"]);
        sandbox.git(&tmp, &["checkout", "-q", "--orphan", "other"]);
        sandbox.git(&tmp, &["rm", "-qf", "README"]);
        std::fs::write(tmp.join("other"), "other\n").unwrap();
        sandbox.git(&tmp, &["add", "other"]);
        sandbox.git(&tmp, &["commit", "-qm", "unrelated"]);

        let (files, added, deleted, _) = git_diff_stat(&tmp, "base").unwrap();
        assert_eq!((files, added, deleted), (2, 1, 1));
        let err = git_diff_stat(&tmp, "no-such-base").unwrap_err();
        assert!(err.to_string().contains("no-such-base"), "{err}");
    }

    #[test]
    fn a_failed_fetch_keeps_each_line_under_the_message() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.work();
        sandbox.git(&tmp, &["init", "-q", "-b", "main"]);
        sandbox.git(
            &tmp,
            &["remote", "add", "origin", "/nonexistent/r\u{1b}[2J\nFORGED"],
        );
        let err = git_fetch_branch(&tmp, "b\u{1b}[2J\nFORGED").unwrap_err();
        let message = format!("{err:#}");
        assert!(
            !message.contains('\u{1b}') && !message.lines().any(|l| l.starts_with("FORGED")),
            "{message}"
        );
        // the repo gets a row of its own with git's stderr, under any context
        let err = err.context("Cannot fetch the PR branch");
        let rows = failure_rows(&err, &tmp);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].project.as_deref(), Some("."));
        assert!(
            rows[0].stderr.as_deref().unwrap().contains("fatal"),
            "{rows:?}"
        );
        let said = &rows[0].error.as_ref().unwrap().message;
        assert!(!said.contains('\u{1b}') && !said.contains('\n'), "{said}");
    }

    /// The command a leftover git entry's message gives is quoted for a shell
    /// and escaped for the terminal: a path's control characters print as
    /// visible escapes. (Windows allows no control characters in file names.)
    #[cfg(unix)]
    #[test]
    fn a_leftover_entry_is_reported_with_its_paths_escaped() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.repo(&sandbox.work());
        let set = tmp.join("set\u{1b}[2J");
        std::fs::create_dir(&set).unwrap();
        let dest = set.join("wt");
        sandbox.git(
            &tmp,
            &["worktree", "add", "-q", "-b", "b", dest.to_str().unwrap()],
        );
        std::fs::remove_dir_all(&dest).unwrap();
        let err = leftover_entry(&tmp, &dest).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("worktree remove"), "{message}");
        assert!(!message.contains('\u{1b}'), "{message}");
    }

    // ── git_delete_branch_at ────────────────────────────────

    #[test]
    fn delete_branch_keeps_a_branch_that_moved() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.repo(&sandbox.work());
        let start = git_rev_parse(&tmp, "HEAD").unwrap().unwrap();
        sandbox.git(&tmp, &["branch", "t"]);
        std::fs::write(tmp.join("more"), "x").unwrap();
        sandbox.git(&tmp, &["add", "more"]);
        sandbox.git(&tmp, &["commit", "-q", "-m", "more"]);
        let moved = git_rev_parse(&tmp, "HEAD").unwrap().unwrap();
        sandbox.git(&tmp, &["config", "branch.t.foo.remote", "origin"]);
        sandbox.git(&tmp, &["update-ref", "refs/heads/t", &moved]);

        assert!(git_delete_branch_at(&tmp, "t", &start).is_err());
        assert_eq!(
            git_rev_parse(&tmp, "refs/heads/t").unwrap(),
            Some(moved.clone())
        );
        git_delete_branch_at(&tmp, "t", &moved).unwrap();
        assert_eq!(git_rev_parse(&tmp, "refs/heads/t").unwrap(), None);
        assert_eq!(
            sandbox.git(&tmp, &["config", "branch.t.foo.remote"]).trim(),
            "origin"
        );
    }

    #[test]
    fn unregistered_admin_head_does_not_reserve_a_branch() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.repo(&sandbox.work());
        sandbox.git(&tmp, &["branch", "reserved"]);
        let admin = tmp.join(".git/worktrees/orphan");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("HEAD"), "ref: refs/heads/reserved\n").unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        // Git ignores an admin directory with no gitdir registration.
        git_branch_is_free(&tmp, "reserved").unwrap();
        worktree_branch_is_free(&tmp, "reserved").unwrap();
    }

    #[test]
    fn review_empty_admin_backlink_does_not_reserve_a_branch() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.repo(&sandbox.work());
        sandbox.git(&tmp, &["branch", "reserved"]);
        let admin = tmp.join(".git/worktrees/orphan");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::write(admin.join("HEAD"), "ref: refs/heads/reserved\n").unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        std::fs::write(admin.join("gitdir"), "").unwrap();
        // Git ignores this admin directory; no branch is held there.
        git_branch_is_free(&tmp, "reserved").unwrap();
        worktree_branch_is_free(&tmp, "reserved").unwrap();
        // Git reads the registration before trimming, so these remain held.
        for backlink in ["\n", "\r\n"] {
            std::fs::write(admin.join("gitdir"), backlink).unwrap();
            assert!(git_branch_is_free(&tmp, "reserved").is_err());
            let error = worktree_branch_is_free(&tmp, "reserved").unwrap_err();
            assert_eq!(
                meta_plugin::envelope::code_of(&error),
                meta_plugin::ErrorCode::Conflict
            );
        }
    }

    #[test]
    fn review_unregistered_admin_rebase_does_not_reserve_a_branch() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.repo(&sandbox.work());
        sandbox.git(&tmp, &["branch", "reserved"]);
        let admin = tmp.join(".git/worktrees/orphan");
        std::fs::create_dir_all(admin.join("rebase-merge")).unwrap();
        std::fs::write(admin.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        std::fs::write(
            admin.join("rebase-merge/head-name"),
            "refs/heads/reserved\n",
        )
        .unwrap();
        // Cleanup retains its legacy state-file check, even for this orphan.
        assert!(git_branch_is_free(&tmp, "reserved").is_err());
        worktree_branch_is_free(&tmp, "reserved").unwrap();
    }

    #[test]
    fn review_primary_reservation_names_primary_from_a_linked_caller() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.repo(&sandbox.work());
        sandbox.git(&tmp, &["branch", "reserved"]);
        let caller = tmp.join("caller");
        git_worktree_add(&tmp, &caller, "caller", None, &|| Ok(None)).unwrap();
        sandbox.git(&tmp, &["checkout", "-q", "--detach"]);
        std::fs::write(tmp.join(".git/BISECT_START"), "reserved\n").unwrap();
        let error = worktree_branch_is_free(&caller, "reserved").unwrap_err();
        let primary = tmp.canonicalize().unwrap();
        assert!(
            error
                .to_string()
                .ends_with(&format!("worktree {}", escape_path(&primary))),
            "primary={primary:?}; caller={caller:?}; message={error:#}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn review_non_utf8_holder_backlink_preserves_the_path_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.repo(&sandbox.work());
        let old = tmp.join("old");
        git_worktree_add(&tmp, &old, "reserved", None, &|| Ok(None)).unwrap();
        let admin = meta_core::gitdir::linked(&old).unwrap().admin;
        std::fs::remove_dir_all(&old).unwrap();
        let holder = tmp.join(std::ffi::OsStr::from_bytes(b"holder-\xff"));
        let mut backlink = holder.join(".git").as_os_str().as_bytes().to_vec();
        backlink.push(b'\n');
        std::fs::write(admin.join("gitdir"), backlink).unwrap();
        let error = worktree_branch_is_free(&tmp, "reserved").unwrap_err();
        assert!(
            error.to_string().contains(&format!("{holder:?}")),
            "{error:#}"
        );
        assert!(!error.to_string().contains('\u{fffd}'), "{error:#}");
    }

    #[test]
    fn branch_is_free_reads_reservations_and_changes_nothing() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.repo(&sandbox.work());
        sandbox.git(&tmp, &["branch", "t"]);
        sandbox.git(&tmp, &["config", "branch.autoSetupMerge", "always"]);
        sandbox.git(&tmp, &["config", "branch.t.remote", "origin"]);
        git_branch_is_free(&tmp, "t").unwrap();
        let remote = sandbox.git(&tmp, &["config", "branch.t.remote"]);
        assert_eq!(remote.trim(), "origin");

        // Checked out in another worktree.
        let other = tmp.join(".").join("other");
        sandbox.git(
            &tmp,
            &["worktree", "add", "-q", other.to_str().unwrap(), "t"],
        );
        // Removal/pruning keep their original error code and text.
        let error = git_branch_is_free(&tmp, "t").unwrap_err();
        assert_eq!(format!("{error:#}"), "'t' is checked out in a worktree");
        assert_eq!(
            meta_plugin::envelope::code_of(&error),
            meta_plugin::ErrorCode::Failed
        );
        assert_eq!(meta_plugin::envelope::info_of(&error).hint, None);
        let conflict = worktree_branch_is_free(&tmp, "t").unwrap_err();
        assert_eq!(
            meta_plugin::envelope::code_of(&conflict),
            meta_plugin::ErrorCode::Conflict
        );
        // Bisected there instead: HEAD detached, branch still reserved.
        sandbox.git(&other, &["checkout", "-q", "--detach"]);
        git_branch_is_free(&tmp, "t").unwrap();
        let admin = tmp.join(".git/worktrees/other");
        std::fs::write(admin.join("BISECT_START"), "t\n").unwrap();
        assert!(git_branch_is_free(&tmp, "t").is_err());
        let plain = worktree_branch_is_free(&tmp, "t").unwrap_err();
        let registered = meta_core::set_record::checkout(&admin).unwrap();
        meta_plugin::errln!("native={other:?}; registered={registered:?}; message={plain:#}");
        assert_eq!(
            registered.canonicalize().unwrap(),
            other.canonicalize().unwrap()
        );
        assert!(
            plain
                .to_string()
                .contains(&format!("worktree {}", escape_path(&registered))),
            "native={other:?}; registered={registered:?}; message={plain:#}"
        );
        let record = meta_core::set_record::SetRecord {
            set: tmp.join("holding-set"),
            project: tmp.clone(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            ephemeral: false,
            ttl_seconds: None,
            custom: Default::default(),
        };
        meta_core::set_record::write(&admin, &record).unwrap();
        for state in ["bisect", "rebase"] {
            if state == "rebase" {
                std::fs::remove_file(admin.join("BISECT_START")).unwrap();
                std::fs::create_dir(admin.join("rebase-merge")).unwrap();
                std::fs::write(admin.join("rebase-merge/head-name"), "refs/heads/t\n").unwrap();
            }
            let error = worktree_branch_is_free(&tmp, "t").unwrap_err();
            assert_eq!(
                meta_plugin::envelope::code_of(&error),
                meta_plugin::ErrorCode::Conflict
            );
            assert!(error.to_string().contains("worktree set 'holding-set'"));
            assert!(error.to_string().contains(&escape_path(&record.set)));
            let removal = git_branch_is_free(&tmp, "t").unwrap_err();
            assert!(!removal.to_string().contains("holding-set"));
            assert_eq!(
                meta_plugin::envelope::code_of(&removal),
                meta_plugin::ErrorCode::Failed
            );
        }
    }

    #[test]
    fn branch_is_free_sees_checkouts_in_a_reftable_repository() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.work();
        let repo = tmp.join("repo");
        if !meta_plugin::testing::git_has_reftable() {
            return;
        }
        sandbox.git(
            &tmp,
            &[
                "init",
                "-q",
                "--ref-format=reftable",
                repo.to_str().unwrap(),
            ],
        );
        std::fs::write(repo.join("README"), "init\n").unwrap();
        sandbox.git(&repo, &["add", "README"]);
        sandbox.git(&repo, &["commit", "-qm", "initial"]);
        assert!(git_rev_parse(&repo, "HEAD").unwrap().is_some());
        sandbox.git(&repo, &["branch", "t"]);
        git_branch_is_free(&repo, "t").unwrap();
        let other = tmp.join("other");
        sandbox.git(
            &repo,
            &["worktree", "add", "-q", other.to_str().unwrap(), "t"],
        );
        assert!(git_branch_is_free(&repo, "t").is_err());
        let conflict = worktree_branch_is_free(&repo, "t").unwrap_err();
        assert_eq!(
            meta_plugin::envelope::code_of(&conflict),
            meta_plugin::ErrorCode::Conflict
        );
        let admin = meta_core::gitdir::linked(&other).unwrap().admin;
        let registered = meta_core::set_record::checkout(&admin).unwrap();
        assert_eq!(
            registered.canonicalize().unwrap(),
            other.canonicalize().unwrap()
        );
        assert!(
            conflict.to_string().contains(&escape_path(&registered)),
            "native={other:?}; registered={registered:?}; message={conflict:#}"
        );
    }

    #[test]
    fn a_symbolic_branch_is_never_moved_or_deleted() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.repo(&sandbox.work());
        let main = git_rev_parse(&tmp, "HEAD").unwrap().unwrap();
        let current = sandbox.git(&tmp, &["symbolic-ref", "HEAD"]);
        let current = current.trim();
        sandbox.git(&tmp, &["symbolic-ref", "refs/heads/t", current]);

        assert!(git_branch_is_free(&tmp, "t").is_err());
        assert!(git_delete_branch_at(&tmp, "t", &main).is_err());
        assert_eq!(git_rev_parse(&tmp, current).unwrap(), Some(main));
    }

    #[test]
    fn checkout_config_reports_core_bare_corruption_with_scoped_repair() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.work();
        sandbox.git(&tmp, &["init", "-q", "-b", "main"]);
        sandbox.git(&tmp, &["config", "--local", "core.bare", "true"]);

        let error = repo_status(&tmp).err().unwrap().to_string();
        assert!(error.contains("core.bare=true"));
        assert!(error.contains(&format!(
            "git -C {} config --local core.bare false",
            meta_plugin::display::shell_quote(&tmp.to_string_lossy())
        )));
    }

    #[test]
    fn checkout_config_guard_allows_legitimate_bare_repository() {
        let sandbox = meta_plugin::testing::isolated!();
        let tmp = sandbox.work();
        sandbox.git(&tmp, &["init", "--bare", "--quiet"]);

        ensure_checkout_config_is_not_bare(&tmp).unwrap();
    }
}
