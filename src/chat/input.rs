//! The one-line message editor, and the talk key as text ("alt+space").

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

#[derive(Default)]
pub struct LineInput {
    chars: Vec<char>,
    cursor: usize,
}

impl LineInput {
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn set(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.cursor = self.chars.len();
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars().filter(|c| !c.is_control() || *c == ' ') {
            self.chars.insert(self.cursor, c);
            self.cursor += 1;
        }
    }

    /// Handles an editing key; false when the key is not for the editor.
    pub fn key(&mut self, k: &KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Char('u') if ctrl => {
                self.chars.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('k') if ctrl => {
                self.chars.truncate(self.cursor);
            }
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Backspace if alt || ctrl => self.delete_word(),
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.chars.len(),
            KeyCode::Char(c) if !ctrl && !alt => {
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.chars.remove(self.cursor);
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.chars.len() {
                    self.chars.remove(self.cursor);
                }
            }
            KeyCode::Left if ctrl || alt => self.word_left(),
            KeyCode::Right if ctrl || alt => self.word_right(),
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.chars.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.chars.len(),
            _ => return false,
        }
        true
    }

    fn delete_word(&mut self) {
        let end = self.cursor;
        self.word_left();
        self.chars.drain(self.cursor..end);
    }

    fn word_left(&mut self) {
        while self.cursor > 0 && self.chars[self.cursor - 1] == ' ' {
            self.cursor -= 1;
        }
        while self.cursor > 0 && self.chars[self.cursor - 1] != ' ' {
            self.cursor -= 1;
        }
    }

    fn word_right(&mut self) {
        let n = self.chars.len();
        while self.cursor < n && self.chars[self.cursor] == ' ' {
            self.cursor += 1;
        }
        while self.cursor < n && self.chars[self.cursor] != ' ' {
            self.cursor += 1;
        }
    }

    /// The part to show in `width` columns, and the cursor's column in it.
    pub fn view(&self, width: usize) -> (String, usize) {
        let width = width.max(1);
        let before: String = self.chars[..self.cursor].iter().collect();
        let mut start = 0;
        // Scroll so the cursor stays visible.
        while UnicodeWidthStr::width(before.chars().skip(start).collect::<String>().as_str()) >= width {
            start += 1;
        }
        let shown: String = self.chars[start..].iter().collect();
        let mut out = String::new();
        for c in shown.chars() {
            if UnicodeWidthStr::width(format!("{out}{c}").as_str()) > width {
                break;
            }
            out.push(c);
        }
        let col = UnicodeWidthStr::width(self.chars[start..self.cursor].iter().collect::<String>().as_str());
        (out, col)
    }
}

impl LineInput {
    /// The text wrapped at word ends into rows of `width` columns, the
    /// `rows` of them around the cursor, and the cursor's row and column
    /// among those rows.
    pub fn view_rows(&self, width: usize, rows: usize) -> (Vec<String>, usize, usize) {
        let width = width.max(1);
        // Where each character goes, and where the cursor after the last one goes.
        let mut place: Vec<(usize, usize)> = Vec::with_capacity(self.chars.len() + 1);
        let (mut row, mut col, mut row_start, mut space) = (0usize, 0usize, 0usize, None::<usize>);
        let mut i = 0;
        while i < self.chars.len() {
            let c = self.chars[i];
            let w = UnicodeWidthStr::width(c.to_string().as_str());
            if col + w > width && col > 0 {
                row += 1;
                col = 0;
                // Carry the word that does not fit to the new row.
                if let Some(sp) = space.filter(|&sp| sp + 1 > row_start && sp + 1 < i) {
                    place.truncate(sp + 1);
                    i = sp + 1;
                    row_start = i;
                    space = None;
                    continue;
                }
                row_start = i;
                space = None;
            }
            place.push((row, col));
            if c == ' ' {
                space = Some(i);
            }
            col += w;
            i += 1;
        }
        place.push(if col >= width { (row + 1, 0) } else { (row, col) });
        let total = place.last().map(|p| p.0 + 1).unwrap_or(1);
        let mut lines = vec![String::new(); total];
        for (c, &(r, _)) in self.chars.iter().zip(&place) {
            lines[r].push(*c);
        }
        let (cursor_row, cursor_col) = place[self.cursor];
        let first = (cursor_row + 1).saturating_sub(rows.max(1));
        let shown = lines.into_iter().skip(first).take(rows.max(1)).collect();
        (shown, cursor_row - first, cursor_col)
    }
}

