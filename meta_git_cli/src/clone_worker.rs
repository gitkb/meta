use crate::clone_queue::{CloneQueue, CloneTask, Outcome};
use anyhow::Context;
use colored::Colorize;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use meta_plugin::errln;
use meta_plugin::RepoUrl;
use std::ffi::OsString;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Clone `tasks` on a pool of `parallelism` threads, and each task
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
        started: AtomicUsize::new(0),
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
    /// How many tasks have started, to number each once
    started: AtomicUsize,
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

    /// Clone one repository; the tasks its `.meta.yaml` adds.
    fn run(&self, task: &CloneTask) -> Vec<CloneTask> {
        let number = self.started.fetch_add(1, Ordering::SeqCst) + 1;
        let (_, total) = self.queue.get_counts();
        let pb = self.mp.add(ProgressBar::new_spinner());
        pb.set_style(self.style.clone());
        pb.set_prefix(format!("[{number}/{total}]"));
        pb.set_message(format!("Cloning {}", task.name));
        pb.enable_steady_tick(Duration::from_millis(100));
        match run_git(task, self.queue, &pb, self.ssh_cmd) {
            Ok(()) => {
                let found = self.queue.mark_completed(task);
                let nested = match found.len() {
                    0 => String::new(),
                    n => format!(" (+{n} nested)"),
                };
                pb.finish_with_message(format!(
                    "{}",
                    format!("Cloned {}{nested}", task.name).green()
                ));
                found
            }
            Err(error) => {
                self.queue.mark_failed(task, error);
                pb.finish_with_message(format!(
                    "{}",
                    format!("Failed to clone {}", task.name).red()
                ));
                Vec::new()
            }
        }
    }
}

/// A row for each clone that failed, with git's stderr, as `meta
/// --json exec` gives a failed step's: named by its path from `root`.
pub(crate) fn failure_rows(queue: &CloneQueue, root: &Path) -> Vec<meta_plugin::ProjectResult> {
    queue
        .outcome()
        .failures
        .iter()
        .map(|failure| {
            let last = failure
                .error
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("no stderr");
            let project = match failure.path.strip_prefix(root) {
                Ok(inside) if inside.as_os_str().is_empty() => ".".to_string(),
                Ok(inside) => inside.to_string_lossy().into_owned(),
                Err(_) => failure.path.to_string_lossy().into_owned(),
            };
            meta_plugin::ProjectResult {
                stderr: Some(failure.error.clone()),
                ..meta_plugin::ProjectResult::error(
                    project,
                    meta_plugin::ErrorInfo::new(
                        meta_plugin::ErrorCode::ToolFailed,
                        format!(
                            "{} could not be cloned: {}",
                            meta_plugin::display::escape(&failed(failure)),
                            meta_plugin::display::escape(last)
                        ),
                    ),
                )
                .at(&failure.path)
            }
        })
        .collect()
}

/// A failed clone's project as its errors name it: with the branch it was
/// to check out, when its config names one ("api (branch release)").
fn failed(failure: &crate::clone_queue::CloneFailure) -> String {
    match &failure.branch {
        Some(branch) => format!("{} (branch {branch})", failure.name),
        None => failure.name.clone(),
    }
}

