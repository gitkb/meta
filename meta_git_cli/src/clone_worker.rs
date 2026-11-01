use anyhow::Context;
use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use meta_git_lib::clone_queue::{CloneQueue, CloneTask, Work};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Clone or pull `tasks` on a pool of `parallelism` threads, and each task
/// the `.meta.yaml` of a finished one adds, until none is left.
///
/// If `ssh_cmd` is provided, it's set as `GIT_SSH_COMMAND` on all spawned git subprocesses
/// to enable SSH multiplexing without requiring `~/.ssh/config` setup.
pub(crate) fn clone_with_queue(
    queue: &CloneQueue,
    tasks: Vec<CloneTask>,
    parallelism: usize,
    mp: &MultiProgress,
    ssh_cmd: Option<&str>,
) -> anyhow::Result<()> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(parallelism)
        .build()
        .context("Cannot start the threads that clone")?;
    let worker = Worker {
        queue,
        mp,
        style: ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {wide_msg}")
            .context("Bad progress template")?
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
        ssh_cmd,
    };
    pool.scope(|scope| {
        for task in tasks {
            worker.spawn(scope, task);
        }
    });
    Ok(())
}

/// What every task on the pool shares.
struct Worker<'a> {
    queue: &'a CloneQueue,
    mp: &'a MultiProgress,
    style: ProgressStyle,
    ssh_cmd: Option<&'a str>,
}

impl<'a> Worker<'a> {
    /// Run `task` on the pool, then the tasks it adds.
    fn spawn<'s>(&'s self, scope: &rayon::Scope<'s>, task: CloneTask)
    where
        'a: 's,
    {
        scope.spawn(move |scope| {
            for found in self.run(&task) {
                self.spawn(scope, found);
            }
        });
    }

    /// Clone or pull one repository; the tasks its `.meta.yaml` adds.
    fn run(&self, task: &CloneTask) -> Vec<CloneTask> {
        let (completed, total) = self.queue.get_counts();
        let pb = self.mp.add(ProgressBar::new_spinner());
        pb.set_style(self.style.clone());
        pb.set_prefix(format!("[{}/{}]", completed + 1, total));
        let (doing, done) = match task.work {
            Work::Clone(_) => ("Cloning", "Cloned"),
            Work::Pull => ("Pulling", "Pulled"),
        };
        pb.set_message(format!("{doing} {}", task.name));
        pb.enable_steady_tick(Duration::from_millis(100));

        if let Some(why) = matches!(task.work, Work::Pull)
            .then(|| nothing_to_pull(&task.target_path))
            .flatten()
        {
            pb.finish_with_message(format!(
                "{}",
                style(format!("Not pulled {}: {why}", task.name)).yellow()
            ));
            return self.queue.mark_not_pulled(task, why);
        }
        match run_git(task, self.queue, &pb, self.ssh_cmd) {
            Ok(()) => {
                let found = self.queue.mark_completed(task);
                let nested = match found.len() {
                    0 => String::new(),
                    n => format!(" (+{n} nested)"),
                };
                pb.finish_with_message(format!(
                    "{}",
                    style(format!("{done} {}{nested}", task.name)).green()
                ));
                found
            }
            Err(error) => {
                let found = self.queue.mark_failed(task, error);
                let failed = match task.work {
                    Work::Clone(_) => "Failed to clone",
                    Work::Pull => "Could not pull",
                };
                pb.finish_with_message(format!(
                    "{}",
                    style(format!("{failed} {}", task.name)).red()
                ));
                found
            }
        }
    }
}

