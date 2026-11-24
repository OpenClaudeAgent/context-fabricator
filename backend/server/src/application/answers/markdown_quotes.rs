//! Resolve a rendered Markdown line to its original bytes.

use std::{ops::Range, path::Path};

use pulldown_cmark::{Event, Parser, Tag};

use ctxfab_inference::generation::Reference;

use super::{Draft, Excerpt, VerifiedQuote, code_quotes, verified_quotes};

pub(super) fn reopen_line<'a>(source: &'a Excerpt, quote: &str) -> Option<VerifiedQuote<'a>> {
    let location = source.evidence.location.as_ref()?;
    let extension = Path::new(&location.path).extension()?.to_str()?;
    if !["md", "markdown"]
        .iter()
        .any(|supported| extension.eq_ignore_ascii_case(supported))
        || quote.contains(['\n', '\r'])
    {
        return None;
    }
    let text = &source.text;
    let mut markers: Vec<_> = Parser::new(text)
        .into_offset_iter()
        .filter_map(|(event, span)| delimiters(&event, span, text))
        .flatten()
        .collect();
    markers.sort_unstable_by_key(|marker| marker.start);
    let mut start = 0;
    let mut found = None;
    for line in text.split_inclusive('\n') {
        let span = start..start + line.len();
        start = span.end;
        if let Some(rendered) = without_delimiters(text, span.clone(), &markers)
            && rendered.trim() == quote
        {
            if found.is_some() {
                return None;
            }
            found = VerifiedQuote::exact_span(source, span);
        }
    }
    found
}

fn delimiters(event: &Event<'_>, span: Range<usize>, text: &str) -> Option<[Range<usize>; 2]> {
    let raw = text.get(span.clone())?;
    let width = match event {
        Event::Start(Tag::Strong) => 2,
        Event::Start(Tag::Emphasis) => 1,
        Event::Code(_) => raw.bytes().take_while(|byte| *byte == b'`').count(),
        _ => return None,
    };
    if width == 0 || raw.len() <= 2 * width || raw.contains(['\n', '\r']) {
        return None;
    }
    let head = raw.get(..width)?;
    let tail = raw.get(raw.len() - width..)?;
    if head != tail || !head.bytes().all(|c| matches!(c, b'*' | b'_' | b'`')) {
        return None;
    }
    Some([span.start..span.start + width, span.end - width..span.end])
}

fn without_delimiters(text: &str, line: Range<usize>, markers: &[Range<usize>]) -> Option<String> {
    let first = markers.partition_point(|marker| marker.start < line.start);
    let mut relevant = markers[first..]
        .iter()
        .take_while(|marker| marker.end <= line.end)
        .peekable();
    relevant.peek()?;
    let mut rendered = String::with_capacity(line.len());
    let mut start = line.start;
    for marker in relevant {
        rendered.push_str(text.get(start..marker.start)?);
        start = marker.end;
    }
    rendered.push_str(text.get(start..line.end)?);
    Some(rendered)
}

/// Translate only JSON-decoded literal LF markers into an exact unique source span.
pub(super) fn escaped_lf_span(source: &Excerpt, quote: &str) -> Option<Range<usize>> {
    let location = source.evidence.location.as_ref()?;
    let extension = Path::new(&location.path).extension()?.to_str()?;
    if source.evidence.kind != "current"
        || !["md", "markdown"]
            .iter()
            .any(|ext| extension.eq_ignore_ascii_case(ext))
        || quote.contains(['\n', '\r'])
        || !quote.contains(r"\n")
        || quote.split(r"\n").any(|piece| piece.contains('\\'))
        || source.text.contains(quote)
    {
        return None;
    }
    let decoded = quote.replace(r"\n", "\n");
    if decoded.trim() != decoded {
        return None;
    }
    let start = source.text.find(&decoded)?;
    (source.text.rfind(&decoded) == Some(start)).then_some(start..start + decoded.len())
}

