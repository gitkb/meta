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

/// The lines of another program's output a summary shows under its failure,
/// fed as the program writes it (one stream, or each of two: see
/// [`Excerpt::chain`]): those with something on them, escaped (colour codes
/// dropped first, tabs as spaces), each cut past [`EXCERPT_WIDTH`]
/// characters; past [`EXCERPT_LINES`] of them, the first [`EXCERPT_HEAD`],
/// how many are left out, and the last [`EXCERPT_TAIL`], where programs put
/// the reason (git's `fatal:` after its hints). What it holds is bounded,
/// however much the program writes. A full one ([`Excerpt::new`],
/// `--full-errors`) keeps every line whole, up to [`FULL_LIMIT`] bytes.
#[derive(Debug, Default, Clone)]
pub struct Excerpt {
    /// The first lines, up to [`EXCERPT_LINES`] (all, when full), ready to show
    head: Vec<String>,
    /// The last lines after those, up to [`EXCERPT_TAIL`]
    tail: std::collections::VecDeque<String>,
    /// How many lines came after the first [`EXCERPT_LINES`]
    rest: usize,
    /// Each stream's line not ended yet (stdout, stderr)
    open: [Partial; 2],
    /// Every line whole (`--full-errors`)
    full: bool,
    /// The bytes of the lines a full one holds
    held: usize,
    /// The lines a full one counted past [`FULL_LIMIT`], not kept
    not_kept: usize,
}

/// A line being read: its first bytes, the last ones after those, and how
/// many characters came after the first (colour codes don't count).
#[derive(Debug, Default, Clone)]
struct Partial {
    kept: Vec<u8>,
    last: std::collections::VecDeque<u8>,
    after: usize,
    /// Where the bytes read so far left off: in a terminal code or not
    scan: Scan,
}

/// Where a line's bytes left off, to count its characters as the excerpt
/// shows them: colour codes (SGR) are none; any other code's bytes are
/// shown, escaped.
#[derive(Debug, Default, Clone, Copy)]
enum Scan {
    #[default]
    Text,
    /// After an ESC; how many of the code's characters are to count
    Esc(usize),
    /// In a CSI (`ESC [ …`)
    Csi(usize),
}

impl Scan {
    /// The state after byte `b`, and how many characters it ends up as
    /// (`counted`: whether `b` is among those counted at all).
    fn step(self, b: u8, counted: bool) -> (Scan, usize) {
        let starts = usize::from(counted && b & 0xC0 != 0x80);
        match self {
            Scan::Text if b == 0x1b => (Scan::Esc(starts), 0),
            Scan::Text => (Scan::Text, starts),
            Scan::Esc(n) if b == b'[' => (Scan::Csi(n + starts), 0),
            Scan::Esc(n) => (Scan::Text, n + starts),
            Scan::Csi(n) if (0x20..=0x3f).contains(&b) => (Scan::Csi(n + starts), 0),
            Scan::Csi(_) if b == b'm' => (Scan::Text, 0),
            Scan::Csi(n) => (Scan::Text, n + starts),
        }
    }
}

/// How many lines of a program's output an [`Excerpt`] shows in full; past
/// that, the first [`EXCERPT_HEAD`], a line saying how many are left out, and
/// the last [`EXCERPT_TAIL`] (a marker that would hide fewer than three lines
/// isn't worth its row).
pub const EXCERPT_LINES: usize = EXCERPT_HEAD + 2 + EXCERPT_TAIL;
pub const EXCERPT_HEAD: usize = 5;
pub const EXCERPT_TAIL: usize = 4;
/// The most characters of one line an [`Excerpt`] shows (a megabyte of
/// minified JavaScript is one line): past that, its first
/// [`EXCERPT_LINE_HEAD`], how many are left out, and its last
/// [`EXCERPT_LINE_TAIL`], where a reason may end it.
pub const EXCERPT_WIDTH: usize = EXCERPT_LINE_HEAD + EXCERPT_LINE_TAIL;
pub const EXCERPT_LINE_HEAD: usize = 200;
pub const EXCERPT_LINE_TAIL: usize = 100;
/// The bytes kept of a line's start and of its end, to show their
/// characters with colour codes among them.
const EXCERPT_KEEP: usize = EXCERPT_WIDTH * 8;
const EXCERPT_KEEP_LAST: usize = EXCERPT_LINE_TAIL * 16;
/// The most a full excerpt keeps of one program's lines, in bytes: as much
/// as meta captures of it ([`crate::process::CAPTURE_LIMIT`]'s 16 MiB).
pub const FULL_LIMIT: usize = 16 << 20;

