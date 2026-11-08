//! A Rust plugin's runtime: [`run_plugin`] answers the host's call, and
//! hands the plugin a [`Ctx`] for what it reports on the way.
//!
//! # The call
//!
//! The host starts the plugin with `--meta-plugin-exec`, the [`Request`]
//! as JSON in the file `META_REQUEST` names, and reads its [`Response`]
//! from the file `META_RESPONSE` names. The plugin keeps the user's
//! terminal: stdin and stderr are meta's, and so is stdout, except under
//! `--json`, when it is meta's stderr (meta's stdout holds the Envelope
//! alone). A plugin that ends without writing a response, or writes one
//! the host can't read, fails with `plugin-failed`.

use crate::envelope::{info_of, ErrorCode, ErrorInfo, Warning};
use crate::event::Event;
use crate::response::{Answer, Response};
use crate::{PluginInfo, Request};
use std::cell::RefCell;

/// The variable naming the file the request is in.
pub const REQUEST_VAR: &str = "META_REQUEST";
/// The variable naming the file the response goes to.
pub const RESPONSE_VAR: &str = "META_RESPONSE";

/// What a plugin reports while it works: warnings collected into its
/// response, and events streamed to meta as they happen.
pub struct Ctx {
    meta: crate::Meta,
    warnings: RefCell<Vec<Warning>>,
    /// Where events go (`META_EVENTS`); with none, records are kept for the
    /// response and progress is dropped
    events: Option<std::sync::Mutex<std::fs::File>>,
    kept: RefCell<Vec<crate::ProjectResult>>,
}

impl Ctx {
    /// A context for a call with `meta`'s options, its events unsent.
    pub fn new(meta: crate::Meta) -> Self {
        Ctx {
            meta,
            warnings: RefCell::new(Vec::new()),
            events: None,
            kept: RefCell::new(Vec::new()),
        }
    }

    /// A context whose events go to `events` (the file `META_EVENTS` names).
    fn streaming(meta: crate::Meta, events: std::fs::File) -> Self {
        Ctx {
            events: Some(std::sync::Mutex::new(events)),
            ..Ctx::new(meta)
        }
    }

    /// Send `event` to meta now. A step is meta's own event: a plugin's is
    /// refused.
    pub fn emit(&self, event: Event) -> anyhow::Result<()> {
        use std::io::Write;
        anyhow::ensure!(
            !matches!(event, Event::Step(_)),
            "a step event is meta's own: a plugin sends records"
        );
        let Some(events) = &self.events else {
            match event {
                Event::Record(record) => self.kept.borrow_mut().push(record),
                Event::Warning(warning) => self.warn(warning),
                _ => {}
            }
            return Ok(());
        };
        let line = event.line();
        anyhow::ensure!(
            line.len() <= crate::event::MAX_LINE,
            "an event of {} bytes is longer than meta reads",
            line.len()
        );
        let mut file = events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        file.write_all(line.as_bytes())?;
        Ok(file.flush()?)
    }

    /// A record of the outcome, shown as it arrives.
    pub fn record(&self, record: crate::ProjectResult) -> anyhow::Result<()> {
        self.emit(Event::Record(record))
    }

    /// How far along the command is (`done` of `total`).
    pub fn progress(&self, done: u64, total: Option<u64>) -> anyhow::Result<()> {
        self.emit(Event::Progress(crate::event::Progress {
            done,
            total,
            ..Default::default()
        }))
    }

    /// A problem the command goes on despite: meta shows it, or lists it in
    /// the Envelope's `warnings`.
    pub fn warn(&self, warning: impl Into<Warning>) {
        self.warnings.borrow_mut().push(warning.into());
    }

    /// [`Ctx::warn`], or with `--strict` (warnings are errors) the error
    /// `<message> (strict mode)`.
    pub fn warn_or_bail(&self, warning: impl Into<Warning>) -> anyhow::Result<()> {
        let warning = warning.into();
        if self.meta.strict {
            anyhow::bail!("{} (strict mode)", warning.message);
        }
        self.warn(warning);
        Ok(())
    }

    /// Whether this is a dry run: change nothing.
    pub fn dry_run(&self) -> bool {
        self.meta.dry_run
    }

    /// meta's options as the plugin may act on them.
    pub fn meta(&self) -> &crate::Meta {
        &self.meta
    }

    /// `response`, with the warnings reported so far before its own, and
    /// the records no channel took before its records.
    pub fn finish(&self, mut response: Response) -> Response {
        let mut warnings = std::mem::take(&mut *self.warnings.borrow_mut());
        warnings.append(&mut response.warnings);
        response.warnings = warnings;
        let mut kept = std::mem::take(&mut *self.kept.borrow_mut());
        if !kept.is_empty() {
            match &mut response.answer {
                Answer::Results(results) => {
                    kept.append(&mut results.records);
                    results.records = kept;
                }
                Answer::Message(_) => {
                    response.answer = Answer::Results(crate::Results {
                        records: kept,
                        text: crate::Text::Plugin,
                    })
                }
                _ => {}
            }
        }
        response
    }
}