/// A passage may begin inside a fence. Parse the pinned document, not its fragment.
pub(super) fn is_prose_span(source: &Excerpt, range: &Range<usize>, document: &str) -> bool {
    let Some(location) = source.evidence.location.as_ref() else {
        return false;
    };
    let begin = location.start_byte as usize;
    if document.get(begin..location.end_byte as usize) != Some(source.text.as_str()) {
        return false;
    }
    let span = begin + range.start..begin + range.end;
    let Some(text) = document.get(span.clone()) else {
        return false;
    };
    if document.find(text) != Some(span.start) || document.rfind(text) != Some(span.start) {
        return false;
    }
    let lines: Vec<_> = text
        .match_indices('\n')
        .map(|(offset, _)| span.start + offset)
        .collect();
    let mut covered = span.start;
    for (event, part) in Parser::new(document).into_offset_iter() {
        if part.end <= span.start || span.end <= part.start {
            continue;
        }
        match event {
            Event::Code(_) if lines.iter().any(|offset| part.contains(offset)) => return false,
            Event::Start(Tag::Paragraph) => {
                let start = part.start.max(span.start);
                if start > covered && !document[covered..start].trim().is_empty() {
                    return false;
                }
                covered = covered.max(part.end.min(span.end));
            }
            _ => {}
        }
    }
    covered == span.end
}

pub(super) fn published_root(sources: &[Excerpt]) -> Option<&Excerpt> {
    let head = sources.first()?;
    let owner = head.repository_id?;
    let loc = head.evidence.location.as_ref()?;
    if !["readme.md", "readme.markdown"]
        .iter()
        .any(|name| loc.path.eq_ignore_ascii_case(name))
    {
        return None;
    }
    sources
        .iter()
        .all(|source| {
            let hit = &source.evidence;
            source.repository_id == Some(owner)
                && hit.kind == "current"
                && hit.access == "available"
                && hit.source == head.evidence.source
                && hit.revision == head.evidence.revision
                && hit.tracked_ref.is_some()
                && hit.tracked_ref == head.evidence.tracked_ref
                && hit
                    .location
                    .as_ref()
                    .is_some_and(|other| other.path == loc.path && other.blob == loc.blob)
        })
        .then_some(head)
}

pub(super) fn relocate(sources: &[Excerpt], draft: &Draft, document: &str) -> Option<Draft> {
    published_root(sources)?;
    let mut changed = false;
    let mut citations = Vec::new();
    for reference in &draft.citations {
        let index = reference.passage.checked_sub(1)?;
        let current = sources.get(index)?;
        let quote = reference.quote.trim();
        let same_slot = Draft {
            answer: String::new(),
            citations: vec![Reference {
                passage: 1,
                quote: quote.into(),
            }],
        };
        let passage = if verified_quotes(std::slice::from_ref(current), &same_slot).is_some()
            || escaped_lf_span(current, quote)
                .is_some_and(|span| is_prose_span(current, &span, document))
        {
            reference.passage
        } else {
            let mut matches = sources.iter().enumerate().filter(|&(slot, candidate)| {
                slot != index && equivalent_prose(candidate, quote, document)
            });
            let (slot, _) = matches.next()?;
            if matches.next().is_some() {
                return None;
            }
            changed = true;
            slot + 1
        };
        citations.push(Reference {
            passage,
            quote: reference.quote.clone(),
        });
    }
    changed.then(|| Draft {
        answer: draft.answer.clone(),
        citations,
    })
}

fn equivalent_prose(source: &Excerpt, quote: &str, document: &str) -> bool {
    if let Some(range) = VerifiedQuote::literal(source, quote)
        .and_then(|literal| literal.byte_range())
        .or_else(|| escaped_lf_span(source, quote))
    {
        return is_prose_span(source, &range, document)
            && source
                .text
                .get(range)
                .is_some_and(|text| unique_layout(document, text));
    }
    let only = std::slice::from_ref(source);
    let Some(wrapped) = code_quotes::reopen_wrapped(only, source, quote) else {
        return false;
    };
    let [proof] = wrapped.as_slice() else {
        return false;
    };
    let Some(range) = proof.byte_range() else {
        return false;
    };
    let Some(start) = source
        .evidence
        .location
        .as_ref()
        .map(|loc| loc.start_byte as usize)
    else {
        return false;
    };
    if !is_prose_span(source, &range, document) {
        return false;
    }
    // Ordinary Markdown soft line wraps may disappear. Hard breaks, fences and
    // paragraph boundaries must stay distinct.
    let soft: Vec<_> = Parser::new(document)
        .into_offset_iter()
        .filter_map(|(event, span)| matches!(event, Event::SoftBreak).then_some(span))
        .collect();
    if proof.text.match_indices('\n').any(|(i, _)| {
        !soft
            .iter()
            .any(|line| line.contains(&(start + range.start + i)))
    }) {
        return false;
    }
    unique_layout(document, quote)
}