/// Which of a program's streams bytes came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

impl Excerpt {
    /// An empty excerpt; with `full`, one that keeps every line whole.
    pub fn new(full: bool) -> Self {
        Excerpt {
            full,
            ..Excerpt::default()
        }
    }

    /// The excerpt of `output`, all of it written to one stream.
    pub fn of(output: &[u8], full: bool) -> Self {
        let mut excerpt = Excerpt::new(full);
        excerpt.push(Stream::Stderr, output);
        excerpt.finish();
        excerpt
    }

    /// Add what the program wrote next to `stream`.
    pub fn push(&mut self, stream: Stream, bytes: &[u8]) {
        let index = match stream {
            Stream::Stdout => 0,
            Stream::Stderr => 1,
        };
        let keep = match self.full {
            true => FULL_LIMIT.saturating_sub(self.held),
            false => EXCERPT_KEEP,
        };
        let mut lines = bytes.split(|&b| b == b'\n').peekable();
        while let Some(piece) = lines.next() {
            let Some(open) = self.open.get_mut(index) else {
                return;
            };
            let room = keep.saturating_sub(open.kept.len());
            let (kept, over) = piece.split_at(room.min(piece.len()));
            open.kept.extend_from_slice(kept);
            for &b in kept {
                open.scan = open.scan.step(b, false).0;
            }
            for &b in over {
                let (scan, chars) = open.scan.step(b, true);
                open.scan = scan;
                open.after += chars;
            }
            open.last.extend(over);
            let extra = open.last.len().saturating_sub(EXCERPT_KEEP_LAST);
            open.last.drain(..extra);
            // the last piece is a line not ended yet
            if lines.peek().is_some() {
                let line = std::mem::take(open);
                self.add(line);
            }
        }
    }

    /// End the lines the program left open (output with no final newline).
    pub fn finish(&mut self) {
        for index in 0..self.open.len() {
            if let Some(open) = self.open.get_mut(index) {
                let line = std::mem::take(open);
                if !line.kept.is_empty() {
                    self.add(line);
                }
            }
        }
    }

    fn add(&mut self, line: Partial) {
        if let Some(line) = shown(&line, self.full) {
            self.add_shown(line);
        }
    }

    fn add_shown(&mut self, line: String) {
        if self.full {
            match self.held < FULL_LIMIT {
                true => {
                    self.held += line.len();
                    self.head.push(line);
                }
                false => self.not_kept += 1,
            }
            return;
        }
        if self.head.len() < EXCERPT_LINES {
            self.head.push(line);
            return;
        }
        if self.tail.len() == EXCERPT_TAIL {
            self.tail.pop_front();
        }
        self.tail.push_back(line);
        self.rest += 1;
    }

    /// This excerpt's lines, then `later`'s, as one: the output of two
    /// streams meta read apart (stdout, then stderr, where the reason goes).
    /// Both must be finished.
    pub fn chain(self, later: Excerpt) -> Excerpt {
        let mut out = Excerpt::new(self.full);
        for part in [self, later] {
            out.not_kept += part.not_kept;
            // the lines between its first and its last aren't known
            let hidden = part.rest.saturating_sub(part.tail.len());
            for line in part.head {
                out.add_shown(line);
            }
            if hidden > 0 {
                out.tail.clear();
                out.rest += hidden;
            }
            for line in part.tail {
                out.add_shown(line);
            }
        }
        out
    }

