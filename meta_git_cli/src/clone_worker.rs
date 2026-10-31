use console::style;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use log::debug;
use meta_core::git::GitUrl;
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

/// Print each failed clone with git's error to stderr, and return the
/// command's error, or `None` when every clone succeeded.
pub(crate) fn report_failures(queue: &CloneQueue) -> Option<String> {
    let failures = queue.failures();
    if failures.is_empty() {
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
    let (finished, _) = queue.get_counts();
    Some(format!(
        "{} of {finished} repositories failed to clone: {}",
        failures.len(),
        failures
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ))
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
    // Skip if target exists
    if task.target_path.exists()
        && task
            .target_path
            .read_dir()
            .map(|mut iter| iter.next().is_some())
            .unwrap_or(false)
    {
        pb.finish_with_message(format!(
            "{}",
            style(format!("Skipped {} (exists)", task.name)).yellow()
        ));
        // Still mark as completed and check for nested .meta.yaml
        if let Err(e) = queue.mark_completed(task) {
            debug!(
                "Failed to check nested .meta.yaml for {}: {}",
                task.name,
                meta_core::display::escape_lines(&format!("{e:#}"))
            );
            record(e);
        }
        return;
    }

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
    match cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(mut child) => {
            // Stream stderr for progress updates, and keep its last lines for
            // the failure report (the progress bar is hidden off a terminal).
            let pb_clone = pb.clone();
            let task_name = task.name.clone();
            let tail = StderrTail::spawn(child.stderr.take(), task.url.clone(), move |line| {
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
    const LINE_BYTES: u64 = meta_core::git::GIT_MESSAGE_BYTES as u64;

    /// git's output while it clones `url`.
    fn spawn(
        stderr: Option<impl std::io::Read + Send + 'static>,
        url: GitUrl,
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
                    // The rest of a line cut here goes; redact_git_output
                    // takes the last word of a line this long, cut here or
                    // by git, which may start a URL
                    if buf.len() as u64 == Self::LINE_BYTES && !buf.ends_with(b"\n") {
                        let _ = reader.skip_until(b'\n');
                    }
                    let line = String::from_utf8_lossy(&buf);
                    // Trimmed only after: the cut needs the line's length
                    // as read
                    let line = meta_core::git::redact_git_output(&line, Some(&url));
                    let line = line.trim_ascii_end().to_string();
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
        let tail = StderrTail::spawn(
            Some(std::io::Cursor::new(output)),
            GitUrl::default(),
            move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );

        let text = tail.text();
        assert_eq!(seen.load(Ordering::SeqCst), 100_000);
        assert_eq!(text.lines().count(), StderrTail::KEPT_LINES);
        assert!(text.ends_with("line 99999"));
    }

    #[test]
    fn stderr_tail_shows_no_credentials() {
        // a URL that a cut at LINE_BYTES splits, one git cut itself (its
        // messages stop at 4096 bytes), each also with a Unicode space in its
        // password (git sends one), and a whole one
        let pad = "x ".repeat(2041);
        let git_cut = format!("warning: {}https://alice:ghp_SECRET", "y ".repeat(2031));
        assert_eq!(git_cut.len() + 1, StderrTail::LINE_BYTES as usize);
        let spaced = format!("https://alice:ghp_SECRET\u{a0}{}@h/r", "z".repeat(5000));
        let git_cut_spaced = format!("warning: {spaced}");
        let git_cut_spaced = git_cut_spaced
            .get(..StderrTail::LINE_BYTES as usize - 1)
            .unwrap();
        // git's cut right after a Unicode space
        let at_space = format!(
            "warning: https://alice:ghp_SECRET{}\u{2003}",
            "w".repeat(4059)
        );
        assert_eq!(at_space.len() + 1, StderrTail::LINE_BYTES as usize);
        // a helper's URL with a space in its password: cut, whole as the
        // helper prints it (without its `foo::`), and cut so
        let helper = "foo::https://alice:ghp_SECRET w@h/r";
        let pad_cut = |line: String| {
            let line = format!("{line} {}@h/r", "v".repeat(5000));
            line.get(..StderrTail::LINE_BYTES as usize - 1)
                .unwrap()
                .to_string()
        };
        let helper_cut = pad_cut("warning: foo::https://alice:ghp_SECRET".into());
        let address_cut = pad_cut("warning: https://alice:ghp_SECRET".into());
        // a helper's password with spaces, cut among them by meta's read
        let spaces_cut = format!("fatal: https://alice:ghp_SECRET{}@h/r", " ".repeat(5000));
        // two spaces in it, cut after the second
        let two_spaces_cut = pad_cut("warning: https://alice:ghp_SECRET w".into());
        // the whole address first, then a second copy that loses its `@`
        // to the cut
        let twice_cut = pad_cut(format!(
            "fatal: cannot open https://alice:ghp_SECRET w@h/r {} retry https://alice:ghp_SECRET",
            "u".repeat(3990)
        ));
        let output = format!(
            "{pad}https://alice:ghp_SECRET@h/r\n{git_cut}\n{spaced}\n{git_cut_spaced}\n\
             {at_space}\n{helper_cut}\n{address_cut}\n{twice_cut}\n{spaces_cut}\n{two_spaces_cut}\nfatal: cannot open https://alice:ghp_SECRET w@h/r\n\
             fatal: 'https://a:ghp_SECRET@h/r'\n"
        );
        let shown = Arc::new(Mutex::new(String::new()));
        let seen = Arc::clone(&shown);
        let tail = StderrTail::spawn(
            Some(std::io::Cursor::new(output)),
            helper.into(),
            move |line| {
                seen.lock().unwrap().push_str(line);
            },
        );
        let text = tail.text();
        assert!(text.ends_with("fatal: 'https://***@h/r'"), "{text}");
        assert!(
            text.contains("fatal: cannot open https://***@h/r\n"),
            "{text}"
        );
        for printed in [text, shown.lock().unwrap().clone()] {
            assert!(
                !printed.contains("ghp_SECRET") && !printed.contains("alice"),
                "{printed}"
            );
        }
    }

    #[test]
    fn stderr_tail_bounds_a_single_huge_line() {
        let output = "x".repeat(10_000_000);
        let tail = StderrTail::spawn(
            Some(std::io::Cursor::new(output)),
            GitUrl::default(),
            |_| {},
        );
        let text = tail.text();
        assert!(text.len() <= StderrTail::KEPT_LINES * (StderrTail::LINE_BYTES as usize + 1));
    }
}
