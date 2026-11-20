//! Reopen code copied with line breaks collapsed using the original source bytes.

use std::ops::Range;

use super::{Excerpt, MIN_QUOTE_BYTES};

const CONTEXTUAL_QUOTE_BYTES: usize = 600;

/// Include one preceding source line when an exact code quote has a unique
/// position in its passage. The whole returned excerpt is rechecked after generation.
pub(super) fn with_context<'a>(source: &'a Excerpt, quote: &'a str) -> &'a str {
    let Some(location) = &source.evidence.location else {
        return quote;
    };
    let Some(start) = unique_match(&source.text, quote) else {
        return quote;
    };
    let Some(line_break) = source.text[..start].rfind('\n') else {
        return quote;
    };
    let previous_line = match source.text[..line_break].rfind('\n') {
        Some(line) => line + 1,
        None if location.start_byte == 0 => 0,
        None => return quote,
    };
    let end = start + quote.len();
    if end - previous_line > CONTEXTUAL_QUOTE_BYTES {
        return quote;
    }
    &source.text[previous_line..end]
}

struct Line<'a> {
    excerpt: &'a Excerpt,
    flattened: Range<usize>,
    source: usize,
}

/// Recover a unique literal sequence split across original source lines. Only
/// the line breaks and surrounding indentation may differ from the model's quote.
pub(super) fn reopen_wrapped<'a>(
    sources: &'a [Excerpt],
    cited: &'a Excerpt,
    quote: &str,
) -> Option<Vec<(&'a Excerpt, &'a str)>> {
    let location = cited.evidence.location.as_ref()?;
    let mut related: Vec<_> = sources
        .iter()
        .filter_map(|source| {
            let position = source.evidence.location.as_ref()?;
            (source.evidence.source == cited.evidence.source
                && source.evidence.revision == cited.evidence.revision
                && source.evidence.tracked_ref == cited.evidence.tracked_ref
                && source.evidence.kind == cited.evidence.kind
                && position.blob == location.blob
                && position.path == location.path
                && (position.start_byte == location.start_byte
                    || (position.end_byte == location.start_byte
                        && line_break(&source.text, &cited.text))
                    || (position.start_byte == location.end_byte
                        && line_break(&cited.text, &source.text))))
            .then_some((position.start_byte, source))
        })
        .collect();
    related.sort_by_key(|(position, _)| *position);

    let mut flattened = String::new();
    let mut lines = Vec::new();
    for (_, excerpt) in related {
        let mut offset = 0;
        for raw in excerpt.text.split_inclusive('\n') {
            let text = raw.trim_ascii();
            if !text.is_empty() {
                if !flattened.is_empty() {
                    flattened.push(' ');
                }
                let start = flattened.len();
                flattened.push_str(text);
                lines.push(Line {
                    excerpt,
                    flattened: start..flattened.len(),
                    source: offset + raw.len() - raw.trim_ascii_start().len(),
                });
            }
            offset += raw.len();
        }
    }
    let quote = quote
        .lines()
        .map(str::trim_ascii)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if quote.len() < MIN_QUOTE_BYTES {
        return None;
    }
    let start = unique_match(&flattened, &quote)?;
    let end = start + quote.len();

    let mut slices: Vec<(&Excerpt, Range<usize>)> = Vec::new();
    let mut matches = 0;
    for line in lines {
        let overlap = start.max(line.flattened.start)..end.min(line.flattened.end);
        if overlap.is_empty() {
            continue;
        }
        matches += 1;
        let range = line.source + overlap.start - line.flattened.start
            ..line.source + overlap.end - line.flattened.start;
        if let Some((excerpt, previous)) = slices.last_mut()
            && std::ptr::eq(*excerpt, line.excerpt)
        {
            previous.end = range.end;
        } else {
            slices.push((line.excerpt, range));
        }
    }
    if matches < 2
        || !slices
            .iter()
            .any(|(excerpt, _)| std::ptr::eq(*excerpt, cited))
    {
        return None;
    }
    slices
        .into_iter()
        .map(|(excerpt, range)| Some((excerpt, excerpt.text.get(range)?)))
        .collect()
}

fn line_break(left: &str, right: &str) -> bool {
    left.ends_with('\n') || right.starts_with('\n') || right.starts_with("\r\n")
}

