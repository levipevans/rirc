//! The single-line editor used by text and command modes.

#[derive(Debug, Default)]
pub struct Input {
    pub chars: Vec<char>,
    pub cursor: usize,
    history: Vec<String>,
    hist: Option<usize>,
    stash: String,
}

impl Input {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn set_text(&mut self, text: &str) {
        self.set_chars(text);
        self.hist = None;
    }

    fn set_chars(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.cursor = self.chars.len();
    }

    pub fn clear(&mut self) {
        self.chars.clear();
        self.cursor = 0;
        self.hist = None;
    }

    pub fn take(&mut self) -> String {
        let text = self.text();
        if !text.trim().is_empty() && self.history.last().map(String::as_str) != Some(text.as_str())
        {
            self.history.push(text.clone());
            if self.history.len() > 200 {
                self.history.remove(0);
            }
        }
        self.clear();
        text
    }

    pub fn insert(&mut self, c: char) {
        let at = self.cursor.min(self.chars.len());
        self.chars.insert(at, c);
        self.cursor = at + 1;
    }

    pub fn insert_str(&mut self, text: &str) {
        for c in text.chars() {
            if c == '\r' || c == '\n' {
                continue;
            }
            self.insert(c);
        }
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.chars.len();
    }

    pub fn delete_word(&mut self) {
        while self.cursor > 0 && self.chars[self.cursor - 1].is_whitespace() {
            self.backspace();
        }
        while self.cursor > 0 && !self.chars[self.cursor - 1].is_whitespace() {
            self.backspace();
        }
    }

    pub fn clear_before(&mut self) {
        self.chars.drain(0..self.cursor);
        self.cursor = 0;
    }

    /// Start of the whitespace-delimited word touching the cursor.
    pub fn word_start(&self) -> usize {
        let mut i = self.cursor.min(self.chars.len());
        while i > 0 && !self.chars[i - 1].is_whitespace() {
            i -= 1;
        }
        i
    }

    pub fn word_prefix(&self) -> String {
        let start = self.word_start();
        self.chars[start..self.cursor.min(self.chars.len())]
            .iter()
            .collect()
    }

    pub fn replace_word(&mut self, start: usize, replacement: &str) {
        let end = self.cursor.min(self.chars.len());
        let start = start.min(end);
        self.chars.splice(start..end, replacement.chars());
        self.cursor = start + replacement.chars().count();
    }

    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.hist {
            None => {
                self.stash = self.text();
                let idx = self.history.len() - 1;
                self.hist = Some(idx);
                let text = self.history[idx].clone();
                self.set_chars(&text);
            }
            Some(idx) => {
                if idx > 0 {
                    self.hist = Some(idx - 1);
                    let text = self.history[idx - 1].clone();
                    self.set_chars(&text);
                }
            }
        }
    }

    pub fn history_next(&mut self) {
        match self.hist {
            None => {}
            Some(idx) => {
                if idx + 1 < self.history.len() {
                    self.hist = Some(idx + 1);
                    let text = self.history[idx + 1].clone();
                    self.set_text(&text);
                    self.hist = Some(idx + 1);
                } else {
                    let stash = self.stash.clone();
                    self.hist = None;
                    self.set_text(&stash);
                }
            }
        }
    }
}
