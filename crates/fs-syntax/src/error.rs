//! Diagnostics.

use crate::span::{LineIndex, Span};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub span: Span,
    pub message: String,
}

impl ParseError {
    pub fn new(span: Span, message: impl Into<String>) -> Self {
        ParseError {
            span,
            message: message.into(),
        }
    }

    /// Render as `file:line:col: message` with the offending source line and a caret.
    pub fn render(&self, file: &str, src: &str) -> String {
        let index = LineIndex::new(src);
        let lc = index.line_col(self.span.start);
        let line_text = src.lines().nth(lc.line as usize - 1).unwrap_or("");
        let caret_len = self
            .span
            .len()
            .max(1)
            .min(line_text.len().saturating_sub(lc.col as usize - 1).max(1));
        format!(
            "{file}:{lc}: error: {msg}\n  {line}\n  {pad}{caret}",
            msg = self.message,
            line = line_text,
            pad = " ".repeat(lc.col as usize - 1),
            caret = "^".repeat(caret_len),
        )
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}", self.message, self.span)
    }
}

impl std::error::Error for ParseError {}

pub type ParseResult<T> = Result<T, ParseError>;
