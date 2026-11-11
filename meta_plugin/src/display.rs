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

/// [`escape`] for multi-line text such as an error message or git's
/// stderr: line breaks and tabs stay, every other unsafe character is
/// escaped.
pub fn escape_lines(s: &str) -> Cow<'_, str> {
    let kept = |c: char| matches!(c, '\n' | '\t');
    if !s.chars().any(|c| !kept(c) && is_unsafe_char(c)) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            c if kept(c) => out.push(c),
            c => out.push_str(&escape(c.encode_utf8(&mut [0; 4]))),
        }
    }
    Cow::Owned(out)
}

/// Another program's output (git's stderr), escaped to follow `<message>:`:
/// one line after a space; several each on a line of their own, indented
/// under the message, so none can pass for a line of meta's own.
pub fn program_output(output: &[u8]) -> String {
    let output = String::from_utf8_lossy(output);
    let text = escape_lines(output.trim());
    if !text.contains('\n') {
        return format!(" {text}");
    }
    text.lines().map(|line| format!("\n  {line}")).collect()
}

/// Whether `output` has a line with something a reader could see on it.
pub fn has_visible_line(output: &[u8]) -> bool {
    String::from_utf8_lossy(output)
        .lines()
        .any(|line| visible(&strip_sgr(line)))
}

fn visible(line: &str) -> bool {
    line.chars()
        .any(|c| !c.is_whitespace() && !is_unsafe_char(c))
}

/// `line` without its complete SGR colour sequences (`ESC [ … m`). Other
/// escape sequences stay, for [`escape`] to make visible.
fn strip_sgr(line: &str) -> Cow<'_, str> {
    if !line.contains("\u{1b}[") {
        return Cow::Borrowed(line);
    }
    let mut plain = String::with_capacity(line.len());
    let mut rest = line;
    while let Some((before, after)) = rest.split_once("\u{1b}[") {
        plain.push_str(before);
        let end = after.trim_start_matches(|c: char| c.is_ascii_digit() || matches!(c, ';' | ':'));
        rest = match end.strip_prefix('m') {
            Some(tail) => tail,
            None => {
                plain.push_str("\u{1b}[");
                after
            }
        };
    }
    plain.push_str(rest);
    Cow::Owned(plain)
}

/// `line`, its middle cut when it is over 300 characters.
fn shortened(line: &str) -> String {
    let count = line.chars().count();
    if count <= 300 {
        return line.to_string();
    }
    let first: String = line.chars().take(200).collect();
    let last: String = line.chars().skip(count.saturating_sub(100)).collect();
    format!(
        "{first}… {} more characters …{last}",
        count.saturating_sub(300)
    )
}

