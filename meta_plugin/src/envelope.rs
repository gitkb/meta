//! What `meta --json` prints, for every command: one [`Envelope`].
//!
//! The host prints it; plugins never print JSON. A command's own data is in
//! `results[].data`, one row per record, so a caller reads every command
//! the same way: `success`, then `error` when it failed, else the rows.

use serde::{Deserialize, Serialize};
use std::fmt;

/// The one JSON document `meta --json …` prints.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    /// Whether the command did what it was asked: false exactly when
    /// `error` is set.
    pub success: bool,
    /// The command's words (`git status`, `exec`, `project list`); empty
    /// when meta failed before it knew the command.
    pub command: String,
    /// One row per project a step ran in, or per record a command
    /// returned; a project can have several.
    #[serde(default)]
    pub results: Vec<ProjectResult>,
    /// Projects the command left out, and why (none of them is a failure).
    #[serde(default)]
    pub skipped: Vec<Skipped>,
    #[serde(default)]
    pub warnings: Vec<Warning>,
    /// What a command that answers in words said (a preview, "nothing to
    /// commit"), when it returned no rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
}

/// One row of an [`Envelope`]: what happened in one project, or one record
/// a command returned (with no project when it is about the workspace as a
/// whole). A step that ran fills `argv` to `stderr`; a record fills
/// `data`. `error` is set exactly when the row failed: a step that ended
/// with a code it doesn't accept (`tool-failed`), or a project's own error.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectResult {
    /// The project, by its path from the workspace root as `--include`
    /// takes it (`.` for the root)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Its folder, absolute (shown as text where it is not UTF-8)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `path`'s bytes in base64, when they are not UTF-8: the folder itself
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_bytes: Option<String>,
    /// The program and arguments a step ran (or would run, in a dry run)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// How the step ended: null when a signal ended it (`signal`), or when
    /// it never ran (a dry run)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    /// What the step wrote (lossy UTF-8)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    /// The command's own data about this project
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Why this project failed, when the command went on with the others
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
}

impl ProjectResult {
    /// A record about `project` (its path from the root) holding `data`.
    pub fn record(project: impl Into<String>, data: impl Serialize) -> Self {
        ProjectResult {
            project: Some(project.into()),
            data: Some(to_value(data)),
            ..Self::default()
        }
    }

    /// A record about the workspace as a whole, holding `data`.
    pub fn about_workspace(data: impl Serialize) -> Self {
        ProjectResult {
            data: Some(to_value(data)),
            ..Self::default()
        }
    }

    /// `project`'s own failure.
    pub fn error(project: impl Into<String>, error: ErrorInfo) -> Self {
        ProjectResult {
            project: Some(project.into()),
            error: Some(error),
            ..Self::default()
        }
    }

    /// Set `path` (and `path_bytes` when it is not UTF-8) from `dir`.
    pub fn at(mut self, dir: &std::path::Path) -> Self {
        self.path = Some(dir.to_string_lossy().into_owned());
        self.path_bytes = crate::wire_path::bytes_of(dir);
        self
    }

    /// Whether this row is a failure: a step that ended badly (its
    /// `error` says how), or a project's own error.
    pub fn is_failure(&self) -> bool {
        self.error.is_some()
    }
}

/// `value` as JSON: a type that can't be JSON (a map with non-string keys)
/// is shown as its error, never dropped.
fn to_value(value: impl Serialize) -> serde_json::Value {
    serde_json::to_value(value)
        .unwrap_or_else(|e| serde_json::Value::String(format!("(not JSON: {e})")))
}

/// A project a command left out on purpose, and why. All projects skipped
/// is still success.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skipped {
    /// Its path from the workspace root (`.` for the root)
    pub project: String,
    pub reason: String,
    /// Not worth a line in the text output (the reason is expected, like
    /// "no Cargo.toml"); still listed here
    #[serde(default)]
    pub quiet: bool,
}

/// A problem the command went on despite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Warning {
    pub message: String,
    /// The project it is about, by its path from the workspace root
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

impl Warning {
    pub fn new(message: impl Into<String>) -> Self {
        Warning {
            message: message.into(),
            project: None,
        }
    }