/// A plugin: what it says about itself (`--meta-plugin-info`), and what
/// answers a call.
pub struct PluginDefinition {
    pub info: PluginInfo,
    pub execute: fn(&Ctx, Request) -> anyhow::Result<Response>,
    /// What a line acts on (its words, from the plugin's word on), for
    /// `--meta-plugin-targeting`; none: the host never asks, and sends the
    /// projects meta's options select.
    pub targeting: Option<fn(&[String]) -> crate::Targeting>,
}

/// Run a plugin: answer `--meta-plugin-info` and `--meta-plugin-exec`, and
/// end with the exit code its response carries (0, 1, or 2 for a wrong
/// command line). Initializes `env_logger` (`RUST_LOG`).
pub fn run_plugin(plugin: PluginDefinition) {
    env_logger::init();
    crate::out::color();
    let code = crate::exit::code(crate::exit::flushed(serve(&plugin)));
    if code != 0 {
        crate::exit::now(code);
    }
}

fn serve(plugin: &PluginDefinition) -> anyhow::Result<()> {
    use crate::display::escape;
    let usage = |problem: String| {
        anyhow::Error::new(crate::exit::Usage(format!(
            "{problem}This binary is a meta plugin. Use via: meta {}",
            escape(&plugin.info.name)
        )))
    };
    let flag = std::env::args_os()
        .nth(1)
        .ok_or_else(|| usage(String::new()))?;
    match flag.to_str() {
        Some("--meta-plugin-info") => {
            let info = crate::PluginInfo {
                answers_targeting: plugin.targeting.is_some(),
                ..plugin.info.clone()
            };
            crate::outln!("{}", serde_json::to_string(&info)?)?;
            Ok(())
        }
        Some(flag @ "--meta-plugin-targeting") => {
            let Some(targeting) = plugin.targeting else {
                return Err(usage(format!("Unknown flag: {flag}\n")));
            };
            let words: Vec<String> = std::env::args_os()
                .skip(2)
                .map(|word| word.to_string_lossy().into_owned())
                .collect();
            crate::outln!("{}", serde_json::to_string(&targeting(&words))?)?;
            Ok(())
        }
        Some("--meta-plugin-exec") => execute(plugin),
        Some("--help" | "-h") => Ok(crate::out!(
            "{}",
            crate::help_text(&plugin.info.name, &plugin.info)
        )?),
        _ => Err(usage(format!(
            "Unknown flag: {}\n",
            escape(&flag.to_string_lossy())
        ))),
    }
}

/// `--meta-plugin-exec`: answer the request in `META_REQUEST`.
fn execute(plugin: &PluginDefinition) -> anyhow::Result<()> {
    use anyhow::Context;
    #[cfg(feature = "process")]
    if let Err(e) = crate::process::own() {
        crate::out::warn(format!(
            "cannot pass signals on to the programs this plugin starts: {e}"
        ));
    }
    // a plugin's call ends with its meta
    #[cfg(all(unix, feature = "process"))]
    crate::process::end_with_parent();
    let var = |name: &str| {
        std::env::var_os(name)
            .with_context(|| format!("{name} is not set: meta starts plugins with it"))
    };
    let (request_file, response_file) = (var(REQUEST_VAR)?, var(RESPONSE_VAR)?);
    let text = std::fs::read(&request_file).context("Failed to read the plugin request")?;
    let request: Request =
        serde_json::from_slice(&text).context("Failed to parse plugin request")?;
    crate::out::set_json(request.meta.json);
    let ctx = match std::env::var_os(crate::event::EVENTS_VAR) {
        Some(path) => {
            let events = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .context("Failed to open the events file meta named")?;
            Ctx::streaming(request.meta.clone(), events)
        }
        None => Ctx::new(request.meta.clone()),
    };
    let response = match (plugin.execute)(&ctx, request) {
        Ok(response) => response,
        // a closed stdout ends the plugin as it ends meta
        Err(e) if crate::exit::ending(&e) == Some(crate::exit::CLOSED_STDOUT) => return Err(e),
        Err(e) => Response::error(info_of(&e)),
    };
    let response = ctx.finish(response);
    let code = match &response.answer {
        Answer::Error(error) => error.code.exit_code(),
        _ => crate::exit::SUCCESS,
    };
    let json = serde_json::to_vec(&response).context("Failed to write the plugin response")?;
    std::fs::write(&response_file, json).context("Failed to write the plugin response")?;
    match code {
        crate::exit::SUCCESS => Ok(()),
        code => Err(crate::exit::ExitWith(code).into()),
    }
}

/// The error for a plugin that can't do what was asked with what it got
/// (a missing argument the host should have caught).
pub fn usage(message: impl Into<String>) -> anyhow::Error {
    ErrorInfo::new(ErrorCode::Usage, message).into()
}
