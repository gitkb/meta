//! How long a worktree set has left.

use meta_core::display::{escape, escape_path};
use meta_core::set_record::SetRecord;

/// Seconds until `record`'s TTL expires: `None` without a TTL, negative
/// once expired. A malformed `created_at` is an error: when the set
/// expires is unknown.
pub fn remaining(record: &SetRecord, now_epoch: i64) -> anyhow::Result<Option<i64>> {
    let Some(ttl) = record.ttl_seconds else {
        return Ok(None);
    };
    let created = chrono::DateTime::parse_from_rfc3339(&record.created_at)
        .map_err(|e| {
            anyhow::anyhow!(
                "Malformed created_at '{}' in the record of '{}' ({e}): its TTL is unknown",
                escape(&record.created_at),
                escape_path(&record.set)
            )
        })?
        .timestamp();
    let ttl = i64::try_from(ttl).unwrap_or(i64::MAX);
    Ok(Some(created.saturating_add(ttl).saturating_sub(now_epoch)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn record(created_at: &str, ttl_seconds: Option<u64>) -> SetRecord {
        SetRecord {
            set: PathBuf::from("/p/.worktrees/test-wt"),
            project: PathBuf::from("/p"),
            created_at: created_at.to_string(),
            ephemeral: false,
            ttl_seconds,
            custom: HashMap::new(),
        }
    }

    #[test]
    fn ttl_remaining() {
        // 2025-01-01T00:00:00Z is epoch 1735689600
        let created = 1_735_689_600i64;
        let r = record("2025-01-01T00:00:00Z", Some(3600));
        assert_eq!(remaining(&r, created + 1800).unwrap(), Some(1800));
        assert_eq!(remaining(&r, created + 3600).unwrap(), Some(0));
        assert_eq!(remaining(&r, created + 7200).unwrap(), Some(-3600));
        assert_eq!(
            remaining(&record("2025-01-01T00:00:00Z", None), 0).unwrap(),
            None
        );
        let huge = record("2025-01-01T00:00:00Z", Some(u64::MAX));
        assert_eq!(
            remaining(&huge, 1_800_000_000).unwrap(),
            Some(i64::MAX - 1_800_000_000)
        );
        let malformed = record("not-a-date", Some(3600));
        assert!(remaining(&malformed, 1_700_000_000).is_err());
    }
}