/// Print each failed clone or pull with git's error, each project left
/// uncloned because its folder holds something else, each meta repo that
/// lists one above it, and each nested `.meta.yaml` that can't be read, to
/// stderr; return the command's error, or `None` when nothing failed.
pub(crate) fn report_failures(queue: &CloneQueue) -> Option<String> {
    let failures = queue.failures();
    let refused = queue.refused();
    let cycles = queue.cycles();
    let unreadable = queue.unreadable();
    if failures.is_empty() && refused.is_empty() && cycles.is_empty() && unreadable.is_empty() {
        return None;
    }
    for failure in &failures {
        match &failure.work {
            Work::Clone(url) => eprintln!(
                "{} {} ({url})",
                style("Failed to clone").red().bold(),
                meta_core::display::escape(&failure.name),
            ),
            Work::Pull => eprintln!(
                "{} {} (git pull --ff-only)",
                style("Could not pull").red().bold(),
                meta_core::display::escape(&failure.name),
            ),
        }
        // git's error names paths from the filesystem
        for line in failure.error.lines() {
            eprintln!("    {}", meta_core::display::escape(line));
        }
    }
    for (name, why) in &refused {
        eprintln!(
            "{} {}: not a git repository ({}); move it away and run 'meta git update' again",
            style("Not cloned").red().bold(),
            meta_core::display::escape(name),
            meta_core::display::escape(why)
        );
    }
    let chain = |names: &Vec<String>| meta_core::display::escape(&names.join(" → ")).into_owned();
    for names in &cycles {
        eprintln!(
            "{} {}: it is a meta repo above it again; the meta repos list each other ({})",
            style("Not cloned").red().bold(),
            meta_core::display::escape(names.last().map_or("", String::as_str)),
            chain(names)
        );
    }
    for line in &unreadable {
        eprintln!(
            "{} {}",
            style("Cannot read nested .meta.yaml").red().bold(),
            meta_core::display::escape_lines(line)
        );
    }
    let (finished, _) = queue.get_counts();
    let names = |names: Vec<&str>| names.join(", ");
    let failed = |clone: bool| -> Vec<&str> {
        failures
            .iter()
            .filter(|f| matches!(f.work, Work::Clone(_)) == clone)
            .map(|f| f.name.as_str())
            .collect()
    };
    let (clones, pulls) = (failed(true), failed(false));
    Some(
        [
            (!clones.is_empty()).then(|| {
                format!(
                    "{} of {finished} repositories failed to clone: {}",
                    clones.len(),
                    names(clones.clone())
                )
            }),
            (!pulls.is_empty()).then(|| {
                format!(
                    "{} could not be fast-forwarded: {}",
                    plural(pulls.len()),
                    names(pulls.clone())
                )
            }),
            (!refused.is_empty()).then(|| {
                format!(
                    "{} project folder(s) hold something that is not a git repository: {}",
                    refused.len(),
                    names(refused.iter().map(|(name, _)| name.as_str()).collect())
                )
            }),
            (!cycles.is_empty()).then(|| {
                format!(
                    "meta repos list each other: {}",
                    cycles.iter().map(chain).collect::<Vec<_>>().join("; ")
                )
            }),
            (!unreadable.is_empty()).then(|| {
                format!(
                    "{} nested .meta.yaml file(s) can't be read",
                    unreadable.len()
                )
            }),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("; "),
    )
}

/// Why the repository at `dir` has nothing to pull from, if so: it isn't
/// on a branch, or its branch has no upstream. A question git can't answer
/// is left to the pull, which reports it.
pub(crate) fn nothing_to_pull(dir: &Path) -> Option<String> {
    let head = match meta_core::git::lookup(dir, ["symbolic-ref", "-q", "HEAD"]) {
        Ok(Some(head)) => head,
        Ok(None) => return Some("HEAD is not on a branch".to_string()),
        Err(_) => return None,
    };
    // From the config: a branch with no commits yet (an empty repo's
    // clone) has its upstream there all the same
    let branch = head.strip_prefix("refs/heads/").unwrap_or(&head);
    let merge = format!("branch.{branch}.merge");
    match meta_core::git::lookup(dir, ["config", "--get", merge.as_str()]) {
        Ok(None) => Some(format!(
            "its branch {} has no upstream",
            meta_core::display::escape(branch)
        )),
        _ => None,
    }
}

fn plural(n: usize) -> String {
    match n {
        1 => "1 repository".to_string(),
        n => format!("{n} repositories"),
    }
}

/// Run git's clone or pull for `task`; git's last lines of error output
/// if it fails.
fn run_git(
    task: &CloneTask,
    queue: &CloneQueue,
    pb: &ProgressBar,
    ssh_cmd: Option<&str>,
) -> Result<(), String> {
    // What appeared in the folder since it was queued makes git refuse it,
    // which is reported like any failed clone: never taken for a clone
    let mut cmd = match &task.work {
        Work::Clone(url) => {
            let mut cmd = meta_core::git::git(Path::new("."));
            cmd.arg("clone");
            if let Some(d) = queue.git_depth() {
                cmd.arg("--depth").arg(d);
            }
            cmd.arg("--").arg(url.as_git_arg()).arg(&task.target_path);
            cmd
        }
        Work::Pull => {
            let mut cmd = meta_core::git::git(&task.target_path);
            // --no-rebase: before git 2.34, pull.rebase won over --ff-only
            cmd.args(["pull", "--ff-only", "--no-rebase"]);
            cmd
        }
    };
    if let Some(ssh) = ssh_cmd {
        cmd.env("GIT_SSH_COMMAND", ssh);
    }
    // Many run at once under spinners: git fails instead of asking for
    // credentials. Only stderr is read: git's progress and errors. Unread
    // stdout would fill its pipe and stall git.
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let mut child =
        meta_core::children::spawn(&mut cmd).map_err(|e| format!("Failed to run git: {e}"))?;
    // Stream stderr for progress updates, and keep its last lines for
    // the failure report (the progress bar is hidden off a terminal).
    let pb = pb.clone();
    let task_name = task.name.clone();
    let tail = StderrTail::spawn(child.stderr(), move |line| {
        // git's progress names paths from the filesystem
        pb.set_message(format!("{task_name}: {}", meta_core::display::escape(line)))
    });
    match child.wait() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => {
            let stderr_text = tail.text();
            Err(if stderr_text.trim().is_empty() {
                format!("git exited with {status}")
            } else {
                stderr_text
            })
        }
        Err(e) => Err(format!("Failed to wait for git: {e}")),
    }
}

