//! Terminal text layout: display width with wide (CJK) characters, wrapping
//! at spaces and between wide characters, and the terminal's width.
use std::io::IsTerminal;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The stdout terminal's columns, or 80 when stdout is not a terminal.
pub fn terminal_width() -> usize {
    if !std::io::stdout().is_terminal() {
        return 80;
    }
    crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
        .filter(|columns| *columns > 0)
        .unwrap_or(80)
}

/// Terminal columns `text` occupies: wide (CJK) characters take two.
pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn char_width(character: char) -> usize {
    UnicodeWidthChar::width(character).unwrap_or(0)
}

/// Breakable pieces of a line: words, and every wide character on its own,
/// each marked with whether a space precedes it.
fn pieces(text: &str) -> Vec<(String, bool)> {
    let mut pieces = Vec::new();
    let mut word = String::new();
    let mut word_spaced = false;
    let mut spaced = false;
    for character in text.chars() {
        if character == ' ' {
            if !word.is_empty() {
                pieces.push((std::mem::take(&mut word), word_spaced));
            }
            spaced = true;
        } else if char_width(character) > 1 {
            if !word.is_empty() {
                pieces.push((std::mem::take(&mut word), word_spaced));
            }
            pieces.push((character.to_string(), spaced));
            spaced = false;
        } else {
            if word.is_empty() {
                word_spaced = spaced;
                spaced = false;
            }
            word.push(character);
        }
    }
    if !word.is_empty() {
        pieces.push((word, word_spaced));
    }
    pieces
}

/// `text` wrapped to `width` columns: at spaces and between wide characters,
/// breaking a word only when it is longer than a whole line. A paragraph's
/// indentation is kept, and its continuation lines hang at the same depth.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let body = paragraph.trim_start_matches(' ');
        let indent = " ".repeat((paragraph.len() - body.len()).min(width.saturating_sub(1)));
        let width = width - indent.len();
        let first = lines.len();
        let mut line = String::new();
        let mut used = 0usize;
        for (piece, spaced) in pieces(body) {
            let piece_width = display_width(&piece);
            let gap = usize::from(spaced && !line.is_empty());
            if !line.is_empty() && used + gap + piece_width > width {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            if !line.is_empty() && spaced {
                line.push(' ');
                used += 1;
            }
            if piece_width <= width.saturating_sub(used) {
                line.push_str(&piece);
                used += piece_width;
                continue;
            }
            for character in piece.chars() {
                let character_width = char_width(character);
                if used + character_width > width && !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                    used = 0;
                }
                line.push(character);
                used += character_width;
            }
        }
        lines.push(line);
        if !indent.is_empty() {
            for line in &mut lines[first..] {
                line.insert_str(0, &indent);
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_counts_wide_characters_and_keeps_indentation() {
        assert_eq!(display_width("检材 disk"), 9);
        assert_eq!(
            wrap("打开检材并浏览文件", 8),
            ["打开检材", "并浏览文", "件"]
        );
        assert_eq!(
            wrap("  open the evidence", 10),
            ["  open the", "  evidence"]
        );
    }
}
