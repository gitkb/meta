//! Whether a command changes anything, or only says what it would change.

/// How a command runs: for real, or as a preview (`--dry-run`) that
/// changes nothing. Resolved once from the user's flags and handed to every
/// command that changes something, so none can miss it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Execute,
    Preview,
}

impl ExecutionMode {
    /// `Preview` when `dry_run` is set.
    pub fn from_dry_run(dry_run: bool) -> Self {
        if dry_run {
            Self::Preview
        } else {
            Self::Execute
        }
    }

    pub fn is_preview(self) -> bool {
        self == Self::Preview
    }
}
