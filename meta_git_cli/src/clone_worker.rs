use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::debug;
use meta_git_lib::clone_queue::{CloneQueue, CloneTask};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Clone repositories using a worker pool where each worker continuously pulls from the queue.
///
/// If `ssh_cmd` is provided, it's set as `GIT_SSH_COMMAND` on all spawned git subprocesses
/// to enable SSH multiplexing without requiring `~/.ssh/config` setup.
pub(crate) fn clone_with_queue(
    queue: Arc<CloneQueue>,
    parallelism: usize,
    mp: &MultiProgress,
    ssh_cmd: Option<&str>,
) -> anyhow::Result<()> {
    use std::sync::Condvar;

    let spinner_style = ProgressStyle::with_template("{prefix:.bold.dim} {spinner} {wide_msg}")
        .unwrap()
        .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");

    // Track active workers for termination detection
    let active_workers = Arc::new(AtomicUsize::new(0));
    // Condition variable to signal when work might be available or workers finish
    let work_signal = Arc::new((Mutex::new(false), Condvar::new()));

    let ssh_cmd = Arc::new(ssh_cmd.map(|s| s.to_string()));
    // Nested .meta.yaml files that failed to parse or validate
    let nested_errors = Arc::new(Mutex::new(Vec::new()));

    // Spawn worker threads
    let handles: Vec<_> = (0..parallelism)
        .map(|_worker_id| {
            let queue = Arc::clone(&queue);
            let active = Arc::clone(&active_workers);
            let signal = Arc::clone(&work_signal);
            let mp = mp.clone();
            let style = spinner_style.clone();
            let ssh_cmd = Arc::clone(&ssh_cmd);
            let nested_errors = Arc::clone(&nested_errors);

            std::thread::spawn(move || {
                loop {
                    // Mark worker as active BEFORE taking a task to prevent
                    // a race where is_finished() sees pending=empty, active=0
                    // while a worker is between take_one() and starting work.
                    active.fetch_add(1, Ordering::SeqCst);

                    // Try to get a task
                    let task = queue.take_one();

                    match task {
                        Some(task) => {
                            // Create progress bar for this task
                            let (completed, total) = queue.get_counts();
                            let pb = mp.add(ProgressBar::new_spinner());
                            pb.set_style(style.clone());
                            pb.set_prefix(format!("[{}/{}]", completed + 1, total));
                            pb.set_message(format!("Cloning {}", task.name));
                            pb.enable_steady_tick(Duration::from_millis(100));

                            // Clone the repo (this may add new tasks to queue)
                            clone_single_repo(
                                &task,
                                &queue,
                                &pb,
                                ssh_cmd.as_deref(),
                                &nested_errors,
                            );

                            // Mark worker as inactive
                            active.fetch_sub(1, Ordering::SeqCst);

                            // Signal that we're done (might enable termination check)
                            let (lock, cvar) = &*signal;
                            let mut done = lock.lock().unwrap_or_else(|e| e.into_inner());
                            *done = true;
                            cvar.notify_all();
                        }
                        None => {
                            // No task available - mark worker as inactive
                            active.fetch_sub(1, Ordering::SeqCst);

                            // Check if we should terminate
                            if queue.is_finished(&active) {
                                break;
                            }

                            // Wait for signal that work might be available
                            let (lock, cvar) = &*signal;
                            let done = lock.lock().unwrap_or_else(|e| e.into_inner());
                            // Wait with timeout to periodically recheck
                            let _ = cvar.wait_timeout(done, Duration::from_millis(50));
                        }
                    }
                }
            })
        })
        .collect();

    // Wait for all workers to finish
    for handle in handles {
        handle.join().expect("Worker thread panicked");
    }

    // A nested config that is refused leaves its repos uncloned, so the
    // command must not report success
    let errors = nested_errors.lock().unwrap_or_else(|e| e.into_inner());
    if !errors.is_empty() {
        // Failed clones are reported too, not hidden behind this error
        let failed = report_failures(&queue)
            .map(|summary| format!("\n{summary}"))
            .unwrap_or_default();
        anyhow::bail!(
            "Could not read nested .meta.yaml files:\n{}{failed}",
            errors.join("\n")
        );
    }
    Ok(())
}