fn unique_layout(document: &str, quote: &str) -> bool {
    let folded = |text: &str| {
        text.lines()
            .map(str::trim_ascii)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let folded_document = folded(document);
    let folded_quote = folded(quote);
    folded_document
        .find(&folded_quote)
        .is_some_and(|start| folded_document.rfind(&folded_quote) == Some(start))
}

#[cfg(test)]
mod escaped_lf_tests {
    use super::*;
    use crate::{application::SearchHit, domain::corpus::snapshot::Location};

    fn exhibit(text: &str, start: usize, path: &str) -> Excerpt {
        let begin = u32::try_from(start).unwrap_or(u32::MAX);
        let end = u32::try_from(start + text.len()).unwrap_or(u32::MAX);
        Excerpt {
            evidence: SearchHit {
                source: format!("github:1:{path}"),
                revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                kind: "current".into(),
                tracked_ref: Some("refs/heads/main".into()),
                title: path.into(),
                access: "available",
                location: Some(Location {
                    path: path.into(),
                    blob: "blob".into(),
                    file_url: format!(
                        "https://github.com/example/project/blob/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/{path}"
                    ),
                    start_byte: begin,
                    end_byte: end,
                    start_line: 1,
                    end_line: 3,
                }),
            },
            text: text.into(),
            repository_id: Some(1),
        }
    }

    #[test]
    fn root_readme_relocates_only_a_unique_proven_submitted_paragraph()
    -> Result<(), Box<dyn std::error::Error>> {
        let head = "Other visible documentation.\n\n";
        let tail = "A single paragraph starts here and\nwraps across one ordinary line.\n";
        let quote = "A single paragraph starts here and wraps across one ordinary line.";
        let draft = |passage, quote: &str| Draft {
            answer: "Test answer.".into(),
            citations: vec![Reference {
                passage,
                quote: quote.into(),
            }],
        };
        let first = exhibit(head, 0, "README.md");
        let second = exhibit(tail, head.len(), "README.md");
        let admitted = [first, second];
        let doc = format!("{head}{tail}");
        assert!(verified_quotes(&admitted, &draft(1, quote)).is_none());
        let moved = relocate(&admitted, &draft(1, quote), &doc).ok_or("unique proof")?;
        assert_eq!(moved.citations[0].passage, 2);
        let original = verified_quotes(&admitted, &moved).ok_or("existing proof")?;
        assert!(original[0].text.contains("and\nwraps"));
        assert!(relocate(&admitted, &draft(2, quote), &doc).is_none());
        assert!(relocate(&admitted[..1], &draft(1, quote), &doc).is_none());
        assert!(relocate(&admitted, &draft(3, quote), &doc).is_none());
        assert!(
            relocate(
                &admitted,
                &draft(1, &quote.replace("ordinary", "invented")),
                &doc
            )
            .is_none()
        );
        assert!(relocate(&admitted, &draft(1, quote), &format!("{doc}\n{quote}\n")).is_none());
        for invalid in [
            format!("```markdown\n{tail}```\n"),
            tail.replace("and\n", "and  \n"),
            tail.replace("and\n", "and\n\n"),
        ] {
            let sources = [
                exhibit(head, 0, "README.md"),
                exhibit(&invalid, head.len(), "README.md"),
            ];
            assert!(relocate(&sources, &draft(1, quote), &format!("{head}{invalid}")).is_none());
        }
        let foreign = [
            exhibit(head, 0, "README.md"),
            exhibit(tail, head.len(), "docs/README.md"),
        ];
        assert!(relocate(&foreign, &draft(1, quote), &doc).is_none());
        let mut other_owner = [
            exhibit(head, 0, "README.md"),
            exhibit(tail, head.len(), "README.md"),
        ];
        other_owner[1].repository_id = Some(888);
        assert!(relocate(&other_owner, &draft(1, quote), &doc).is_none());
        let escaped = [
            exhibit(head, 0, "README.md"),
            exhibit(RAW, head.len(), "README.md"),
        ];
        let corrected = relocate(&escaped, &draft(1, QUOTE), &format!("{head}{RAW}"))
            .ok_or("existing LF repair")?;
        assert_eq!(corrected.citations[0].passage, 2);
        Ok(())
    }

    const QUOTE: &str = r"First sentence has `an.Identifier`.\n\nSecond sentence has text.";
    const RAW: &str = "First sentence has `an.Identifier`.\n\nSecond sentence has text.";

    #[test]
    fn unique_markdown_prose_repairs_only_escaped_line_feeds_with_inline_identifier()
    -> Result<(), Box<dyn std::error::Error>> {
        let prefix = "# Header\n\n";
        let complete = format!("{prefix}{RAW}\n");
        let mut source = exhibit(RAW, prefix.len(), "README.md");
        let location = source.evidence.location.as_mut().ok_or("coordinates")?;
        location.start_line = 3;
        location.end_line = 5;
        let range = escaped_lf_span(&source, QUOTE).ok_or("find canonical span")?;
        assert!(is_prose_span(&source, &range, &complete));
        let repaired = VerifiedQuote::exact_span(&source, range).ok_or("canonical quote")?;
        assert_eq!(repaired.text, RAW);
        assert!(
            repaired
                .line_url(&source)
                .is_some_and(|url| url.ends_with("#L3-L5"))
        );
        assert!(escaped_lf_span(&source, &QUOTE.replace("text.", "fiction.")).is_none());
        assert!(escaped_lf_span(&source, &QUOTE.replace(r"\n", r"\r\n")).is_none());
        assert!(escaped_lf_span(&source, &QUOTE.replace(r"\n", r"\\n")).is_none());
        assert!(escaped_lf_span(&exhibit(RAW, 0, "example.java"), QUOTE).is_none());
        assert!(escaped_lf_span(&exhibit(QUOTE, 0, "README.md"), QUOTE).is_none());
        assert!(escaped_lf_span(&exhibit("a\na\na", 0, "README.md"), r"a\na").is_none());
        assert!(!is_prose_span(
            &exhibit("a\na", 0, "README.md"),
            &(0..3),
            "a\na\na"
        ));
        let other = format!("{RAW}\n\n{RAW}");
        assert!(escaped_lf_span(&exhibit(&other, 0, "README.md"), QUOTE).is_none());
        let mismatched = complete.replace("Identifier", "Different");
        assert!(!is_prose_span(&source, &(0..RAW.len()), &mismatched));
        Ok(())
    }

    #[test]
    fn refuses_fenced_indented_inline_and_duplicate_document_occurrences()
    -> Result<(), Box<dyn std::error::Error>> {
        let fence = "Opening paragraph.\n\n```text\n";
        let source = exhibit(RAW, fence.len(), "README.md");
        let range = escaped_lf_span(&source, QUOTE).ok_or("candidate span")?;
        assert!(!is_prose_span(
            &source,
            &range,
            &format!("{fence}{RAW}\n```")
        ));
        assert!(!is_prose_span(
            &exhibit(RAW, 0, "README.md"),
            &(0..RAW.len()),
            &format!("{RAW}\n\n{RAW}")
        ));
        let inline = "Read `one\nside` in this example.";
        let source = exhibit(inline, 0, "README.md");
        let range = escaped_lf_span(&source, r"Read `one\nside` in this example.")
            .ok_or("inline candidate")?;
        assert!(!is_prose_span(&source, &range, inline));
        let indented = "    Indented language output.\n    Continuation.";
        let source = exhibit(indented, 0, "README.md");
        let range = escaped_lf_span(&source, r"Indented language output.\n    Continuation.")
            .ok_or("indented candidate")?;
        assert!(!is_prose_span(&source, &range, indented));
        Ok(())
    }
}