/// Print each failed clone with git's error, each project left
/// uncloned because its folder holds something else, each meta repo that
/// lists one above it, and each nested `.meta.yaml` that can't be read, to
/// stderr; return the command's error, or `None` when nothing failed.
/// `command` (`git update`) is the one to run again.
pub(crate) fn report_failures(queue: &CloneQueue, command: &str) -> Option<String> {
    let Outcome {
        cloned,
        failures,
        refused,
        cycles,
        unreadable,
        ..
    } = queue.outcome();
    if failures.is_empty() && refused.is_empty() && cycles.is_empty() && unreadable.is_empty() {
        return None;
    }
    for failure in &failures {
        errln!(
            "{} {} ({}{})",
            "Failed to clone".red().bold(),
            meta_plugin::display::escape(&failure.name),
            failure.url,
            failure
                .branch
                .as_ref()
                .map(|b| format!(", branch {}", meta_plugin::display::escape(b)))
                .unwrap_or_default(),
        );
        // git's error names paths from the filesystem
        for line in failure.error.lines() {
            errln!("    {}", meta_plugin::display::escape(line));
        }
    }
    for (name, _, why) in &refused {
        errln!(
            "{} {}: not a git repository ({}); move it away and run 'meta {command}' again",
            "Not cloned".red().bold(),
            meta_plugin::display::escape(name),
            meta_plugin::display::escape(why)
        );
    }
    let chain = |names: &Vec<String>| meta_plugin::display::escape(&names.join(" → ")).into_owned();
    for names in &cycles {
        errln!(
            "{} {}: it is a meta repo above it again; the meta repos list each other ({})",
            "Left out".red().bold(),
            meta_plugin::display::escape(names.last().map_or("", String::as_str)),
            chain(names)
        );
    }
    for line in &unreadable {
        errln!(
            "{} {}",
            "Cannot read nested .meta.yaml".red().bold(),
            meta_plugin::display::escape_lines(line)
        );
    }
    // each named with why: the excerpt of git's error every summary shows
    let clones: Vec<String> = failures
        .iter()
        .map(|f| {
            let name = meta_plugin::display::escape(&failed(f)).into_owned();
            match meta_plugin::display::Excerpt::of(f.error.as_bytes())
                .lines()
                .as_slice()
            {
                [] => format!("{name}: no output"),
                [one] => format!("{name}: {one}"),
                many => format!(
                    "{name}:{}",
                    many.iter()
                        .map(|line| format!("\n    {line}"))
                        .collect::<String>()
                ),
            }
        })
        .collect();
    Some(
        [
            (!clones.is_empty()).then(|| {
                format!(
                    "{} of {} repositories failed to clone: {}",
                    clones.len(),
                    cloned.len() + clones.len(),
                    match clones.as_slice() {
                        [one] => one.clone(),
                        many => many.iter().map(|c| format!("\n  {c}")).collect(),
                    }
                )
            }),
            (!refused.is_empty()).then(|| {
                format!(
                    "{} project folder(s) hold something that is not a git repository: {}",
                    refused.len(),
                    refused
                        .iter()
                        .map(|(name, _, _)| name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
            (!cycles.is_empty()).then(|| {
                format!(
                    "the meta repos list each other: {}",
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

/// The git command `task` runs, `url` giving a clone's URL as git gets it
/// or as meta shows it.
pub(crate) fn git_argv(
    task: &CloneTask,
    git_depth: Option<&str>,
    url: impl Fn(&RepoUrl) -> OsString,
) -> Vec<OsString> {
    let path = task.target_path.clone().into_os_string();
    ["git", "clone"]
        .map(OsString::from)
        .into_iter()
        .chain(
            git_depth
                .into_iter()
                .flat_map(|d| ["--depth", d].map(OsString::from)),
        )
        .chain(
            task.branch
                .iter()
                .flat_map(|b| [OsString::from("--branch"), OsString::from(b)]),
        )
        .chain(["--".into(), url(&task.url), path])
        .collect()
}

/// Run git's clone for `task`; git's last lines of error output
/// if it fails.
fn run_git(
    task: &CloneTask,
    queue: &CloneQueue,
    pb: &ProgressBar,
    ssh_cmd: Option<&str>,
) -> Result<(), String> {
    // What appeared in the folder since it was queued makes git refuse it,
    // which is reported like any failed clone: never taken for a clone
    let mut cmd = meta_core::git::git(Path::new("."));
    let argv = git_argv(task, queue.git_depth(), |url| {
        url.as_git_arg().to_os_string()
    });
    cmd.args(argv.iter().skip(1));
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
        meta_plugin::process::spawn(&mut cmd).map_err(|e| format!("Failed to run git: {e}"))?;
    // Stream stderr for progress updates, and keep its last lines for
    // the failure report (the progress bar is hidden off a terminal).
    let pb = pb.clone();
    let task_name = task.name.clone();
    let tail = StderrTail::spawn(child.stderr(), move |line| {
        // git's progress names paths from the filesystem
        pb.set_message(format!(
            "{task_name}: {}",
            meta_plugin::display::escape(line)
        ))
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
        assert!(
            text.len()
                <= StderrTail::KEPT_LINES * (usize::try_from(StderrTail::LINE_BYTES).unwrap() + 1)
        );
    }
}
