//! Whether a command changes anything, or only says what it would change.

/// How a command runs: for real, or as a preview (`--dry-run`) that
/// changes nothing. Resolved once from the user's flags and handed to every
/// command that changes something, so none can miss it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ExecutionMode {
    #[default]
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

    /// A preview if either this or `dry_run` asks for one.
    pub fn or_dry_run(self, dry_run: bool) -> Self {
        Self::from_dry_run(self.is_preview() || dry_run)
    }
}
