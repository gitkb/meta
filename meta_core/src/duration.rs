//! Human-friendly durations: `30s`, `5m`, `2h`, `1d`, `1w`.

use crate::display::escape;
use anyhow::Context;

/// Parse a human-friendly duration string to seconds.
/// Supported formats: "30s", "5m", "1h", "2d", "1w" (any case), or bare seconds "300".
/// The result always fits in an `i64`, so timestamp arithmetic can't wrap.
pub fn parse_duration(s: &str) -> anyhow::Result<u64> {
    let s = s.trim().to_ascii_lowercase();
    let s = s.as_str();
    if s.is_empty() {
        anyhow::bail!("Empty duration string");
    }

    // Bare number: treat as seconds
    let (num_str, multiplier) = if s.chars().all(|c| c.is_ascii_digit()) {
        (s, 1)
    } else {
        let suffix_at = s.len() - s.chars().last().map_or(0, char::len_utf8);
        let (num_str, suffix) = s.split_at(suffix_at);
        let multiplier = match suffix {
            "s" => 1,
            "m" => 60,
            "h" => 3600,
            "d" => 86400,
            "w" => 604800,
            _ => anyhow::bail!(
                "Invalid duration suffix '{}'. Use s (seconds), m (minutes), h (hours), d (days), or w (weeks)",
                escape(suffix)
            ),
        };
        (num_str, multiplier)
    };
    let num: u64 = num_str.parse().with_context(|| {
        format!(
            "Invalid duration '{}': use a number with an optional s, m, h, d or w suffix",
            escape(s)
        )
    })?;

    num.checked_mul(multiplier)
        .filter(|secs| i64::try_from(*secs).is_ok())
        .ok_or_else(|| anyhow::anyhow!("Duration '{}' is too large", escape(s)))
}

/// Format seconds into a human-friendly duration string.
/// Returns the largest appropriate unit (e.g., "2h" not "7200s").
pub fn format_duration(secs: i64) -> String {
    let abs_secs = secs.unsigned_abs();
    let prefix = if secs < 0 { "-" } else { "" };

    if abs_secs >= 604800 && abs_secs.is_multiple_of(604800) {
        let weeks = abs_secs / 604800;
        format!("{prefix}{weeks}w")
    } else if abs_secs >= 86400 && abs_secs.is_multiple_of(86400) {
        let days = abs_secs / 86400;
        format!("{prefix}{days}d")
    } else if abs_secs >= 3600 && abs_secs.is_multiple_of(3600) {
        let hours = abs_secs / 3600;
        format!("{prefix}{hours}h")
    } else if abs_secs >= 60 && abs_secs.is_multiple_of(60) {
        let mins = abs_secs / 60;
        format!("{prefix}{mins}m")
    } else {
        format!("{prefix}{abs_secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_unit() {
        for (input, secs) in [
            ("300", 300),
            ("30s", 30),
            ("5m", 300),
            ("  2h ", 7200),
            ("24H", 86400),
            ("1d", 86400),
            ("2w", 1_209_600),
            ("9223372036854775807", i64::MAX as u64),
        ] {
            assert_eq!(parse_duration(input).unwrap(), secs, "{input}");
        }
    }

    #[test]
    fn rejects_what_is_not_a_duration() {
        for input in [
            "",
            "   ",
            "10x",
            "abcs",
            "5é",
            "-5m",
            "9223372036854775808",
            "99999999999999999w",
        ] {
            assert!(parse_duration(input).is_err(), "{input}");
        }
    }

    #[test]
    fn formats_in_the_largest_exact_unit() {
        for (secs, text) in [
            (45, "45s"),
            (90, "90s"),
            (300, "5m"),
            (7200, "2h"),
            (86400, "1d"),
            (604800, "1w"),
            (0, "0s"),
            (-3600, "-1h"),
        ] {
            assert_eq!(format_duration(secs), text);
            if secs > 0 {
                assert_eq!(parse_duration(text).unwrap(), secs as u64);
            }
        }
    }
}
