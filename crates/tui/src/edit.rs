//! The edit line for a cell value or a formula: its text and a caret.

use crossterm::event::KeyCode;

/// Text being edited and the caret in it: a byte offset on a char
/// boundary, `0..=text.len()`. Built from a `&str` with the caret at the
/// end; derefs to the text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LineEdit {
    pub text: String,
    pub caret: usize,
    /// The text is a cell's old value, as if selected: the first typed
    /// character replaces it, as in a spreadsheet (H1-2). Any editing or
    /// caret key keeps the text and clears this.
    pub fresh: bool,
}

impl From<&str> for LineEdit {
    fn from(s: &str) -> Self {
        s.to_string().into()
    }
}

impl From<String> for LineEdit {
    fn from(text: String) -> Self {
        LineEdit {
            caret: text.len(),
            text,
            fresh: false,
        }
    }
}

impl std::ops::Deref for LineEdit {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

impl LineEdit {
    /// `text` with the caret at the end, replaced by the first character
    /// typed ([`LineEdit::fresh`]).
    pub fn replacing(text: String) -> Self {
        LineEdit {
            fresh: true,
            ..text.into()
        }
    }

    /// Insert `s` at the caret and move the caret past it; on a fresh
    /// edit, `s` replaces the text.
    pub fn insert(&mut self, s: &str) {
        if std::mem::take(&mut self.fresh) {
            self.text.clear();
            self.caret = 0;
        }
        self.text.insert_str(self.caret, s);
        self.caret += s.len();
    }

    /// Replace `start..caret` with `s`, leaving the caret after it.
    pub fn replace(&mut self, start: usize, s: &str) {
        self.text.replace_range(start..self.caret, s);
        self.caret = start + s.len();
    }

    fn prev(&self) -> usize {
        self.text[..self.caret]
            .chars()
            .next_back()
            .map_or(self.caret, |c| self.caret - c.len_utf8())
    }

    fn next(&self) -> usize {
        self.text[self.caret..]
            .chars()
            .next()
            .map_or(self.caret, |c| self.caret + c.len_utf8())
    }

    /// Apply a line-editing key; true if the text changed.
    pub fn key(&mut self, code: KeyCode) -> bool {
        let before = self.text.len();
        match code {
            KeyCode::Char(c) => {
                self.insert(c.encode_utf8(&mut [0; 4]));
                return true;
            }
            KeyCode::Backspace => {
                let p = self.prev();
                self.text.replace_range(p..self.caret, "");
                self.caret = p;
            }
            KeyCode::Delete => {
                let n = self.next();
                self.text.replace_range(self.caret..n, "");
            }
            KeyCode::Left => self.caret = self.prev(),
            KeyCode::Right => self.caret = self.next(),
            KeyCode::Home => self.caret = 0,
            KeyCode::End => self.caret = self.text.len(),
            _ => return false,
        }
        self.fresh = false;
        self.text.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_edits_on_char_boundaries() {
        let mut e = LineEdit::from("aé");
        assert_eq!(e.caret, 3);
        assert!(!e.key(KeyCode::Left));
        assert_eq!(e.caret, 1, "stepped over the 2-byte é");
        assert!(e.key(KeyCode::Char('x')));
        assert_eq!(&*e, "axé");
        assert!(!e.key(KeyCode::Home));
        assert!(!e.key(KeyCode::Backspace), "nothing before the caret");
        assert!(e.key(KeyCode::Delete));
        assert_eq!(&*e, "xé");
        assert!(!e.key(KeyCode::End));
        assert!(!e.key(KeyCode::Delete), "nothing after the caret");
        assert!(!e.key(KeyCode::Right));
        assert!(e.key(KeyCode::Backspace));
        assert_eq!((e.text.as_str(), e.caret), ("x", 1));
        assert!(!e.key(KeyCode::F(5)));
        e.caret = 0;
        e.replace(0, "SUM(");
        assert_eq!((e.text.as_str(), e.caret), ("SUM(x", 4));
    }

    /// H1-2: a fresh edit's first typed character replaces the old value;
    /// a caret or editing key first keeps it, and an unbound key changes
    /// nothing.
    #[test]
    fn a_fresh_edit_is_replaced_by_typing() {
        let mut e = LineEdit::replacing("300".into());
        assert_eq!(e.caret, 3);
        assert!(!e.key(KeyCode::F(5)));
        assert!(e.fresh, "an unbound key keeps it fresh");
        assert!(e.key(KeyCode::Char('3')));
        assert!(e.key(KeyCode::Char('5')));
        assert_eq!((e.text.as_str(), e.caret, e.fresh), ("35", 2, false));
        let mut e = LineEdit::replacing("300".into());
        assert!(!e.key(KeyCode::Left));
        assert!(e.key(KeyCode::Char('5')));
        assert_eq!(&*e, "3050", "moving the caret first edits in place");
        let mut e = LineEdit::replacing("300".into());
        assert!(e.key(KeyCode::Backspace));
        assert!(e.key(KeyCode::Char('5')));
        assert_eq!(&*e, "305", "Backspace edits the old value");
        let mut e = LineEdit::replacing("300".into());
        e.insert("PMT(");
        assert_eq!(&*e, "PMT(", "an insert replaces it too");
        assert!(!LineEdit::from("300").fresh);
    }
}
