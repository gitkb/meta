//! What a plugin answers: one [`Response`], written to the file the host
//! names in `META_RESPONSE`.

use crate::envelope::{ErrorInfo, ProjectResult, Skipped, Warning};
use crate::wire_path;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A plugin's answer: what it is, and the warnings and skipped projects it
/// comes with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    #[serde(flatten)]
    pub answer: Answer,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<Warning>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<Skipped>,
}

/// What a plugin answers with.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// Programs for the host to run, one step per project
    Plan(Plan),
    /// What the plugin found or did, as records
    Results(Results),
    /// Words for the user (a preview, "nothing to commit")
    Message(String),
    /// Run this meta command line instead (once: a delegate's own delegate
    /// is refused)
    Delegate(Vec<String>),
    /// The command failed
    Error(ErrorInfo),
}

/// Programs for the host to run: with its loop engine, `-j` and
/// `--sequential`, `--dry-run` (shown, not run) and the Envelope.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub steps: Vec<Step>,
    /// Whether the steps may run at once; none: as meta's options say
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel: Option<bool>,
    /// At most this many at once (a shared resource's limit)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<usize>,
    /// Milliseconds between two step starts
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stagger_ms: Option<u64>,
}

/// One program to run in one folder, with no shell in between.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// The project it runs for, by its path from the workspace root
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Where it runs
    #[serde(with = "wire_path")]
    pub dir: PathBuf,
    /// The program and its arguments, as given
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// The exit codes that are success (default: 0 alone)
    #[serde(default = "zero", skip_serializing_if = "is_zero")]
    pub ok_codes: Vec<i32>,
    /// Variables of `env` whose values are secret: never shown in a dry
    /// run, the Envelope or a log. Every plugin, and every step, inherits
    /// the user's environment; a secret a step needs that isn't there yet
    /// goes in `env` and is named here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secret_env: Vec<String>,
}

fn zero() -> Vec<i32> {
    vec![0]
}

fn is_zero(codes: &Vec<i32>) -> bool {
    codes.as_slice() == [0]
}

impl Step {
    /// `argv` in `dir`, for `project` (its path from the root).
    pub fn new(project: Option<&str>, dir: impl Into<PathBuf>, argv: Vec<String>) -> Self {
        Step {
            project: project.map(str::to_string),
            dir: dir.into(),
            argv,
            env: BTreeMap::new(),
            ok_codes: zero(),
            secret_env: Vec::new(),
        }
    }

    /// `argv` in each selected project of `request`.
    pub fn each(request: &crate::Request, argv: &[String]) -> Vec<Step> {
        request
            .projects
            .iter()
            .map(|p| Step::new(Some(&p.path), &p.abs, argv.to_vec()))
            .collect()
    }
}

/// What a plugin found or did.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Results {
    /// One per thing found; a project may have several, the workspace as a
    /// whole has those with no project
    #[serde(default)]
    pub records: Vec<ProjectResult>,
    /// Who shows them in the text output
    #[serde(default, skip_serializing_if = "Text::is_host")]
    pub text: Text,
}

/// Who shows a command's results in the text output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Text {
    /// The host, from the records: escaped, under project headers
    #[default]
    Host,
    /// The plugin printed them itself (a table); the host adds nothing
    Plugin,
}

impl Text {
    fn is_host(&self) -> bool {
        *self == Text::Host
    }
}

impl Response {
    fn of(answer: Answer) -> Self {
        Response {
            answer,
            warnings: Vec::new(),
            skipped: Vec::new(),
        }
    }

    pub fn plan(plan: Plan) -> Self {
        Self::of(Answer::Plan(plan))
    }

    /// Records the host shows itself.
    pub fn results(records: Vec<ProjectResult>) -> Self {
        Self::of(Answer::Results(Results {
            records,
            text: Text::Host,
        }))
    }

    /// Records for `--json`, the plugin having shown them in its own words.
    pub fn printed(records: Vec<ProjectResult>) -> Self {
        Self::of(Answer::Results(Results {
            records,
            text: Text::Plugin,
        }))
    }

    pub fn message(text: impl Into<String>) -> Self {
        Self::of(Answer::Message(text.into()))
    }

    /// Nothing to show: no records, no words.
    pub fn done() -> Self {
        Self::printed(Vec::new())
    }

    pub fn delegate(args: Vec<String>) -> Self {
        Self::of(Answer::Delegate(args))
    }

    pub fn error(error: ErrorInfo) -> Self {
        Self::of(Answer::Error(error))
    }

    /// This response with `warning` too.
    pub fn warn(mut self, warning: impl Into<Warning>) -> Self {
        self.warnings.push(warning.into());
        self
    }

    /// This response with `skipped` listed.
    pub fn skipping(mut self, skipped: Vec<Skipped>) -> Self {
        self.skipped.extend(skipped);
        self
    }
}

impl From<Plan> for Response {
    fn from(plan: Plan) -> Self {
        Response::plan(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_answer_is_one_key_beside_its_warnings() {
        let plan = Response::plan(Plan {
            steps: vec![Step::new(Some("api"), "/w/api", vec!["true".into()])],
            ..Plan::default()
        })
        .warn("b is not cloned");
        let wire = serde_json::to_value(&plan).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "plan": {"steps": [{"project": "api", "dir": "/w/api", "argv": ["true"]}]},
                "warnings": [{"message": "b is not cloned"}],
            })
        );
        let back: Response = serde_json::from_value(wire).unwrap();
        let codes = match &back.answer {
            Answer::Plan(plan) => plan.steps.first().map(|s| s.ok_codes.clone()),
            _ => None,
        };
        assert_eq!(codes, Some(vec![0]));
        for written in [
            serde_json::json!({"message": "nothing to commit"}),
            serde_json::json!({"delegate": ["git", "status"]}),
            serde_json::json!({"error": {"code": "dirty", "message": "api has changes"}}),
            serde_json::json!({"results": {"records": [{"project": "api", "data": 1}]}}),
        ] {
            let back: Response = serde_json::from_value(written.clone()).unwrap();
            assert_eq!(serde_json::to_value(back).unwrap(), written);
        }
    }
}
