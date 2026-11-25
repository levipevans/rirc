//! Small text helpers shared by the model and the UI.

use chrono::{DateTime, Local};
use ratatui::style::Color;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// IRC case folding (rfc1459 flavour).
pub fn fold(s: &str) -> String {
    s.chars()
        .map(|c| match c.to_ascii_lowercase() {
            '[' => '{',
            ']' => '}',
            '\\' => '|',
            '^' => '~',
            other => other,
        })
        .collect()
}

pub fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Clip a string to `max` terminal cells, adding an ellipsis.
pub fn truncate(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// Word-wrap `text`, using a different width for the first line than for the
/// remaining (hanging indent) lines. Newlines force a break.
pub fn wrap_widths(text: &str, first: usize, rest: usize) -> Vec<String> {
    let text = text.replace('\t', "    ");
    let first = first.max(1);
    let rest = rest.max(1);
    let mut out: Vec<String> = Vec::new();
    let mut current: Vec<char> = Vec::new();
    let mut cur_w = 0usize;
    let mut last_space: Option<usize> = None;
    let mut limit = first;

    for ch in text.chars() {
        if ch == '\r' {
            continue;
        }
        if ch == '\n' {
            out.push(current.iter().collect::<String>().trim_end().to_string());
            current.clear();
            cur_w = 0;
            last_space = None;
            limit = rest;
            continue;
        }
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + cw > limit && !current.is_empty() {
            if let Some(sp) = last_space {
                let tail: Vec<char> = current.split_off(sp + 1);
                out.push(current.iter().collect::<String>().trim_end().to_string());
                current = tail;
            } else {
                out.push(current.iter().collect());
                current.clear();
            }
            cur_w = current
                .iter()
                .map(|c| UnicodeWidthChar::width(*c).unwrap_or(0))
                .sum();
            last_space = current.iter().rposition(|c| *c == ' ');
            limit = rest;
        }
        current.push(ch);
        cur_w += cw;
        if ch == ' ' {
            last_space = Some(current.len() - 1);
        }
    }
    out.push(current.iter().collect::<String>().trim_end().to_string());
    out
}

/// Wrap with the same width everywhere.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    wrap_widths(text, width, width)
}

/// Does `text` mention `nick` as a whole word (irc-ish)?
pub fn mentions(text: &str, nick: &str) -> bool {
    if nick.is_empty() {
        return false;
    }
    let hay = fold(text);
    let needle = fold(nick);
    let bytes = hay.as_bytes();
    let mut from = 0;
    while let Some(pos) = hay[from..].find(&needle) {
        let start = from + pos;
        let end = start + needle.len();
        let boundary = |c: u8| !c.is_ascii_alphanumeric() && c != b'_' && c != b'-';
        let before_ok = start == 0 || boundary(bytes[start - 1]);
        let after_ok = end >= bytes.len() || boundary(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + needle.len().max(1);
        if from >= hay.len() {
            break;
        }
    }
    false
}

/// Flattens newlines and drops IRC formatting/control codes for single-line UI.
pub fn sanitize_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{3}' => {
                // mIRC colour: up to two foreground digits, optional ,bg
                for _ in 0..2 {
                    if chars.peek().is_some_and(|d| d.is_ascii_digit()) {
                        chars.next();
                    } else {
                        break;
                    }
                }
                if chars.peek() == Some(&',') {
                    chars.next();
                    for _ in 0..2 {
                        if chars.peek().is_some_and(|d| d.is_ascii_digit()) {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
            }
            '\u{2}' | '\u{1d}' | '\u{1f}' | '\u{0f}' | '\u{16}' | '\u{1e}' => {}
            '\n' | '\t' => out.push(' '),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Split an outgoing message into IRC-sized chunks, preferring word boundaries.
pub fn irc_chunks(text: &str, max: usize) -> Vec<String> {
    let max = max.max(4);
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_bytes = 0usize;
    for word in text.split_inclusive(' ') {
        let word_bytes = word.len();
        if !current.is_empty() && current_bytes + word_bytes > max {
            out.push(current.trim_end().to_string());
            current.clear();
            current_bytes = 0;
        }
        if word_bytes > max {
            for ch in word.chars() {
                let char_bytes = ch.len_utf8();
                if !current.is_empty() && current_bytes + char_bytes > max {
                    out.push(current.clone());
                    current.clear();
                    current_bytes = 0;
                }
                current.push(ch);
                current_bytes += char_bytes;
            }
        } else {
            current.push_str(word);
            current_bytes += word_bytes;
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim_end().to_string());
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

pub const NICK_COLORS: [Color; 12] = [
    Color::Rgb(122, 178, 255),
    Color::Rgb(158, 206, 106),
    Color::Rgb(247, 140, 108),
    Color::Rgb(187, 154, 247),
    Color::Rgb(125, 207, 255),
    Color::Rgb(224, 175, 104),
    Color::Rgb(255, 117, 127),
    Color::Rgb(115, 218, 202),
    Color::Rgb(210, 140, 255),
    Color::Rgb(190, 210, 120),
    Color::Rgb(255, 158, 100),
    Color::Rgb(150, 170, 240),
];

pub fn nick_color(nick: &str) -> Color {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in fold(nick).bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    NICK_COLORS[(hash % NICK_COLORS.len() as u64) as usize]
}

pub fn clock(ts: DateTime<Local>) -> String {
    ts.format("%H:%M").to_string()
}

pub fn parse_server_time(value: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|d| d.with_timezone(&Local))
}

/// "3d 4h", "12m", "42s"
pub fn short_duration(secs: i64) -> String {
    let secs = secs.max(0);
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{}d {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else if mins > 0 {
        format!("{}m", mins)
    } else {
        format!("{}s", secs)
    }
}

pub fn base64(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_matches_rfc1459() {
        assert_eq!(fold("Nick[One]"), "nick{one}");
    }

    #[test]
    fn wrapping_breaks_on_words() {
        let lines = wrap_widths("hello wonderful world", 12, 12);
        assert_eq!(lines, vec!["hello", "wonderful", "world"]);
    }

    #[test]
    fn wrapping_handles_long_words() {
        let lines = wrap_widths("abcdefghij", 4, 4);
        assert_eq!(lines.concat(), "abcdefghij");
        assert!(lines.iter().all(|l| width(l) <= 4));
    }

    #[test]
    fn mention_needs_boundaries() {
        assert!(mentions("hi vimmer!", "vimmer"));
        assert!(!mentions("vim is great", "vimmer"));
        assert!(!mentions("vimmer2", "vimmer"));
    }

    #[test]
    fn chunks_long_messages() {
        let chunks = irc_chunks("the quick brown fox jumps over the lazy dog", 12);
        assert!(chunks.iter().all(|c| c.len() <= 12));
        assert_eq!(
            chunks.join(" "),
            "the quick brown fox jumps over the lazy dog"
        );

        let text = "é".repeat(300);
        let chunks = irc_chunks(&text, 100);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 100));
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn strips_escape_sequences() {
        let clean = sanitize_line("hi\u{1b}[31mthere\nnew line");
        assert!(!clean.contains('\u{1b}'));
        assert!(clean.contains("new line"));
        let pretty = sanitize_line("\u{3}04,02red\u{f} text\u{2}");
        assert_eq!(pretty, "red text");
    }
}