fn unique_match(source: &str, quote: &str) -> Option<usize> {
    let start = source.find(quote)?;
    let next = start + quote.chars().next()?.len_utf8();
    (!source.get(next..)?.contains(quote)).then_some(start)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use ctxfab_inference::generation::{Draft, Reference};

    use super::super::verified_quotes;
    use super::{Excerpt, reopen_wrapped, with_context};
    use crate::application::SearchHit;
    use crate::domain::corpus::snapshot::Location;

    fn excerpt(
        text: &str,
        start_byte: u32,
        line: u32,
        tracked_ref: &str,
    ) -> Result<Excerpt, Box<dyn Error>> {
        Ok(Excerpt {
            text: text.into(),
            evidence: SearchHit {
                source: "github:42:src/lib.rs".into(),
                revision: "pinned".into(),
                kind: "current".into(),
                tracked_ref: Some(tracked_ref.into()),
                title: "src/lib.rs".into(),
                access: "available",
                location: Some(Location {
                    path: "src/lib.rs".into(),
                    blob: "source-blob".into(),
                    file_url: "https://example.invalid/pinned/src/lib.rs".into(),
                    start_byte,
                    end_byte: start_byte + u32::try_from(text.len())?,
                    start_line: line,
                    end_line: line + u32::try_from(text.lines().count())? - 1,
                }),
            },
        })
    }

    #[test]
    fn wrapped_code_reopens_original_text_and_deduplicates_citations() -> Result<(), Box<dyn Error>>
    {
        let leading = "if allowed(value)\r\n    || value == ':'\r\n";
        let code = [
            excerpt(
                "    || value == '+' {\n",
                u32::try_from(leading.len())?,
                3,
                "main",
            )?,
            excerpt(leading, 0, 1, "main")?,
        ];
        let draft = Draft {
            answer: "The filter permits colon and plus.".into(),
            citations: [1, 2]
                .map(|passage| Reference {
                    passage,
                    quote: "if allowed(value) || value == ':' || value == '+'".into(),
                })
                .into(),
        };
        let cited = verified_quotes(&code, &draft).ok_or("code bytes")?;
        assert_eq!(cited.len(), 2);
        assert!(cited[0].0.evidence.same_evidence(&code[1].evidence));
        assert_eq!(cited[0].1, leading.trim_end());
        assert!(cited[1].0.evidence.same_evidence(&code[0].evidence));
        assert_eq!(cited[1].1, "|| value == '+'");
        Ok(())
    }

    #[test]
    fn wrapped_code_requires_one_literal_match_under_the_cited_identity()
    -> Result<(), Box<dyn Error>> {
        let leading = "if allowed(value)\n";
        let code = [
            excerpt(leading, 0, 1, "main")?,
            excerpt(
                "    || value == '+' {\n",
                u32::try_from(leading.len())?,
                2,
                "other",
            )?,
        ];
        let quote = "if allowed(value) || value == '+'";
        assert!(reopen_wrapped(&code, &code[1], quote).is_none());
        let repeated = [excerpt(
            "if allowed(value)\n    || value == '+'\nif allowed(value)\n    || value == '+'\n",
            0,
            1,
            "main",
        )?];
        assert!(reopen_wrapped(&repeated, &repeated[0], quote).is_none());
        assert!(
            reopen_wrapped(&repeated, &repeated[0], "if allowed(value) || value == '-'").is_none()
        );
        let short = [excerpt("a\r\nb\n", 0, 1, "main")?];
        assert!(reopen_wrapped(&short, &short[0], "a\n                   b").is_none());

        let line_start = u32::try_from(leading.trim_end().len())?;
        let actual_break = [
            excerpt(leading.trim_end(), 0, 1, "main")?,
            excerpt("\r\n\r\n    || value == '+'\n", line_start, 1, "main")?,
        ];
        assert!(
            reopen_wrapped(
                &actual_break,
                &actual_break[1],
                "if allowed(value)\n\n|| value == '+'"
            )
            .is_some()
        );
        let no_break = [
            excerpt(leading.trim_end(), 0, 1, "main")?,
            excerpt("|| value == '+'\n", line_start, 1, "main")?,
        ];
        assert!(reopen_wrapped(&no_break, &no_break[1], quote).is_none());
        Ok(())
    }

    #[test]
    fn exact_code_quote_keeps_one_preceding_original_line() -> Result<(), Box<dyn Error>> {
        let text = "if allowed(value)\n  || value == '-'\r\n  || value == ':'\n  || value == '+'\n";
        let mut code = excerpt(text, 0, 1, "main")?;
        let quote = "|| value == ':'";
        assert_eq!(
            with_context(&code, quote),
            "  || value == '-'\r\n  || value == ':'"
        );
        assert_eq!(
            with_context(&code, "if allowed(value)"),
            "if allowed(value)"
        );
        code.evidence.location = None;
        assert_eq!(with_context(&code, quote), quote);
        let repeated = excerpt("marker\nababababab", 0, 1, "main")?;
        assert_eq!(with_context(&repeated, "abababab"), "abababab");
        let partial = excerpt("start of the passage\nstop_here();", 37, 3, "main")?;
        assert_eq!(with_context(&partial, "stop_here();"), "stop_here();");
        Ok(())
    }
}