    pub fn about(project: impl Into<String>, message: impl Into<String>) -> Self {
        Warning {
            message: message.into(),
            project: Some(project.into()),
        }
    }
}

impl From<String> for Warning {
    fn from(message: String) -> Self {
        Warning::new(message)
    }
}

impl From<&str> for Warning {
    fn from(message: &str) -> Self {
        Warning::new(message)
    }
}

/// What kind of failure: a closed set a caller can act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    /// The command line is wrong (exit code 2)
    Usage,
    /// Something it needs isn't there: a config, a project, a set
    NotFound,
    /// It would clash with what is there (a set that exists, a lease held)
    Conflict,
    /// A repo has work that the command would lose, so it refused
    Dirty,
    /// A plugin ended without a usable answer
    PluginFailed,
    /// A program the command ran failed in at least one project
    ToolFailed,
    /// Stopped by a signal (Ctrl-C)
    Cancelled,
    /// A program wrote more output than meta keeps of one
    /// ([`crate::process::CAPTURE_LIMIT`]): it was stopped
    TooMuchOutput,
    /// A service refused the credentials, or none were given
    Auth,
    /// A service couldn't be reached
    Network,
    /// A service asked to wait (`retry_after` seconds)
    RateLimited,
    /// Anything else meta itself failed at
    Failed,
}