/// Tells dictated text from typing by its speed. A dictation tool types a
/// whole phrase with a millisecond or so between characters; a person, or a
/// held key repeating, is far slower. A paste counts as dictated, since some
/// dictation tools paste instead of typing.
#[derive(Default)]
pub struct Dictation {
    last: Option<std::time::Instant>,
    run: u32,
    pub dictated: bool,
}

impl Dictation {
    /// Characters closer together than this belong to one burst.
    const GAP: std::time::Duration = std::time::Duration::from_millis(15);
    /// This many characters in one burst is dictation.
    const RUN: u32 = 5;

    pub fn char_at(&mut self, now: std::time::Instant) {
        self.run = match self.last {
            Some(t) if now.duration_since(t) < Self::GAP => self.run + 1,
            _ => 1,
        };
        self.last = Some(now);
        if self.run >= Self::RUN {
            self.dictated = true;
        }
    }

    pub fn pasted(&mut self) {
        self.dictated = true;
    }

    pub fn reset(&mut self) {
        *self = Dictation::default();
    }
}

/// A key from the config, like "alt+space", "ctrl+t" or "f2".
#[derive(Clone, Debug, PartialEq)]
pub struct KeySpec {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeySpec {
    pub fn parse(s: &str) -> Option<KeySpec> {
        let mut modifiers = KeyModifiers::NONE;
        let mut code = None;
        for part in s.to_lowercase().split('+').map(str::trim) {
            match part {
                "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
                "alt" => modifiers |= KeyModifiers::ALT,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                "space" => code = Some(KeyCode::Char(' ')),
                "tab" => code = Some(KeyCode::Tab),
                "enter" => code = Some(KeyCode::Enter),
                p if p.len() > 1 && p.starts_with('f') => code = Some(KeyCode::F(p[1..].parse().ok()?)),
                p if p.chars().count() == 1 => code = p.chars().next().map(KeyCode::Char),
                _ => return None,
            }
        }
        Some(KeySpec { code: code?, modifiers })
    }

    pub fn matches(&self, k: &KeyEvent) -> bool {
        let code = match k.code {
            KeyCode::Char(c) => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        };
        // Shift is part of a letter's case, not a separate press.
        let mods = if matches!(k.code, KeyCode::Char(_)) && !self.modifiers.contains(KeyModifiers::SHIFT) {
            k.modifiers - KeyModifiers::SHIFT
        } else {
            k.modifiers
        };
        code == self.code && mods == self.modifiers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_word_ends() {
        let mut l = LineInput::default();
        l.set("hello there world");
        let (rows, r, c) = l.view_rows(12, 3);
        assert_eq!(rows, vec!["hello there ", "world"]);
        assert_eq!((r, c), (1, 5));
        // Only the last rows show, and the cursor stays on them.
        l.set("aa bb cc dd ee");
        let (rows, r, c) = l.view_rows(3, 2);
        assert_eq!(rows, vec!["dd ", "ee"]);
        assert_eq!((r, c), (1, 2));
        l.set("abcdef");
        assert_eq!(l.view_rows(3, 3), (vec!["abc".to_string(), "def".into(), "".into()], 2, 0));
    }

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn edits_a_line() {
        let mut i = LineInput::default();
        i.insert_str("hello world");
        i.key(&key(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(i.text(), "hello ");
        i.key(&key(KeyCode::Home, KeyModifiers::NONE));
        i.key(&key(KeyCode::Char('>'), KeyModifiers::NONE));
        assert_eq!(i.text(), ">hello ");
        assert_eq!(i.view(4), (">hel".to_string(), 1));
    }

    #[test]
    fn tells_dictation_from_typing() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let mut typed = Dictation::default();
        for i in 0..20 {
            typed.char_at(t0 + Duration::from_millis(30 * i)); // key repeat at ~33/s
        }
        assert!(!typed.dictated);
        let mut dictated = Dictation::default();
        for i in 0..6 {
            dictated.char_at(t0 + Duration::from_millis(i)); // voxtype: 1 ms apart
        }
        assert!(dictated.dictated);
    }

    #[test]
    fn parses_talk_keys() {
        let k = KeySpec::parse("ctrl+space").unwrap();
        assert!(k.matches(&key(KeyCode::Char(' '), KeyModifiers::CONTROL)));
        assert!(!k.matches(&key(KeyCode::Char(' '), KeyModifiers::NONE)));
        assert_eq!(KeySpec::parse("f2").unwrap().code, KeyCode::F(2));
        assert!(KeySpec::parse("alt+t").unwrap().matches(&key(KeyCode::Char('T'), KeyModifiers::ALT | KeyModifiers::SHIFT)));
        assert!(KeySpec::parse("hyper+x").is_none());
    }
}
