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
use crate::response::{Answer, Response};
use crate::{PluginInfo, Request};
use std::cell::RefCell;

/// The variable naming the file the request is in.
pub const REQUEST_VAR: &str = "META_REQUEST";
/// The variable naming the file the response goes to.
pub const RESPONSE_VAR: &str = "META_RESPONSE";

/// What a plugin reports while it works, collected into its response.
pub struct Ctx {
    meta: crate::Meta,
    warnings: RefCell<Vec<Warning>>,
}

impl Ctx {
    /// A context for a call with `meta`'s options.
    pub fn new(meta: crate::Meta) -> Self {
        Ctx {
            meta,
            warnings: RefCell::new(Vec::new()),
        }
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

    /// `response`, with the warnings reported so far before its own.
    pub fn finish(&self, mut response: Response) -> Response {
        let mut warnings = std::mem::take(&mut *self.warnings.borrow_mut());
        warnings.append(&mut response.warnings);
        response.warnings = warnings;
        response
    }
}

/// A plugin: what it says about itself (`--meta-plugin-info`), and what
/// answers a call.
pub struct PluginDefinition {
    pub info: PluginInfo,
    pub execute: fn(&Ctx, Request) -> anyhow::Result<Response>,
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
            crate::outln!("{}", serde_json::to_string(&plugin.info)?)?;
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
    let var = |name: &str| {
        std::env::var_os(name)
            .with_context(|| format!("{name} is not set: meta starts plugins with it"))
    };
    let (request_file, response_file) = (var(REQUEST_VAR)?, var(RESPONSE_VAR)?);
    let text = std::fs::read(&request_file).context("Failed to read the plugin request")?;
    let request: Request =
        serde_json::from_slice(&text).context("Failed to parse plugin request")?;
    crate::out::set_json(request.meta.json);
    let ctx = Ctx::new(request.meta.clone());
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
