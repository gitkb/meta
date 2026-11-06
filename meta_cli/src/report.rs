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
    /// as meta's error (`Error: …`); `silent` names the repos of a told one,
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
        let failed: Vec<String> = self
            .results
            .iter()
            .filter(|r| r.error.is_some())
            .map(|r| meta_plugin::display::escape(r.project.as_deref().unwrap_or(".")).into_owned())
            .collect();
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
            (false, true, true) => anyhow::bail!("{}", loop_lib::failed_line(&failed)),
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
