//! Whether origin has a branch this clone hasn't fetched, so a set continues
//! it instead of quietly starting a new branch of the same name.

use anyhow::Result;
use meta_core::git::{git, lookup};
use meta_plugin::display::escape;
use meta_plugin::process;
use meta_plugin::Ctx;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::git_ops::{change, git_rev_parse, GitFailed};

/// How long origin gets to answer, per repo.
const ASK_SECONDS: u64 = 5;
/// How many repos are asked or fetched at once.
const AT_ONCE: usize = 8;

/// A repo of a set being made, on the branch it wants.
pub struct Wanted<'a> {
    pub project: &'a str,
    pub source: &'a Path,
    pub branch: &'a str,
}

enum Answer {
    /// Decided without origin, or a repo with no origin
    NotAsked,
    Has(String),
    Hasnt,
    Unreachable,
}

/// The commit origin has for each of `wanted` that isn't decided here: not
/// by `from_ref`, a local branch or a cached `origin/<branch>`. Each is asked
/// once, all before anything is made; one that can't be asked goes on as new,
/// with a warning (with `--strict`, an error). A real run also fetches each
/// branch found, so the set tracks it with its commits; a preview writes
/// nothing.
pub fn branches_on_origin(
    wanted: &[Wanted],
    from_ref: Option<&str>,
    ctx: &Ctx,
) -> Result<HashMap<PathBuf, String>> {
    if from_ref.is_some() {
        return Ok(HashMap::new());
    }
    let mut found = HashMap::new();
    for (repo, answer) in wanted.iter().zip(each(wanted, ask)) {
        match answer? {
            Answer::Has(commit) => {
                found.insert(repo.source.to_path_buf(), commit);
            }
            Answer::Unreachable => ctx.warn_or_bail(format!(
                "Can't ask origin of '{}' whether it has branch '{}' (unreachable, or no answer \
                 within {ASK_SECONDS} seconds), so the branch starts here as new",
                escape(repo.project),
                escape(repo.branch)
            ))?,
            Answer::NotAsked | Answer::Hasnt => {}
        }
    }
    if !ctx.dry_run() {
        let to_fetch: Vec<&Wanted> = wanted
            .iter()
            .filter(|repo| found.contains_key(repo.source))
            .collect();
        for fetched in each(&to_fetch, |repo| fetch(repo)) {
            fetched?;
        }
    }
    Ok(found)
}

/// `work` for each of `items`, at most [`AT_ONCE`] at a time, in their order.
fn each<T: Sync, R: Send>(items: &[T], work: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let (sender, results) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..AT_ONCE.min(items.len()) {
            let sender = sender.clone();
            let (next, work) = (&next, &work);
            scope.spawn(move || {
                while let Some((i, item)) = {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    items.get(i).map(|item| (i, item))
                } {
                    let _ = sender.send((i, work(item)));
                }
            });
        }
    });
    drop(sender);
    let mut results: Vec<(usize, R)> = results.into_iter().collect();
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, result)| result).collect()
}

/// What origin says of `repo`'s branch, if it is asked.
fn ask(repo: &Wanted) -> Result<Answer> {
    let (source, branch) = (repo.source, repo.branch);
    let reference = format!("refs/heads/{branch}");
    if git_rev_parse(source, &reference)?.is_some()
        || git_rev_parse(source, &format!("refs/remotes/origin/{branch}"))?.is_some()
        || lookup(source, ["config", "--get", "remote.origin.url"])?.is_none()
    {
        return Ok(Answer::NotAsked);
    }
    // what git wouldn't take as a branch's name is no question for origin:
    // `git worktree add` refuses it, as it always has
    let named = process::output(git(source).args(["check-ref-format", &reference]));
    if branch.starts_with('-') || !named.is_ok_and(|out| out.status.success()) {
        return Ok(Answer::NotAsked);
    }
    let mut command = git(source);
    command
        .args(["ls-remote", "--exit-code", "origin", &reference])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    no_prompts(&mut command, source);
    let Ok(mut child) = process::spawn_group(&mut command) else {
        return Ok(Answer::Unreachable);
    };
    let limit = Instant::now() + Duration::from_secs(ASK_SECONDS);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                child.stop(true);
                let _ = child.wait();
                return Ok(Answer::Unreachable);
            }
        }
    };
    Ok(match status.code() {
        Some(0) => {
            let mut said = String::new();
            if let Some(mut out) = child.stdout() {
                let _ = std::io::Read::read_to_string(&mut out, &mut said);
            }
            said.split_whitespace()
                .next()
                .map_or(Answer::Unreachable, |commit| {
                    Answer::Has(commit.to_string())
                })
        }
        // `--exit-code`: the remote answered and has no such ref
        Some(2) => Answer::Hasnt,
        _ => Answer::Unreachable,
    })
}

/// Fetch the branch into `origin/<branch>`, also in a single-branch (shallow)
/// clone, whose config must then name it for `git worktree add --track` to
/// accept it.
fn fetch(repo: &Wanted) -> Result<()> {
    let (source, branch) = (repo.source, repo.branch);
    let refspec = format!("refs/heads/{branch}:refs/remotes/origin/{branch}");
    let mut command = git(source);
    command.args(["fetch", "--quiet", "origin", &format!("+{refspec}")]);
    no_prompts(&mut command, source);
    let output = process::output(&mut command)?;
    if !output.status.success() {
        return Err(GitFailed {
            repo: source.to_path_buf(),
            command: format!("fetch origin +{refspec}"),
            dir: source.to_path_buf(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
        .into());
    }
    let fetches = lookup(source, ["config", "--get-all", "remote.origin.fetch"])?;
    let covered = fetches.unwrap_or_default().lines().any(|line| {
        let line = line.trim_start_matches('+');
        line == "refs/heads/*:refs/remotes/origin/*" || line == refspec
    });
    if !covered {
        change(
            source,
            source,
            ["remote", "set-branches", "--add", "origin", branch],
        )?;
    }
    Ok(())
}

/// `command` never waits for a person: no password prompt, and ssh gives up
/// on a host that doesn't answer. The ssh command the user set still runs.
pub(super) fn no_prompts<'a>(command: &'a mut Command, repo: &Path) -> &'a mut Command {
    command.env("GIT_TERMINAL_PROMPT", "0");
    let ssh = std::env::var("GIT_SSH_COMMAND")
        .ok()
        .filter(|ssh| !ssh.is_empty())
        .or_else(|| {
            lookup(repo, ["config", "--get", "core.sshCommand"])
                .ok()
                .flatten()
        });
    // `GIT_SSH` is a program git runs without a shell: no options to add,
    // and a `GIT_SSH_COMMAND` would replace it
    let program_set = std::env::var_os("GIT_SSH").is_some_and(|ssh| !ssh.is_empty());
    if ssh.is_some() || !program_set {
        let ssh = ssh.unwrap_or_else(|| "ssh".to_string());
        command.env(
            "GIT_SSH_COMMAND",
            format!("{ssh} -o BatchMode=yes -o ConnectTimeout={ASK_SECONDS}"),
        );
    }
    command
}
