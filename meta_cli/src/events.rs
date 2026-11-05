//! The events a command reports while it runs ([`meta_plugin::Event`]):
//! a plugin's, read from the file its call names in `META_EVENTS`, and the
//! plan runner's steps. Each is shown as it arrives (text, or a JSON line
//! under `--json-stream`) and kept for the one Envelope.

use meta_plugin::display::escape;
use meta_plugin::event::{Event, MAX_LINE};
use meta_plugin::{ProjectResult, Warning};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// How events are shown as they arrive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    /// Records and warnings in text; progress on a terminal's stderr
    Text,
    /// Nothing: they go into the Envelope (`--json`)
    Json,
    /// One `{"event": …}` line each on stdout (`--json-stream`)
    Stream,
}

/// Show `event` as `show` says.
pub fn show(show: Show, event: &Event) {
    match show {
        Show::Json => {}
        Show::Stream => {
            let line = serde_json::json!({ "event": event });
            let _ = meta_plugin::out::document(&line.to_string());
        }
        Show::Text => match event {
            Event::Record(record) => {
                let _ = crate::invoke::show(std::slice::from_ref(record));
            }
            Event::Warning(warning) => meta_plugin::out::warn(&warning.message),
            Event::Progress(progress) => {
                if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
                    let of = progress.total.map(|t| format!("/{t}")).unwrap_or_default();
                    let at = progress.project.as_deref().map(escape).unwrap_or_default();
                    let says = progress.message.as_deref().map(escape).unwrap_or_default();
                    meta_plugin::errln!("[{}{of}] {at} {says}", progress.done);
                }
            }
            // the plan runner shows its steps itself
            Event::Step(_) => {}
        },
    }
}

/// What a plugin's events added to its outcome.
#[derive(Default)]
pub struct Received {
    pub records: Vec<ProjectResult>,
    pub warnings: Vec<Warning>,
    /// Why the events can't be trusted (a line too long, not an event, a
    /// step): the call fails with `plugin-failed`
    pub problem: Option<String>,
}

/// The events of one plugin call, read as they arrive.
pub struct Channel {
    path: PathBuf,
    ended: Arc<AtomicBool>,
    reader: std::thread::JoinHandle<Received>,
}

impl Channel {
    /// A channel in `dir` (the call's private folder), shown as `how`. On
    /// Unix a FIFO: a plugin that writes faster than meta reads waits, so
    /// nothing piles up; elsewhere a file meta follows.
    pub fn open(dir: &Path, how: Show) -> anyhow::Result<Channel> {
        let path = dir.join("events");
        let file = endpoint(&path)?;
        let ended = Arc::new(AtomicBool::new(false));
        let done = Arc::clone(&ended);
        let reader = std::thread::spawn(move || read(file, &done, how));
        Ok(Channel {
            path,
            ended,
            reader,
        })
    }

    /// The path a plugin writes its events to (`META_EVENTS`).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The events, once the plugin ended: what is left is read first.
    pub fn finish(self) -> Received {
        self.ended.store(true, Ordering::Release);
        // it may be waiting for more: wake it now
        self.reader.thread().unpark();
        self.reader.join().unwrap_or_else(|_| Received {
            problem: Some("meta's reader of its events failed".to_string()),
            ..Received::default()
        })
    }
}

/// The FIFO at `path`, open for reading without waiting for a writer.
#[cfg(unix)]
fn endpoint(path: &Path) -> anyhow::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    nix::unistd::mkfifo(
        path,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )?;
    let nonblocking = i32::try_from(rustix::fs::OFlags::NONBLOCK.bits()).unwrap_or_default();
    Ok(std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(nonblocking)
        .open(path)?)
}

/// An empty file at `path`, which meta follows as the plugin appends.
#[cfg(not(unix))]
fn endpoint(path: &Path) -> anyhow::Result<std::fs::File> {
    std::fs::File::create(path)?;
    Ok(std::fs::File::open(path)?)
}

/// Read lines from `file` until the plugin `ended` and nothing is left.
fn read(mut file: std::fs::File, ended: &AtomicBool, how: Show) -> Received {
    let mut received = Received::default();
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        // read before checking: what a plugin wrote before it ended counts
        let finished = ended.load(Ordering::Acquire);
        match file.read(&mut chunk) {
            Ok(0) => {}
            Ok(n) => {
                pending.extend_from_slice(chunk.get(..n).unwrap_or_default());
                lines(&mut pending, &mut received, how);
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => {
                received
                    .problem
                    .get_or_insert(format!("its events could not be read ({e})"));
                break;
            }
        }
        if finished {
            break;
        }
        std::thread::park_timeout(Duration::from_millis(10));
    }
    if !pending.is_empty() {
        received
            .problem
            .get_or_insert("its last event line has no newline".to_string());
    }
    received
}

/// Take the whole lines out of `pending` as events.
fn lines(pending: &mut Vec<u8>, received: &mut Received, how: Show) {
    while let Some(end) = pending.iter().position(|b| *b == b'\n') {
        let rest = pending
            .get(end + 1..)
            .map(<[u8]>::to_vec)
            .unwrap_or_default();
        pending.truncate(end);
        let line = std::mem::replace(pending, rest);
        if received.problem.is_some() || line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match Event::parse(&line) {
            Ok(Event::Step(_)) => {
                received.problem = Some("it sent a step event, which is meta's own".to_string())
            }
            Ok(event) => {
                show(how, &event);
                match event {
                    Event::Record(record) => received.records.push(record),
                    Event::Warning(warning) => received.warnings.push(warning),
                    _ => {}
                }
            }
            Err(why) => received.problem = Some(format!("it wrote {why}")),
        }
    }
    if pending.len() > MAX_LINE {
        received
            .problem
            .get_or_insert(format!("it wrote an event line past {MAX_LINE} bytes"));
        pending.clear();
    }
}