/// The last lines a process writes to stderr, kept by a reader thread that
/// drains the pipe and holds at most [`StderrTail::KEPT_LINES`].
struct StderrTail {
    lines: Arc<Mutex<std::collections::VecDeque<String>>>,
    done: std::sync::mpsc::Receiver<()>,
}

impl StderrTail {
    const KEPT_LINES: usize = 20;
    const LINE_BYTES: u64 = 4096;

    fn spawn(
        stderr: Option<impl std::io::Read + Send + 'static>,
        on_line: impl Fn(&str) + Send + 'static,
    ) -> Self {
        let lines = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        let (done_tx, done) = std::sync::mpsc::channel();
        if let Some(stderr) = stderr {
            let kept = Arc::clone(&lines);
            std::thread::spawn(move || {
                use std::io::{BufRead, BufReader, Read};
                let mut reader = BufReader::new(stderr);
                let mut buf = Vec::new();
                // Reads at most LINE_BYTES at a time, so one huge line can't
                // grow memory; a longer line is kept in pieces.
                while (&mut reader)
                    .take(Self::LINE_BYTES)
                    .read_until(b'\n', &mut buf)
                    .is_ok_and(|n| n > 0)
                {
                    let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                    buf.clear();
                    on_line(&line);
                    let mut kept = kept.lock().unwrap_or_else(|e| e.into_inner());
                    if kept.len() == Self::KEPT_LINES {
                        kept.pop_front();
                    }
                    kept.push_back(line);
                }
                drop(done_tx);
            });
        }
        Self { lines, done }
    }

    /// The kept lines. Waits briefly for the reader to finish, but not for a
    /// background process (an ssh master, say) that still holds the pipe.
    fn text(&self) -> String {
        let _ = self.done.recv_timeout(Duration::from_secs(2));
        let lines = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        Vec::from(lines.clone()).join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn stderr_tail_drains_everything_and_keeps_only_the_last_lines() {
        let output: String = (0..100_000).map(|i| format!("line {i}\n")).collect();
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        let tail = StderrTail::spawn(Some(std::io::Cursor::new(output)), move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });

        let text = tail.text();
        assert_eq!(seen.load(Ordering::SeqCst), 100_000);
        assert_eq!(text.lines().count(), StderrTail::KEPT_LINES);
        assert!(text.ends_with("line 99999"));
    }

    #[test]
    fn stderr_tail_bounds_a_single_huge_line() {
        let output = "x".repeat(10_000_000);
        let tail = StderrTail::spawn(Some(std::io::Cursor::new(output)), |_| {});
        let text = tail.text();
        assert!(text.len() <= StderrTail::KEPT_LINES * (StderrTail::LINE_BYTES as usize + 1));
    }
}