    /// The lines to show, ended ones only (see [`Excerpt::finish`]).
    pub fn lines(&self) -> Vec<String> {
        if self.full {
            let mut lines = self.head.clone();
            match self.not_kept {
                0 => {}
                n => lines.push(format!(
                    "… {n} more lines (past {} MiB, not kept)",
                    FULL_LIMIT >> 20
                )),
            }
            return lines;
        }
        if self.rest == 0 {
            return self.head.clone();
        }
        // the last ones may start among the first
        let from_head = EXCERPT_TAIL.saturating_sub(self.tail.len());
        let last: Vec<&String> = self
            .head
            .iter()
            .skip(self.head.len().saturating_sub(from_head))
            .chain(&self.tail)
            .collect();
        let left_out = (self.head.len() + self.rest).saturating_sub(EXCERPT_HEAD + last.len());
        self.head
            .iter()
            .take(EXCERPT_HEAD)
            .cloned()
            .chain(std::iter::once(format!("… {left_out} more lines")))
            .chain(last.into_iter().cloned())
            .collect()
    }

    /// Whether the program wrote any line with something on it.
    pub fn is_empty(&self) -> bool {
        self.head.is_empty() && self.not_kept == 0
    }
}

/// `line` as an excerpt shows it, None when nothing would show of it (blank,
/// or only terminal codes): colour codes dropped, tabs as spaces, a CRLF's
/// CR dropped, every other control escaped; past [`EXCERPT_WIDTH`]
/// characters, its start, how many are left out, and its end (`full`: all
/// of it, up to [`FULL_LIMIT`]).
fn shown(line: &Partial, full: bool) -> Option<String> {
    let start = plain(&line.kept);
    // the end kept may start inside a character, or inside a colour code
    // (`1m` left of `ESC [ 3 1 m`)
    let last: Vec<u8> = line
        .last
        .iter()
        .copied()
        .skip_while(|b| b & 0xC0 == 0x80)
        .collect();
    let last = match line.after > 0 {
        true => without_cut_code(&last),
        false => &last[..],
    };
    let end = plain(last);
    if start.visible.trim().is_empty() && end.visible.trim().is_empty() {
        return None;
    }
    let chars: Vec<char> = start.text.chars().collect();
    let (head, tail, total): (String, String, usize) = match line.after {
        0 if full || chars.len() <= EXCERPT_WIDTH => return Some(escape(&start.text).into_owned()),
        after if full => (start.text.clone(), String::new(), chars.len() + after),
        0 => (
            chars.iter().take(EXCERPT_LINE_HEAD).collect(),
            chars
                .iter()
                .skip(chars.len().saturating_sub(EXCERPT_LINE_TAIL))
                .collect(),
            chars.len(),
        ),
        after => {
            let end: Vec<char> = end.text.chars().collect();
            (
                chars.iter().take(EXCERPT_LINE_HEAD).collect(),
                end.iter()
                    .skip(end.len().saturating_sub(EXCERPT_LINE_TAIL))
                    .collect(),
                chars.len() + after,
            )
        }
    };
    let left_out = total.saturating_sub(head.chars().count() + tail.chars().count());
    Some(match tail.is_empty() {
        true => format!("{} … {left_out} more characters", escape(&head)),
        false => format!(
            "{} … {left_out} more characters … {}",
            escape(&head),
            escape(&tail)
        ),
    })
}

/// `bytes` without the end of a terminal code cut off at their start: the
/// parameters and final byte of a CSI whose `ESC [` (or `ESC`) went.
fn without_cut_code(bytes: &[u8]) -> &[u8] {
    let rest = bytes.strip_prefix(b"[").unwrap_or(bytes);
    let params = rest
        .iter()
        .take_while(|b| (0x30..=0x3f).contains(*b))
        .count();
    match rest.get(params) {
        Some(end) if (0x40..=0x7e).contains(end) && params < 16 => {
            rest.get(params + 1..).unwrap_or_default()
        }
        _ => bytes,
    }
}