impl ErrorCode {
    /// The exit code a command ending with this error ends with.
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::Usage => crate::exit::USAGE,
            ErrorCode::Cancelled => crate::exit::signalled(2),
            _ => crate::exit::FAILURE,
        }
    }

    /// The name on the wire (`not-found`).
    pub fn name(self) -> &'static str {
        match self {
            ErrorCode::Usage => "usage",
            ErrorCode::NotFound => "not-found",
            ErrorCode::Conflict => "conflict",
            ErrorCode::Dirty => "dirty",
            ErrorCode::PluginFailed => "plugin-failed",
            ErrorCode::ToolFailed => "tool-failed",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::TooMuchOutput => "too-much-output",
            ErrorCode::Auth => "auth",
            ErrorCode::Network => "network",
            ErrorCode::RateLimited => "rate-limited",
            ErrorCode::Failed => "failed",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why a command (or one project of it) failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorInfo {
    pub code: ErrorCode,
    pub message: String,
    /// What to do about it: the line to run instead, say
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// The word or value that was refused
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected: Option<String>,
    /// For `rate-limited`: how many seconds to wait
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<u64>,
}

impl ErrorInfo {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        ErrorInfo {
            code,
            message: message.into(),
            hint: None,
            rejected: None,
            retry_after: None,
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for ErrorInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// An [`ErrorInfo`] as an error: a plugin or meta returns it to end with
/// that code (`Err(ErrorInfo::new(ErrorCode::Dirty, "…").into())`).
impl std::error::Error for ErrorInfo {}

/// The code of `error`: the [`ErrorInfo`] in its chain, else `usage` for a
/// [`crate::exit::Usage`], else `failed`.
pub fn code_of(error: &anyhow::Error) -> ErrorCode {
    // as context, it isn't one of the chain's causes
    if let Some(info) = error.downcast_ref::<ErrorInfo>() {
        return info.code;
    }
    error
        .chain()
        .find_map(|cause| {
            if let Some(info) = cause.downcast_ref::<ErrorInfo>() {
                return Some(info.code);
            }
            if cause
                .downcast_ref::<std::io::Error>()
                .and_then(crate::process::TooMuchOutput::of)
                .is_some()
            {
                return Some(ErrorCode::TooMuchOutput);
            }
            cause.is::<crate::exit::Usage>().then_some(ErrorCode::Usage)
        })
        .unwrap_or(ErrorCode::Failed)
}

/// `error` as an [`ErrorInfo`]: its own when it carries one (its context
/// added to the message), else one of [`code_of`] with the whole chain as
/// its message.
pub fn info_of(error: &anyhow::Error) -> ErrorInfo {
    let message = format!("{error:#}");
    let found = error
        .downcast_ref::<ErrorInfo>()
        .or_else(|| error.chain().find_map(|c| c.downcast_ref::<ErrorInfo>()));
    match found {
        Some(info) => ErrorInfo {
            message,
            ..info.clone()
        },
        None => ErrorInfo::new(code_of(error), message),
    }
}

impl Envelope {
    /// A failed command's envelope: `error`, and nothing else.
    pub fn failed(command: impl Into<String>, error: ErrorInfo) -> Self {
        Envelope {
            success: false,
            command: command.into(),
            error: Some(error),
            ..Envelope::default()
        }
    }

    /// Print it on stdout, as `meta --json` does: pretty-printed, with only
    /// the row fields `fields` names when it names any.
    pub fn print(&self, fields: &[String]) -> Result<(), crate::out::StdoutError> {
        let value = match fields.is_empty() {
            true => serde_json::to_value(self).unwrap_or_default(),
            false => self.select(fields),
        };
        let text = serde_json::to_string_pretty(&value).unwrap_or_default();
        crate::out::document(&text)
    }

    /// Print it as the last line of `--json-stream`: `{"envelope": …}`.
    pub fn print_last(&self, fields: &[String]) -> Result<(), crate::out::StdoutError> {
        let value = match fields.is_empty() {
            true => serde_json::to_value(self).unwrap_or_default(),
            false => self.select(fields),
        };
        crate::out::document(&serde_json::json!({ "envelope": value }).to_string())
    }

    /// The envelope as `meta --fields a,b.c` prints it: each row keeps only
    /// the fields named (a dotted name reaches into `data`). Every other
    /// part stays, so a caller still sees success and errors.
    pub fn select(&self, fields: &[String]) -> serde_json::Value {
        let mut value = serde_json::to_value(self).unwrap_or_default();
        if let Some(rows) = value.get_mut("results").and_then(|r| r.as_array_mut()) {
            for row in rows.iter_mut() {
                *row = pick(row, fields);
            }
        }
        value
    }
}

/// The fields of `row` that `fields` names, at the places they were.
fn pick(row: &serde_json::Value, fields: &[String]) -> serde_json::Value {
    let mut out = serde_json::Value::Object(serde_json::Map::new());
    for field in fields {
        let path: Vec<&str> = field.split('.').collect();
        if let Some(found) = path.iter().try_fold(row, |value, key| value.get(*key)) {
            put(&mut out, &path, found.clone());
        }
    }
    out
}

fn put(out: &mut serde_json::Value, path: &[&str], value: serde_json::Value) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut at = out;
    for key in parents {
        let Some(map) = at.as_object_mut() else {
            return;
        };
        at = map
            .entry(key.to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    }
    if let Some(map) = at.as_object_mut() {
        map.insert(last.to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_keep_only_the_named_parts_of_each_row() {
        let envelope = Envelope {
            success: true,
            command: "project list".to_string(),
            results: vec![ProjectResult::record(
                "api",
                serde_json::json!({"name": "api", "tags": ["rust"], "repo": "u"}),
            )
            .at(std::path::Path::new("/w/api"))],
            ..Envelope::default()
        };
        let shown = envelope.select(&["path".to_string(), "data.tags".to_string()]);
        assert_eq!(
            shown["results"],
            serde_json::json!([{"path": "/w/api", "data": {"tags": ["rust"]}}])
        );
        assert_eq!(shown["success"], true);
    }

    #[test]
    fn an_error_keeps_its_code_through_the_context_added_to_it() {
        let error = anyhow::Error::new(ErrorInfo::new(ErrorCode::Dirty, "api has changes"))
            .context("removing set t");
        let info = info_of(&error);
        assert_eq!(info.code, ErrorCode::Dirty);
        assert_eq!(info.message, "removing set t: api has changes");
        assert_eq!(
            code_of(&crate::exit::Usage("x".into()).into()),
            ErrorCode::Usage
        );
        assert_eq!(code_of(&anyhow::anyhow!("x")), ErrorCode::Failed);
        let wire = serde_json::to_value(ErrorInfo::new(ErrorCode::RateLimited, "wait")).unwrap();
        assert_eq!(wire["code"], "rate-limited");
    }
}