/// The lines of `output` to show, colours stripped and escaped: with `full`
/// all of them as written. Otherwise only those with something visible on
/// them, each at most about 300 characters: all when there are at most
/// `whole`, else the first `head`, how many were left out (when `head` is
/// not 0) and the last `tail`. Only the lines shown are formatted.
fn shown_lines(
    output: &[u8],
    full: bool,
    (whole, head, tail): (usize, usize, usize),
) -> Vec<String> {
    if !has_visible_line(output) {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(output);
    let kept = || {
        text.lines()
            .filter(move |line| full || visible(&strip_sgr(line)))
    };
    let count = kept().count();
    let cut = !full && count > whole;
    let shown = |line: &str| {
        let line = strip_sgr(line);
        let line = line.trim_end();
        match full {
            true => escape(line).into_owned(),
            false => escape(&shortened(line)).into_owned(),
        }
    };
    let mut lines: Vec<String> = kept()
        .enumerate()
        .filter(|(i, _)| !cut || *i < head || *i >= count.saturating_sub(tail))
        .map(|(_, line)| shown(line))
        .collect();
    if cut && head > 0 {
        let hidden = count.saturating_sub(head + tail);
        lines.insert(head.min(lines.len()), format!("… {hidden} more lines"));
    }
    lines
}

/// What a failed command wrote, to follow its line in a summary, each line
/// on its own, indented: stderr up to 11 lines, past that its first 5 and
/// last 4; the last 4 lines of stdout, where some errors are (a merge
/// conflict's, a test runner's). With `full`, all of both.
pub fn failure_excerpt(stderr: &[u8], stdout: &[u8], full: bool) -> String {
    let errors = shown_lines(stderr, full, (11, 5, 4));
    let output = shown_lines(stdout, full, (4, 0, 4));
    if output.is_empty() {
        return errors.iter().map(|l| format!("\n  {l}")).collect();
    }
    [("stderr", errors), ("stdout", output)]
        .iter()
        .filter(|(_, lines)| !lines.is_empty())
        .map(|(label, lines)| {
            let lines: String = lines.iter().map(|l| format!("\n    {l}")).collect();
            format!("\n  {label}:{lines}")
        })
        .collect()
}

/// [`escape`] for a path, which may also hold bytes that are not UTF-8.
pub fn escape_path(path: &std::path::Path) -> String {
    escape(&path.to_string_lossy()).into_owned()
}

/// `argv` as one line, quoted so that it reads back as the same words.
pub fn display_command(argv: &[String]) -> String {
    shlex::try_join(argv.iter().map(String::as_str)).unwrap_or_else(|_| argv.join(" "))
}

/// `value` quoted for a POSIX shell, for commands meta prints for the user
/// to paste: plain words stay as they are. A NUL byte, which no shell word
/// can hold, leaves nothing to quote.
pub fn shell_quote(value: &str) -> String {
    shlex::try_quote(value)
        .map(|quoted| quoted.into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_excerpt_keeps_the_ends_and_says_what_it_cut() {
        let numbered = |n: usize| -> String { (1..=n).map(|i| format!("l{i}\n")).collect() };
        let shown = |text: &str| failure_excerpt(text.as_bytes(), b"", false);
        assert_eq!(shown(&numbered(11)).lines().count(), 12);
        let cut = shown(&numbered(12));
        assert!(cut.contains("l5\n  … 3 more lines\n  l9\n"), "{cut}");
        assert!(cut.ends_with("l12") && !cut.contains("l6"), "{cut}");

        // nothing visible is no line; colour goes, other controls are shown
        let said = shown("\u{1b}[31m\u{1b}[0m\n \u{200b}\n\u{1b}[1mbold\u{1b}[0m\u{1b}]0;t\u{7}\n");
        assert_eq!(said, "\n  bold\\u{1b}]0;t\\u{7}");
        // a tab is escaped, as everywhere else in the summary
        for full in [false, true] {
            assert_eq!(failure_excerpt(b"L\tR\n", b"", full), "\n  L\\tR");
        }
        assert_eq!(failure_excerpt(b"\n \n", b"\x1b[0m\n", true), "");

        // 300 characters stay whole; past that the middle goes
        assert_eq!(shortened(&"x".repeat(300)), "x".repeat(300));
        let line = format!("{}{}{}", "a".repeat(200), "é".repeat(101), "z".repeat(100));
        let want = format!(
            "{}… 101 more characters …{}",
            "a".repeat(200),
            "z".repeat(100)
        );
        assert_eq!(shortened(&line), want);

        // stdout's end, labelled; all of both with `full`, blank lines too
        let both = failure_excerpt(b"e\n", numbered(6).as_bytes(), false);
        assert_eq!(
            both,
            "\n  stderr:\n    e\n  stdout:\n    l3\n    l4\n    l5\n    l6"
        );
        let full = failure_excerpt(numbered(40).as_bytes(), b"a\n\nb\n", true);
        assert!(
            full.contains("l20") && full.contains("    a\n    \n    b"),
            "{full}"
        );
    }

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
    fn program_output_follows_the_message_and_indents_its_lines() {
        assert_eq!(program_output(b"fatal: no\n"), " fatal: no");
        assert_eq!(
            program_output("Preparing\nfatal: '\u{1b}x\nFORGED' used\n".as_bytes()),
            "\n  Preparing\n  fatal: '\\u{1b}x\n  FORGED' used"
        );
    }

    #[test]
    fn escape_lines_keeps_line_breaks_and_tabs_only() {
        assert!(matches!(escape_lines("a\nb\tc"), Cow::Borrowed(_)));
        assert_eq!(escape_lines("a\u{1b}[2J\nb\r\tc"), "a\\u{1b}[2J\nb\\r\tc");
    }
}
