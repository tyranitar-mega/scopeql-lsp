//! Text-document helpers: mapping byte offsets to LSP positions.
//!
//! LSP uses 0-based `(line, character)` positions where `character` counts
//! UTF-16 code units. ScopeQL sources are mostly ASCII, but to stay correct for
//! multi-byte content (e.g. inside string literals) we still count columns in
//! UTF-16 code units.

use lsp_types::Position;
use lsp_types::Range;

/// Pre-computed line-start byte offsets for a document plus its length.
#[derive(Debug, Clone)]
pub struct LineIndex {
    line_starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut line_starts = vec![0];
        for (i, c) in text.char_indices() {
            if c == '\n' {
                line_starts.push(i + 1);
            }
        }
        let len = text.len();
        Self { line_starts, len }
    }

    fn line(&self, byte: usize) -> usize {
        match self.line_starts.binary_search(&byte) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        }
    }

    /// The 0-based line-start byte offsets of the document.
    pub fn line_starts(&self) -> &[usize] {
        &self.line_starts
    }

    /// Convert a byte offset into an LSP position.
    pub fn to_position(&self, byte: usize, text: &str) -> Position {
        let line = self.line(byte.min(self.len));
        let line_start = self.line_starts[line];
        let line_text = &text[line_start..byte.min(self.len)];
        Position {
            line: line as u32,
            character: utf16_len(line_text) as u32,
        }
    }

    pub fn to_range(&self, start: usize, end: usize, text: &str) -> Range {
        Range {
            start: self.to_position(start, text),
            end: self.to_position(end.min(self.len), text),
        }
    }
}

/// Number of UTF-16 code units a slice spans.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { c.len_utf16() }).sum()
}