/// Print each failed clone with git's error, and each project left uncloned
/// because its folder holds something else, to stderr; return the command's
/// error, or `None` when every project is cloned.
pub(crate) fn report_failures(queue: &CloneQueue) -> Option<String> {
    let failures = queue.failures();
    let refused = queue.refused();
    if failures.is_empty() && refused.is_empty() {
        return None;
    }
    for failure in &failures {
        eprintln!(
            "{} {} ({})",
            style("Failed to clone").red().bold(),
            meta_core::display::escape(&failure.name),
            failure.url
        );
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
    let (finished, _) = queue.get_counts();
    let names = |names: Vec<&str>| names.join(", ");
    Some(
        [
            (!failures.is_empty()).then(|| {
                format!(
                    "{} of {finished} repositories failed to clone: {}",
                    failures.len(),
                    names(failures.iter().map(|f| f.name.as_str()).collect())
                )
            }),
            (!refused.is_empty()).then(|| {
                format!(
                    "{} project folder(s) hold something that is not a git repository: {}",
                    refused.len(),
                    names(refused.iter().map(|(name, _)| name.as_str()).collect())
                )
            }),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("; "),
    )
}

/// Clone a single repository and handle .meta.yaml discovery
fn clone_single_repo(
    task: &CloneTask,
    queue: &Arc<CloneQueue>,
    pb: &ProgressBar,
    ssh_cmd: Option<&str>,
    nested_errors: &Mutex<Vec<String>>,
) {
    let record = |e: anyhow::Error| {
        nested_errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(format!("  {}: {e:#}", task.name));
    };
    // What appeared in the folder since it was queued makes git refuse it,
    // which is reported like any failed clone: never taken for a clone
    // Build git clone command
    let mut cmd = meta_core::git::git(Path::new("."));
    cmd.arg("clone");
    if let Some(d) = queue.git_depth() {
        cmd.arg("--depth").arg(d);
    }
    cmd.arg("--")
        .arg(task.url.as_git_arg())
        .arg(&task.target_path);
    if let Some(ssh) = ssh_cmd {
        cmd.env("GIT_SSH_COMMAND", ssh);
    }

    // Run clone
    match meta_core::children::spawn(
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped()),
    ) {
        Ok(mut child) => {
            // Stream stderr for progress updates, and keep its last lines for
            // the failure report (the progress bar is hidden off a terminal).
            let pb_clone = pb.clone();
            let task_name = task.name.clone();
            let tail = StderrTail::spawn(child.stderr(), move |line| {
                // git's progress names paths from the filesystem
                pb_clone.set_message(format!("{task_name}: {}", meta_core::display::escape(line)))
            });

            let status = child.wait();
            match status {
                Ok(status) if status.success() => {
                    // Check for nested .meta.yaml and report new discoveries
                    match queue.mark_completed(task) {
                        Ok(added) if added > 0 => {
                            let (_, total) = queue.get_counts();
                            pb.finish_with_message(format!(
                                "{}",
                                style(format!("Cloned {} (+{} nested)", task.name, added)).green()
                            ));
                            // Update for new total
                            debug!(
                                "Discovered {} more repos in {}, total now {}",
                                added, task.name, total
                            );
                        }
                        Ok(_) => {
                            pb.finish_with_message(format!(
                                "{}",
                                style(format!("Cloned {}", task.name)).green()
                            ));
                        }
                        Err(e) => {
                            pb.finish_with_message(format!(
                                "{}",
                                style(format!(
                                    "Cloned {} (meta parse error: {})",
                                    task.name,
                                    meta_core::display::escape_lines(&format!("{e:#}"))
                                ))
                                .yellow()
                            ));
                            record(e);
                        }
                    }
                }
                Ok(status) => {
                    let stderr_text = tail.text();
                    let error = if stderr_text.trim().is_empty() {
                        format!("git clone exited with {status}")
                    } else {
                        stderr_text
                    };
                    queue.mark_failed(task, error);
                    pb.finish_with_message(format!(
                        "{}",
                        style(format!("Failed to clone {}", task.name)).red()
                    ));
                }
                Err(e) => {
                    queue.mark_failed(task, format!("Failed to wait for git: {e}"));
                    pb.finish_with_message(format!(
                        "{}",
                        style(format!("Failed to clone {}", task.name)).red()
                    ));
                }
            }
        }
        Err(e) => {
            queue.mark_failed(task, format!("Failed to run git: {e}"));
            pb.finish_with_message(format!(
                "{}",
                style(format!("Failed to spawn git for {}", task.name)).red()
            ));
        }
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
