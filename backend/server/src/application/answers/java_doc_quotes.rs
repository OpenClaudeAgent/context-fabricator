//! Verify exact same-slot Java documentation paragraphs against pinned source.

use std::{ops::Range, path::Path};

use crate::domain::java::parse_source;

use super::Excerpt;

const MAX_BYTES: usize = 600;
const MAX_LINES: usize = 4;

pub(super) fn paragraph_span(source: &Excerpt, quote: &str) -> Option<Range<usize>> {
    let location = source.evidence.location.as_ref()?;
    if source.evidence.kind != "current"
        || source.evidence.access != "available"
        || Path::new(&location.path).extension()? != "java"
        || quote.len() > MAX_BYTES
        || quote.contains(['\r', '\n'])
        || !source.text.contains("/**")
    {
        return None;
    }
    let mut offsets = 0;
    let mut previous_boundary = false;
    let mut paragraph = None;
    let mut found = None;
    for row in source.text.split_inclusive('\n') {
        let offset = offsets;
        offsets += row.len();
        let row = row.trim_ascii();
        if let Some(body) = row
            .strip_prefix("* ")
            .filter(|body| !body.is_empty() && *body != "<p>" && *body != "</p>")
        {
            if previous_boundary {
                paragraph = Some((offset, 0, String::new()));
            }
            if let Some((start, count, folded)) = &mut paragraph {
                if *count == MAX_LINES || offsets - *start > MAX_BYTES || !plain_line(body) {
                    paragraph = None;
                } else {
                    if *count > 0 {
                        folded.push(' ');
                    }
                    folded.push_str(body);
                    *count += 1;
                }
            }
            previous_boundary = false;
        } else {
            let boundary = matches!(row, "*" | "*<p>" | "* <p>" | "*/" | "*</p>" | "* </p>");
            if let Some((start, count, folded)) = paragraph.take()
                && boundary
                && count >= 2
                && folded == quote
                && found.replace(start..offset).is_some()
            {
                return None;
            }
            previous_boundary = boundary || row == "/**";
        }
    }
    found
}

fn plain_line(body: &str) -> bool {
    !body.starts_with(['@', '<', ' ', '\t'])
        && body.split("{@").skip(1).all(|tag| tag.contains('}'))
}

pub(super) fn is_javadoc_span(source: &Excerpt, range: &Range<usize>, document: &str) -> bool {
    let Some(location) = &source.evidence.location else {
        return false;
    };
    let start = location.start_byte as usize;
    let end = location.end_byte as usize;
    if document.get(start..end) != Some(source.text.as_str())
        || source.text.get(range.clone()).is_none()
    {
        return false;
    }
    let Some(tree) = parse_source(document) else {
        return false;
    };
    let Some(node) = tree
        .root_node()
        .descendant_for_byte_range(start + range.start, start + range.end)
    else {
        return false;
    };
    if node.kind() != "block_comment" || node.start_byte() < start || node.end_byte() > end {
        return false;
    }
    let comment = &document[node.byte_range()];
    let folded_case = comment.to_ascii_lowercase();
    comment.starts_with("/**")
        && !["<pre", "<code", "{@snippet", "```", "~~~"]
            .iter()
            .any(|markup| folded_case.contains(markup))
}
