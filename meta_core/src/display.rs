//! Escaping for untrusted text in human-readable output.
//!
//! Names, paths, tags and URLs come from `.meta.yaml` files, git and the filesystem.
//! Printed raw, a control character in one of them can move the cursor, clear
//! the screen, set the window title or the clipboard, or break a line-based
//! format such as the `meta context` table. Every human-readable printer
//! passes such values through [`escape`]; `--json` output needs nothing more,
//! since JSON already escapes them.

use std::borrow::Cow;
use std::fmt::Write;

/// True for characters that must never reach a terminal raw: C0 and C1
/// controls and DEL; the Unicode bidirectional controls, which reorder how a
/// line is displayed (U+061C, U+200E-F, U+202A-E, U+2066-9); the line and
/// paragraph separators (U+2028-9), which many renderers treat as line
/// breaks; and invisible characters that hide text from a reader or make one
/// value look like another (soft hyphen, combining grapheme joiner, Hangul
/// fillers, Mongolian vowel separator, zero-width characters (not the joiner, which emoji sequences use), word joiners
/// and deprecated format controls, BOM, interlinear
/// annotation controls, tag characters).
pub fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{AD}'
                | '\u{034F}'
                | '\u{061C}'
                | '\u{115F}'..='\u{1160}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200C}'
                | '\u{200E}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{3164}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{E0000}'..='\u{E007F}'

        )
}

/// Return `s` with every unsafe character written as a visible escape
/// (`\n`, `\t`, `\r`, or `\u{1b}`). Borrows `s` unchanged when it is clean.
pub fn escape(s: &str) -> Cow<'_, str> {
    if !s.chars().any(is_unsafe_char) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if is_unsafe_char(c) => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// [`escape`] for multi-line text such as an error message: newlines stay,
/// every other unsafe character is escaped.
pub fn escape_lines(s: &str) -> Cow<'_, str> {
    if !s.chars().any(|c| c != '\n' && is_unsafe_char(c)) {
        return Cow::Borrowed(s);
    }
    Cow::Owned(s.split('\n').map(escape).collect::<Vec<_>>().join("\n"))
}

/// [`escape`] for a path, which may also hold bytes that are not UTF-8.
pub fn escape_path(path: &std::path::Path) -> String {
    escape(&path.to_string_lossy()).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_is_borrowed() {
        assert!(matches!(escape("api/core-1.2 ✓"), Cow::Borrowed(_)));
    }

    #[test]
    fn terminal_sequences_are_made_visible() {
        assert_eq!(
            escape("x\u{1b}]0;PWNED\u{7}\u{1b}[2J"),
            "x\\u{1b}]0;PWNED\\u{7}\\u{1b}[2J"
        );
        assert_eq!(escape("n\nfake\r\t"), "n\\nfake\\r\\t");
        assert_eq!(escape("c1\u{9b}31m del\u{7f}"), "c1\\u{9b}31m del\\u{7f}");
        assert_eq!(escape("rtl\u{202e}txt"), "rtl\\u{202e}txt");
        assert_eq!(escape("alm\u{61c}x"), "alm\\u{61c}x");
        assert_eq!(escape("a\u{2028}## b"), "a\\u{2028}## b");
        assert_eq!(
            escape("zw\u{200b}j\u{2060}t\u{e0041}"),
            "zw\\u{200b}j\\u{2060}t\\u{e0041}"
        );
    }

    #[test]
    fn invisible_fillers_are_made_visible() {
        assert_eq!(
            escape("a\u{34f}b\u{3164}c\u{180e}d\u{206a}e\u{fff9}f"),
            "a\\u{34f}b\\u{3164}c\\u{180e}d\\u{206a}e\\u{fff9}f"
        );
        // Emoji keep their variation selector
        assert!(matches!(escape("ok \u{2714}\u{fe0f}"), Cow::Borrowed(_)));
        assert!(matches!(
            escape("\u{1f468}\u{200d}\u{1f467}"),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn escape_lines_keeps_newlines_only() {
        assert!(matches!(escape_lines("a\nb"), Cow::Borrowed(_)));
        assert_eq!(escape_lines("a\u{1b}[2J\nb\r"), "a\\u{1b}[2J\nb\\r");
    }
}