/// A piece of a line, cleaned for an excerpt.
struct Plain {
    /// Colour codes dropped, tabs as spaces, trailing spaces and a CRLF's
    /// CR dropped
    text: String,
    /// What a terminal would show of it: every code dropped
    visible: String,
}

fn plain(bytes: &[u8]) -> Plain {
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_suffix('\r').unwrap_or(&text);
    let mut plain = String::with_capacity(text.len());
    let mut column = 0;
    for c in without_codes(text, false).trim_end().chars() {
        match c {
            '\t' => {
                let to = (column / 8 + 1) * 8;
                plain.extend(std::iter::repeat_n(' ', to - column));
                column = to;
            }
            c => {
                plain.push(c);
                column += 1;
            }
        }
    }
    Plain {
        text: plain,
        visible: without_codes(text, true),
    }
}

/// `text` without its colour codes (SGR: `ESC [ … m`), or with `all`,
/// without any terminal code (`ESC [ … X`, `ESC ] … BEL`, `ESC X`).
fn without_codes(text: &str, all: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, body)) = rest.split_once('\u{1b}') {
        out.push_str(before);
        // the code's length after its ESC, and whether it sets colour
        let (len, colour) = match body.chars().next() {
            // CSI: parameters and intermediates, then a final byte
            Some('[') => {
                let end = body
                    .char_indices()
                    .skip(1)
                    .find(|(_, c)| !matches!(c, '\u{20}'..='\u{3f}'))
                    .map(|(i, c)| (i + c.len_utf8(), c));
                match end {
                    Some((i, c)) if ('\u{40}'..='\u{7e}').contains(&c) => (i, c == 'm'),
                    _ => (0, false),
                }
            }
            // OSC: up to BEL or ST
            Some(']') => match body.find(['\u{7}', '\u{9c}']) {
                Some(i) => (
                    i + body
                        .get(i..)
                        .and_then(|t| t.chars().next())
                        .map_or(1, char::len_utf8),
                    false,
                ),
                None => match body.find("\u{1b}\\") {
                    Some(i) => (i + 2, false),
                    None => (0, false),
                },
            },
            Some(c) if ('\u{40}'..='\u{7e}').contains(&c) => (c.len_utf8(), false),
            _ => (0, false),
        };
        // a lone ESC stays, escaped later
        if len == 0 || !(all || colour) {
            out.push('\u{1b}');
            out.push_str(body.get(..len).unwrap_or_default());
        }
        rest = body.get(len..).unwrap_or_default();
    }
    out.push_str(rest);
    out
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
    fn an_excerpt_keeps_the_first_and_the_last_lines_of_a_long_output() {
        let with = |n: usize| {
            let output: String = (1..=n).map(|i| format!("line {i}\n\n")).collect();
            Excerpt::of(output.as_bytes(), false).lines()
        };
        let lines = |range: std::ops::RangeInclusive<usize>| -> Vec<String> {
            range.map(|i| format!("line {i}")).collect()
        };
        // up to eleven, all of them (a marker would hide only two); blank
        // ones never count
        assert_eq!(with(11), lines(1..=11));
        // past that: the first five, how many are left out, the last four
        let cut = |n: usize, left_out: &str| -> Vec<String> {
            [lines(1..=5), vec![left_out.to_string()], lines(n - 3..=n)].concat()
        };
        assert_eq!(with(12), cut(12, "… 3 more lines"));
        assert_eq!(with(15), cut(15, "… 6 more lines"));
        assert_eq!(with(100_000), cut(100_000, "… 99991 more lines"));
        assert!(with(0).is_empty());
    }

    #[test]
    fn an_excerpt_cuts_a_long_line_and_drops_colour_codes() {
        // a megabyte on one line, fed in pieces as a pipe gives it
        let mut excerpt = Excerpt::default();
        for _ in 0..1024 {
            excerpt.push(Stream::Stderr, &[b'x'; 1024]);
        }
        excerpt.push(Stream::Stderr, b"\nfatal: after it\n");
        excerpt.finish();
        let expected = format!(
            "{} … {} more characters … {}",
            "x".repeat(200),
            1024 * 1024 - 300,
            "x".repeat(100)
        );
        assert_eq!(
            excerpt.lines(),
            vec![expected, "fatal: after it".to_string()]
        );
        // colour goes, other codes are shown; a line of codes alone takes no
        // row; tabs line up as a terminal shows them; a CRLF's CR goes
        let codes = Excerpt::of(
            b"\x1b[31merror:\x1b[0m red\n\x1b[0m\n\x1b[K\n\t\nfatal: real\r\n\x1b]0;T\x07x\nab\terr\n",
            false,
        );
        assert_eq!(
            codes.lines(),
            [
                "error: red",
                "fatal: real",
                "\\u{1b}]0;T\\u{7}x",
                "ab      err"
            ]
        );
    }

    #[test]
    fn a_long_coloured_line_counts_the_characters_it_shows() {
        // 400 red R's (10 bytes each with their codes), then END: 403 shown
        let line = format!("{}END\n", "\x1b[31mR\x1b[0m".repeat(400));
        let lines = Excerpt::of(line.as_bytes(), false).lines();
        assert_eq!(
            lines,
            [format!(
                "{} … 103 more characters … {}END",
                "R".repeat(200),
                "R".repeat(97)
            )]
        );
    }

    #[test]
    fn a_full_excerpt_keeps_every_line_whole() {
        let mut output: String = (1..=30).map(|i| format!("line {i}\n")).collect();
        output.push_str(&"x".repeat(1 << 20));
        output.push('\n');
        let lines = Excerpt::of(output.as_bytes(), true).lines();
        assert_eq!(lines.len(), 31);
        assert_eq!(lines.first().map(String::as_str), Some("line 1"));
        assert_eq!(lines.last().map(String::len), Some(1 << 20));
        // capped: 5, the count, 4; the long line cut
        let capped = Excerpt::of(output.as_bytes(), false).lines();
        assert_eq!(capped.len(), 10);
        assert!(capped.iter().all(|line| line.len() < 400), "{capped:?}");
    }

    #[test]
    fn two_streams_read_apart_chain_with_the_later_one_last() {
        let out = Excerpt::of(
            (1..=20)
                .map(|i| format!("out {i}\n"))
                .collect::<String>()
                .as_bytes(),
            false,
        );
        let err = Excerpt::of(b"error: the reason\n", false);
        assert_eq!(
            out.chain(err).lines(),
            [
                "out 1",
                "out 2",
                "out 3",
                "out 4",
                "out 5",
                "… 12 more lines",
                "out 18",
                "out 19",
                "out 20",
                "error: the reason"
            ]
        );
    }

    #[test]
    fn an_excerpt_takes_both_streams_in_the_order_they_came() {
        let mut excerpt = Excerpt::default();
        excerpt.push(Stream::Stderr, b"From remote\n   a..b main\n");
        excerpt.push(Stream::Stdout, b"CONFLICT (content): Merge ");
        // a line of each stream may be open at once
        excerpt.push(Stream::Stderr, b"error: half");
        excerpt.push(Stream::Stdout, b"conflict in f\nAutomatic merge failed\n");
        excerpt.finish();
        assert_eq!(
            excerpt.lines(),
            [
                "From remote",
                "   a..b main",
                "CONFLICT (content): Merge conflict in f",
                "Automatic merge failed",
                "error: half"
            ]
        );
    }

    #[test]
    fn escape_lines_keeps_line_breaks_and_tabs_only() {
        assert!(matches!(escape_lines("a\nb\tc"), Cow::Borrowed(_)));
        assert_eq!(escape_lines("a\u{1b}[2J\nb\r\tc"), "a\\u{1b}[2J\nb\\r\tc");
    }
}
