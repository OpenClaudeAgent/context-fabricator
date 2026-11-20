//! Line-aware passage boundaries over immutable UTF-8 source bytes.

use std::ops::Range;

#[derive(Debug, Eq, PartialEq)]
pub struct Passage<'a> {
    /// Exact input text for evidence, with its original line endings.
    pub text: &'a str,
    /// Half-open offsets in the original source blob.
    pub bytes: Range<usize>,
    pub start_line: usize,
    pub end_line: usize,
}

/// Splits a source blob into consecutive passages within the byte budget.
///
/// Prefers complete lines and keeps CRLF together. An overlong line can produce
/// multiple passages with the same line number and distinct byte ranges.
/// Returns `None` for budgets below four bytes.
#[must_use]
pub fn passages(source: &str, byte_budget: usize) -> Option<impl Iterator<Item = Passage<'_>>> {
    if byte_budget < 4 {
        return None;
    }
    let mut start = 0;
    let mut line = 1;
    Some(std::iter::from_fn(move || {
        if start == source.len() {
            return None;
        }
        let end = boundary(source, start, byte_budget);
        let text = &source[start..end];
        let newlines = text.matches('\n').count();
        let passage = Passage {
            text,
            bytes: start..end,
            start_line: line,
            end_line: line + newlines - usize::from(text.ends_with('\n')),
        };
        start = end;
        line += newlines;
        Some(passage)
    }))
}

fn boundary(source: &str, start: usize, budget: usize) -> usize {
    let limit = source.floor_char_boundary(start.saturating_add(budget).min(source.len()));
    if limit == source.len() {
        return limit;
    }
    let prefix = &source[start..limit];
    if let Some(newline) = prefix.rfind('\n') {
        return start + newline + 1;
    }
    if prefix.ends_with('\r') && source.as_bytes().get(limit) == Some(&b'\n') {
        return limit - 1;
    }
    limit
}
