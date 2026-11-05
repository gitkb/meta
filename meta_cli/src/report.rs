//! What a command did, gathered for the one Envelope `meta --json` prints,
//! and how meta ends after it.

use meta_plugin::{Envelope, ErrorInfo, ProjectResult, Skipped, Warning};

/// A command's outcome as it is gathered: its rows, what it skipped and
/// warned of, its words, and how it failed.
#[derive(Default)]
pub struct Report {
    pub command: String,
    pub results: Vec<ProjectResult>,
    pub skipped: Vec<Skipped>,
    pub warnings: Vec<Warning>,
    pub message: Option<String>,
    error: Option<ErrorInfo>,
    /// The failure was already told in the text output (the summary of the
    /// commands that failed): text mode says no more of it
    told: bool,
}

impl Report {
    pub fn new(command: impl Into<String>) -> Self {
        Report {
            command: command.into(),
            ..Report::default()
        }
    }

    /// The command failed with `error`; `told` when the text output already
    /// said so.
    pub fn fail(&mut self, error: ErrorInfo, told: bool) {
        if self.error.is_none() {
            self.error = Some(error);
            self.told = told;
        }
    }

    pub fn envelope(self) -> Envelope {
        Envelope {
            success: self.error.is_none(),
            command: self.command,
            results: self.results,
            skipped: self.skipped,
            warnings: self.warnings,
            message: self.message,
            error: self.error,
        }
    }

    /// End the command: under `--json` print the Envelope (only the row
    /// fields `fields` names, when it names any); then the error that ends
    /// meta, if it failed. In text mode a failure not yet told is reported
    /// as meta's error (`Error: …`); `silent` says a told one once more,
    /// as the summary was not shown.
    pub fn finish(
        self,
        show: crate::events::Show,
        silent: bool,
        fields: &[String],
    ) -> anyhow::Result<()> {
        use crate::events::Show;
        let told = self.told;
        let json = show != Show::Text;
        // silenced, the first failure's name and why, as its summary line
        let first_failure = self.results.iter().find(|r| r.error.is_some()).map(|r| {
            format!(
                "{}: {}",
                // named as the summary names it: ./api, . for the root
                match r.project.as_deref() {
                    None | Some(".") => ".".to_string(),
                    Some(path) => format!("./{}", meta_plugin::display::escape(path)),
                },
                last_line(r.stderr.as_deref())
            )
        });
        let envelope = self.envelope();
        match show {
            Show::Text => {}
            Show::Json => envelope.print(fields)?,
            Show::Stream => envelope.print_last(fields)?,
        }
        let Some(error) = envelope.error else {
            return Ok(());
        };
        let code = error.code.exit_code();
        match (json, told, silent) {
            (false, true, true) => match first_failure {
                Some(why) => anyhow::bail!("At least one command failed: {why}"),
                None => anyhow::bail!("At least one command failed"),
            },
            // the hint said under the error, as one
            (false, false, _) => Err(ErrorInfo {
                message: match &error.hint {
                    Some(hint) if !error.message.contains(hint.as_str()) => {
                        format!("{}\n{hint}", error.message)
                    }
                    _ => error.message.clone(),
                },
                hint: None,
                ..error
            }
            .into()),
            _ => Err(meta_plugin::exit::ExitWith(code).into()),
        }
    }
}

/// The last line of `stderr` that has anything on it, escaped, or that
/// there was none.
fn last_line(stderr: Option<&str>) -> String {
    stderr
        .unwrap_or_default()
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map_or_else(
            || "no stderr".to_string(),
            |line| meta_plugin::display::escape(line).into_owned(),
        )
}
