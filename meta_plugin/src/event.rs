//! What a command reports while it runs: one [`Event`] per line, as it
//! happens.
//!
//! A plugin writes events to the file `META_EVENTS` names (one JSON object
//! per line, at most [`MAX_LINE`] bytes): `Ctx::{emit, record, progress}` in
//! a Rust plugin, `printf '%s\n' "$json" >> "$META_EVENTS"` in a script. On
//! Unix it is a FIFO, so a writer waits while meta is behind and nothing
//! piles up; on Windows a file meta follows. meta shows the events as they
//! come: records and warnings in text, `{"event": …}` lines under
//! `--json-stream`; under `--json` it folds them into its one Envelope.
//! A call still ends with its one [`crate::Response`]: there is no result
//! or error event.

use crate::envelope::{ProjectResult, Warning};
use serde::{Deserialize, Serialize};

/// The variable naming where a plugin writes its events.
pub const EVENTS_VAR: &str = "META_EVENTS";

/// The longest event line meta reads; a longer one fails the call
/// (`plugin-failed`).
pub const MAX_LINE: usize = 64 * 1024;

/// One thing a command reports while it runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A record, as one of `Results.records`: it is part of the outcome
    /// (the Envelope's rows), shown as it arrives
    Record(ProjectResult),
    /// How far along the command is: shown on a terminal only, never part
    /// of the outcome
    Progress(Progress),
    /// A problem the command goes on despite, as one of `warnings`
    Warning(Warning),
    /// One step of a plan ended (meta's own: a plugin that writes one fails)
    Step(ProjectResult),
}

/// How far along a command is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub done: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    /// The project it is at, by its path from the workspace root
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl Event {
    /// `line` (one line of the events file) as an event, or why it is none.
    pub fn parse(line: &[u8]) -> Result<Event, String> {
        if line.len() > MAX_LINE {
            return Err(format!(
                "an event line of {} bytes (at most {MAX_LINE})",
                line.len()
            ));
        }
        serde_json::from_slice(line).map_err(|e| format!("an event line that is no event ({e})"))
    }

    /// The event as one line, with its newline.
    pub fn line(&self) -> String {
        let mut line = serde_json::to_string(self).unwrap_or_default();
        line.push('\n');
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_event_is_one_line_of_its_type_and_fields() {
        let record = Event::Record(ProjectResult::record("api", serde_json::json!({"line": 3})));
        let wire: serde_json::Value = serde_json::from_str(&record.line()).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"type": "record", "project": "api", "data": {"line": 3}})
        );
        for event in [
            record,
            Event::Progress(Progress {
                done: 1,
                total: Some(3),
                ..Progress::default()
            }),
            Event::Warning(Warning::about("api", "slow")),
        ] {
            let line = event.line();
            assert!(line.ends_with('\n') && !line.trim_end().contains('\n'));
            assert_eq!(Event::parse(line.trim_end().as_bytes()), Ok(event));
        }
        assert!(Event::parse(br#"{"type":"result"}"#).is_err());
        assert!(Event::parse(&vec![b' '; MAX_LINE + 1]).is_err());
    }
}
