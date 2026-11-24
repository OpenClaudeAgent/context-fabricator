//! Deterministic line hunks projected only from independently reverified blobs.
use super::comparison::{Change, File, Side, side};
use crate::domain::github::Error;
use serde::{Deserialize, Serialize};
use similar::{Algorithm, capture_diff_slices, group_diff_ops};

const MAX_LINES: usize = 1_024;
const MAX_WORK: usize = 1_048_576;
const MAX_HUNKS: usize = 64;
const MAX_HUNK_BYTES: usize = 16 * 1_024;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Anchor {
    pub path: String,
    pub blob: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Hunk {
    pub before: Option<Anchor>,
    pub after: Option<Anchor>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct AlignedLine {
    pub before: Anchor,
    pub after: Anchor,
}

const CONTEXT_LINES: usize = 4;
const CONTEXT_LINE_BYTES: usize = 384;

/// Derive complete unchanged neighbors after the owner has verified both blobs.
/// Only corresponding offsets adjacent to this hunk can certify equality.
pub(crate) fn adjacent(
    projection: &Projection,
    hunk: &Hunk,
    old: &[u8],
    new: &[u8],
) -> Vec<AlignedLine> {
    let (Some(before), Some(after)) = (&hunk.before, &hunk.after) else {
        return Vec::new();
    };
    if before.path != after.path {
        return Vec::new();
    }
    let (Ok(old), Ok(new)) = (std::str::from_utf8(old), std::str::from_utf8(new)) else {
        return Vec::new();
    };
    let old_offsets = offsets(old);
    let new_offsets = offsets(new);
    let complete = |anchor: &Anchor, offsets: &[usize]| {
        anchor
            .start_line
            .checked_sub(1)
            .and_then(|line| offsets.get(line))
            == Some(&anchor.start_byte)
            && offsets.get(anchor.end_line) == Some(&anchor.end_byte)
    };
    if !complete(before, &old_offsets) || !complete(after, &new_offsets) {
        return Vec::new();
    }
    let unchanged = |before_line, after_line| {
        if projection.hunks.iter().any(|hunk| {
            let changed = |anchor: Option<&Anchor>, line| {
                anchor.is_some_and(|a| (a.start_line..=a.end_line).contains(&line))
            };
            changed(hunk.before.as_ref(), before_line) || changed(hunk.after.as_ref(), after_line)
        }) {
            return None;
        }
        let before = context_line(before, old, &old_offsets, before_line)?;
        let after = context_line(after, new, &new_offsets, after_line)?;
        (before.text == after.text).then_some(AlignedLine { before, after })
    };
    let preceding = (1..=CONTEXT_LINES)
        .map_while(|distance| {
            unchanged(
                before.start_line.checked_sub(distance)?,
                after.start_line.checked_sub(distance)?,
            )
        })
        .collect::<Vec<_>>();
    preceding
        .into_iter()
        .rev()
        .chain(
            (1..=CONTEXT_LINES).map_while(|distance| {
                unchanged(before.end_line + distance, after.end_line + distance)
            }),
        )
        .collect()
}

fn context_line(anchor: &Anchor, text: &str, offsets: &[usize], line: usize) -> Option<Anchor> {
    let start = *offsets.get(line.checked_sub(1)?)?;
    let end = *offsets.get(line)?;
    let original = text.get(start..end)?;
    if original.len() > CONTEXT_LINE_BYTES {
        return None;
    }
    Some(Anchor {
        path: anchor.path.clone(),
        blob: anchor.blob.clone(),
        start_byte: start,
        end_byte: end,
        start_line: line,
        end_line: line,
        text: original.to_owned(),
    })
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Projection {
    pub coverage: String,
    pub hunks: Vec<Hunk>,
}

/// Project paired line anchors from a comparison change, reopening both sides.
/// # Errors
/// Rejects a purported text side that fails immutable blob verification.
pub fn project(change: &Change) -> Result<Projection, Error> {
    if change.kind == "renamed" {
        return Ok(Projection {
            coverage: "pathOnly".to_owned(),
            hunks: Vec::new(),
        });
    }
    let Some(old) = verified_text(change.before.as_ref())? else {
        return Ok(unavailable());
    };
    let Some(new) = verified_text(change.after.as_ref())? else {
        return Ok(unavailable());
    };
    let old_offsets = offsets(old);
    let new_offsets = offsets(new);
    let old_lines = old_offsets.len() - 1;
    let new_lines = new_offsets.len() - 1;
    if old_lines > MAX_LINES
        || new_lines > MAX_LINES
        || old_lines.saturating_mul(new_lines) > MAX_WORK
    {
        return Ok(limited());
    }
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let groups = group_diff_ops(
        capture_diff_slices(Algorithm::Myers, &old_lines, &new_lines),
        0,
    );
    if groups.len() > MAX_HUNKS {
        return Ok(limited());
    }
    let mut hunks = Vec::with_capacity(groups.len());
    for group in groups {
        let Some(first) = group.first() else { continue };
        let Some(last) = group.last() else { continue };
        let old_range = first.old_range().start..last.old_range().end;
        let new_range = first.new_range().start..last.new_range().end;
        let before = anchor(change.before.as_ref(), old, &old_offsets, old_range)?;
        let after = anchor(change.after.as_ref(), new, &new_offsets, new_range)?;
        if before
            .as_ref()
            .is_some_and(|a| a.end_byte - a.start_byte > MAX_HUNK_BYTES)
            || after
                .as_ref()
                .is_some_and(|a| a.end_byte - a.start_byte > MAX_HUNK_BYTES)
        {
            return Ok(limited());
        }
        hunks.push(Hunk { before, after });
    }
    Ok(Projection {
        coverage: "complete".to_owned(),
        hunks,
    })
}

fn verified_text(input: Option<&Side>) -> Result<Option<&str>, Error> {
    let Some(input) = input else {
        return Ok(Some(""));
    };
    if input.coverage != "fullBlob" {
        return Ok(None);
    }
    let text = input.text.as_deref().ok_or(Error::Upstream)?;
    let file = File {
        path: input.path.clone(),
        blob: input.blob.clone(),
        size: input.bytes,
        mode: input.mode.clone(),
    };
    if side(&file, text.as_bytes().to_vec())?.coverage != "fullBlob" {
        return Err(Error::Upstream);
    }
    Ok(Some(text))
}

fn offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    let mut position = 0;
    for line in text.split_inclusive('\n') {
        position += line.len();
        offsets.push(position);
    }
    offsets
}

fn anchor(
    side: Option<&Side>,
    text: &str,
    offsets: &[usize],
    lines: std::ops::Range<usize>,
) -> Result<Option<Anchor>, Error> {
    if lines.is_empty() {
        return Ok(None);
    }
    let input = side.ok_or(Error::Upstream)?;
    let start = *offsets.get(lines.start).ok_or(Error::Upstream)?;
    let end = *offsets.get(lines.end).ok_or(Error::Upstream)?;
    let exact = text.get(start..end).ok_or(Error::Upstream)?;
    Ok(Some(Anchor {
        path: input.path.clone(),
        blob: input.blob.clone(),
        start_byte: start,
        end_byte: end,
        start_line: lines.start + 1,
        end_line: lines.end,
        text: exact.to_owned(),
    }))
}
fn limited() -> Projection {
    Projection {
        coverage: "limit".to_owned(),
        hunks: Vec::new(),
    }
}
fn unavailable() -> Projection {
    Projection {
        coverage: "contentUnavailable".to_owned(),
        hunks: Vec::new(),
    }
}

/// Verify a retained anchor against its immutable blob and selected side.
/// # Errors
/// Rejects a forged blob, path, range, line coordinate or quote.
pub fn reopen(anchor: &Anchor, source: &[u8], selected: &Side) -> Result<(), Error> {
    if anchor.path != selected.path || anchor.blob != selected.blob {
        return Err(Error::Upstream);
    }
    let file = File {
        path: selected.path.clone(),
        blob: selected.blob.clone(),
        size: selected.bytes,
        mode: selected.mode.clone(),
    };
    let verified = side(&file, source.to_vec())?;
    let text = verified.text.as_deref().ok_or(Error::Upstream)?;
    let quote = text
        .get(anchor.start_byte..anchor.end_byte)
        .ok_or(Error::Upstream)?;
    if quote.is_empty() || quote != anchor.text {
        return Err(Error::Upstream);
    }
    let prefix = text.get(..anchor.start_byte).ok_or(Error::Upstream)?;
    let start_line = 1 + prefix.bytes().filter(|b| *b == b'\n').count();
    let end_line = start_line + quote.bytes().filter(|b| *b == b'\n').count()
        - usize::from(quote.ends_with('\n'));
    if (anchor.start_line, anchor.end_line) != (start_line, end_line) {
        return Err(Error::Upstream);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::project;
    use crate::domain::github::history::comparison::{Change, File, side};
    use sha1::{Digest, Sha1};
    use std::fmt::Write;

    fn make_side(path: &str, text: &str) -> Result<super::Side, crate::domain::github::Error> {
        let mut hash = Sha1::new();
        hash.update(format!("blob {}\0", text.len()));
        hash.update(text.as_bytes());
        let mut id = String::new();
        for byte in hash.finalize() {
            let _ = write!(id, "{byte:02x}");
        }
        let file = File {
            path: path.to_owned(),
            blob: id,
            size: Some(text.len() as u64),
            mode: "100644".to_owned(),
        };
        side(&file, text.as_bytes().to_vec())
    }

    #[test]
    fn paired_hunks_reopen_exact_crlf_unicode_and_missing_terminal_newline()
    -> Result<(), Box<dyn std::error::Error>> {
        let old = "début\r\nrepeat\r\nrepeat\r\nold";
        let new = "début\r\nrepeat\r\nchanged\r\nnew";
        let change = Change {
            kind: "modified".to_owned(),
            before: Some(make_side("old.txt", old)?),
            after: Some(make_side("new.txt", new)?),
        };
        let result = project(&change)?;
        assert_eq!(result.coverage, "complete");
        assert_eq!(result.hunks.len(), 1);
        for (anchor, source) in [
            (result.hunks[0].before.as_ref(), old),
            (result.hunks[0].after.as_ref(), new),
        ] {
            let anchor = anchor.ok_or("expected paired anchor")?;
            assert_eq!(&source[anchor.start_byte..anchor.end_byte], anchor.text);
            assert_eq!(anchor.start_line, 3);
            assert_eq!(anchor.end_line, 4);
        }
        Ok(())
    }

    #[test]
    fn rejects_forged_bytes_and_returns_explicit_bounded_coverage()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut before = make_side("file", "original\n")?;
        before.text = Some("tampered\n".to_owned());
        let change = Change {
            kind: "modified".to_owned(),
            before: Some(before),
            after: Some(make_side("file", "changed\n")?),
        };
        assert!(project(&change).is_err());
        let long = "x\n".repeat(1025);
        let change = Change {
            kind: "added".to_owned(),
            before: None,
            after: Some(make_side("file", &long)?),
        };
        let result = project(&change)?;
        assert_eq!(result.coverage, "limit");
        assert!(result.hunks.is_empty());
        Ok(())
    }
    #[test]
    fn bare_cr_stays_within_corpus_line_and_bounds_are_enforced()
    -> Result<(), Box<dyn std::error::Error>> {
        let old = format!("{}old", "a\r".repeat(1025));
        let new = format!("{}new", "a\r".repeat(1025));
        let change = Change {
            kind: "modified".to_owned(),
            before: Some(make_side("file", &old)?),
            after: Some(make_side("file", &new)?),
        };
        let result = project(&change)?;
        assert_eq!(result.coverage, "complete");
        let before = result.hunks[0].before.as_ref().ok_or("before")?;
        assert_eq!((before.start_line, before.end_line), (1, 1));
        assert_eq!(&old[before.start_byte..before.end_byte], before.text);
        Ok(())
    }
}
