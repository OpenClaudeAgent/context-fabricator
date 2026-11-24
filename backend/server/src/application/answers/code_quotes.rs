//! Reopen code copied with line breaks collapsed using the original source bytes.

use std::{ops::Range, path::Path};

use super::{Excerpt, MIN_QUOTE_BYTES};
use crate::domain::github::source::pinned_line_url;

const CONTEXTUAL_QUOTE_BYTES: usize = 600;

pub(super) struct VerifiedQuote<'a> {
    pub(super) excerpt: &'a Excerpt,
    pub(super) text: &'a str,
    offset: Option<usize>,
}

impl<'a> VerifiedQuote<'a> {
    pub(super) fn literal(excerpt: &'a Excerpt, text: &'a str) -> Option<Self> {
        excerpt.text.contains(text).then(|| Self {
            excerpt,
            text,
            offset: unique_match(&excerpt.text, text),
        })
    }

    /// Correct a passage ID only when it points to the same pinned file and
    /// an admitted excerpt contains exactly one possible source location.
    pub(super) fn recover_passage(
        sources: &'a [Excerpt],
        cited: &Excerpt,
        text: &'a str,
    ) -> Option<Self> {
        let cited_location = cited.evidence.location.as_ref()?;
        let mut recovered: Option<Self> = None;
        for excerpt in sources {
            let Some(location) = &excerpt.evidence.location else {
                continue;
            };
            if excerpt.repository_id != cited.repository_id
                || excerpt.evidence.source != cited.evidence.source
                || excerpt.evidence.revision != cited.evidence.revision
                || excerpt.evidence.tracked_ref != cited.evidence.tracked_ref
                || excerpt.evidence.kind != cited.evidence.kind
                || location.blob != cited_location.blob
                || location.path != cited_location.path
            {
                continue;
            }
            let offset = match unique_match(&excerpt.text, text) {
                Some(offset) => offset,
                None if !excerpt.text.contains(text) => continue,
                None => return None,
            };
            if let Some(previous) = &recovered {
                let previous_location = previous.excerpt.evidence.location.as_ref()?;
                if usize::try_from(previous_location.start_byte).ok()? + previous.offset?
                    != usize::try_from(location.start_byte).ok()? + offset
                {
                    return None;
                }
                continue;
            }
            recovered = Some(Self {
                excerpt,
                text,
                offset: Some(offset),
            });
        }
        recovered
    }

    pub(super) fn byte_range(&self) -> Option<Range<usize>> {
        let start = self.offset?;
        Some(start..start + self.text.len())
    }

    pub(super) fn exact_span(excerpt: &'a Excerpt, range: Range<usize>) -> Option<Self> {
        let line = excerpt.text.get(range.clone())?;
        let text = line.trim();
        if text.len() < MIN_QUOTE_BYTES {
            return None;
        }
        Some(Self {
            excerpt,
            text,
            offset: Some(range.start + line.len() - line.trim_start().len()),
        })
    }

    /// Include one preceding line where its boundary is known. A repeated
    /// fragment recovered from adjacent passages keeps its original slice.
    pub(super) fn with_context(self) -> Self {
        let source = self.excerpt;
        let Some(location) = &source.evidence.location else {
            return self;
        };
        let Some(start) = unique_match(&source.text, self.text) else {
            return self;
        };
        let Some(line_break) = source.text[..start].rfind('\n') else {
            return self;
        };
        let previous_line = match source.text[..line_break].rfind('\n') {
            Some(line) => line + 1,
            None if location.start_byte == 0 => 0,
            None => return self,
        };
        let end = start + self.text.len();
        if end - previous_line > CONTEXTUAL_QUOTE_BYTES {
            return self;
        }
        Self {
            text: &source.text[previous_line..end],
            offset: Some(previous_line),
            ..self
        }
    }

    pub(super) fn line_url(&self, reopened: &Excerpt) -> Option<String> {
        let location = reopened.evidence.location.as_ref()?;
        // The answer verifier already rechecked the original excerpt bytes.
        let preceding = reopened.text.get(..self.offset?)?.matches('\n').count();
        let first = location
            .start_line
            .checked_add(u32::try_from(preceding).ok()?)?;
        let quoted_lines = self.text.strip_suffix('\n').unwrap_or(self.text);
        let last = first.checked_add(u32::try_from(quoted_lines.matches('\n').count()).ok()?)?;
        pinned_line_url(location, &reopened.evidence.revision, first, last)
    }
}

struct Line<'a> {
    excerpt: &'a Excerpt,
    flattened: Range<usize>,
    source: usize,
}

/// Reopen a complete Rust documentation paragraph when generation drops only
/// the line-comment prefixes and joins the lines. Both boundaries must be known
/// in the cited passage and a repeated paragraph is ambiguous.
pub(super) fn reopen_rust_doc_paragraph<'a>(
    cited: &'a Excerpt,
    quote: &str,
) -> Option<VerifiedQuote<'a>> {
    let location = cited.evidence.location.as_ref()?;
    if location.start_line == 0 || Path::new(&location.path).extension()? != "rs" {
        return None;
    }
    // A mid-file excerpt may begin inside a comment line. Its first
    // completely visible line can bound a paragraph starting on the third.
    let first_known_start = if location.start_byte == 0 {
        0
    } else {
        cited.text.split_inclusive('\n').take(2).map(str::len).sum()
    };
    let mut offset = 0;
    let mut lines = cited
        .text
        .split_inclusive('\n')
        .map(|raw| {
            let start = offset;
            offset += raw.len();
            (start, raw)
        })
        .peekable();
    let mut found = None;
    while let Some((start, raw)) = lines.next() {
        let Some((prefix, body)) = rust_doc_line(raw) else {
            continue;
        };
        let mut folded = String::from(body);
        let mut end = start + raw.len();
        let mut multiline = false;
        while let Some((_, next)) = lines.peek() {
            let Some((next_prefix, body)) = rust_doc_line(next) else {
                break;
            };
            if next_prefix != prefix {
                break;
            }
            multiline = true;
            folded.push(' ');
            folded.push_str(body);
            end += next.len();
            lines.next();
        }
        if multiline && folded == quote {
            if found.is_some()
                || start < first_known_start
                || !lines.peek().is_some_and(|(_, line)| line.ends_with('\n'))
            {
                return None;
            }
            found = VerifiedQuote::exact_span(cited, start..end);
        }
    }
    found
}

fn rust_doc_line(raw: &str) -> Option<(&str, &str)> {
    let line = raw.trim_ascii();
    let (prefix, text) = line.split_at_checked(3)?;
    if !matches!(prefix, "///" | "//!") || (prefix == "///" && text.starts_with('/')) {
        return None;
    }
    let body = text.trim_ascii_start();
    (!body.is_empty()).then_some((prefix, body))
}

/// Recover a unique literal sequence split across original source lines. Only
/// the line breaks and surrounding indentation may differ from the model's quote.
pub(super) fn reopen_wrapped<'a>(
    sources: &'a [Excerpt],
    cited: &'a Excerpt,
    quote: &str,
) -> Option<Vec<VerifiedQuote<'a>>> {
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
        .map(|(excerpt, range)| {
            let offset = range.start;
            Some(VerifiedQuote {
                excerpt,
                text: excerpt.text.get(range)?,
                offset: Some(offset),
            })
        })
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
    use super::{Excerpt, VerifiedQuote, reopen_wrapped};
    use crate::application::SearchHit;
    use crate::domain::corpus::snapshot::Location;
    use crate::domain::github::source::pinned_file_url;

    fn excerpt(
        text: &str,
        start_byte: u32,
        line: u32,
        tracked_ref: &str,
    ) -> Result<Excerpt, Box<dyn Error>> {
        Ok(Excerpt {
            text: text.into(),
            repository_id: None,
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
    fn recovers_only_an_unambiguous_quote_in_the_same_pinned_file() -> Result<(), Box<dyn Error>> {
        let cited = excerpt("const OTHER: usize = 1;\n", 0, 1, "main")?;
        let text = "const CAPACITY: usize = 16;";
        let revision = "a123456789012345678901234567890123456789";
        let pin = |mut excerpt: Excerpt| {
            excerpt.evidence.revision = revision.into();
            if let Some(location) = &mut excerpt.evidence.location {
                location.file_url = pinned_file_url("fixture/sample", revision, &location.path);
            }
            excerpt
        };
        let matching = pin(excerpt(text, 100, 10, "main")?);
        let draft = Draft {
            answer: "The capacity is 16.".into(),
            citations: vec![Reference {
                passage: 1,
                quote: text.into(),
            }],
        };
        let admitted = [pin(cited), matching];
        let quotes = verified_quotes(&admitted, &draft).ok_or("unique quote not recovered")?;
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].text, text);
        assert!(
            quotes[0]
                .line_url(&admitted[1])
                .is_some_and(|url| url.ends_with("#L10"))
        );
        assert!(verified_quotes(&admitted[..1], &draft).is_none());

        let [cited, matching] = admitted;
        let duplicate = pin(excerpt(text, 200, 20, "main")?);
        let ambiguous = [cited, matching, duplicate];
        assert!(verified_quotes(&ambiguous, &draft).is_none());

        let [cited, mut different_revision, _] = ambiguous;
        different_revision.evidence.revision = "other-commit".into();
        assert!(verified_quotes(&[cited, different_revision], &draft).is_none());
        Ok(())
    }

    #[test]
    fn refuses_wrong_slot_recovery_when_multiline_utf8_matches_overlap()
    -> Result<(), Box<dyn Error>> {
        let cited = || excerpt("const OTHER: usize = 1;\n", 0, 1, "main");
        let line = "// café λ";
        let text = format!("{line}\n{line}");
        let ambiguous = || excerpt(&format!("{text}\n{line}"), 100, 10, "main");
        let unique = || excerpt(&text, 100, 10, "main");
        let overlapping_duplicate = excerpt(&format!("x\n{text}"), 98, 9, "main")?;
        let draft = Draft {
            answer: "The matching lines.".into(),
            citations: vec![Reference {
                passage: 1,
                quote: text.clone(),
            }],
        };

        let unambiguous = [cited()?, unique()?, overlapping_duplicate];
        let recovered = verified_quotes(&unambiguous, &draft).ok_or("unique source offset")?;
        assert_eq!(recovered[0].text, text);
        assert!(verified_quotes(&[cited()?, ambiguous()?, unique()?], &draft).is_none());
        assert!(verified_quotes(&[cited()?, ambiguous()?], &draft).is_none());
        Ok(())
    }

    #[test]
    fn reparses_java_javadoc_and_reopens_only_its_exact_same_slot_paragraph()
    -> Result<(), Box<dyn Error>> {
        use super::super::{java_doc_quotes, verified_quotes_with_breaks};
        let prose = " *<p>\r\n * The résumé contains {@code null} labels and\r\n * preserves {@link Existing} values.\r\n *<p>";
        let folded =
            "The résumé contains {@code null} labels and preserves {@link Existing} values.";
        let comment = format!("/**\r\n{prose}\r\n */");
        let document = format!("public class Demo {{\r\n{comment}\r\n void use() {{ }} \r\n}}");
        let start = u32::try_from(document.find(&comment).ok_or("known comment")?)?;
        let java = |text: &str, offset: u32, line: u32| -> Result<Excerpt, Box<dyn Error>> {
            let mut value = excerpt(text, offset, line, "main")?;
            value.evidence.revision = "a123456789012345678901234567890123456789".into();
            if let Some(location) = &mut value.evidence.location {
                location.path = "src/Demo.java".into();
                location.file_url =
                    pinned_file_url("fixture/sample", &value.evidence.revision, &location.path);
            }
            Ok(value)
        };
        let selected = java(&comment, start, 2)?;
        let span =
            java_doc_quotes::paragraph_span(&selected, folded).ok_or("complete paragraph")?;
        assert!(java_doc_quotes::is_javadoc_span(
            &selected, &span, &document
        ));
        let rendered = Draft {
            answer: "The behavior is documented.".into(),
            citations: vec![Reference {
                passage: 1,
                quote: folded.into(),
            }],
        };
        let shown = std::slice::from_ref(&selected);
        assert!(verified_quotes(shown, &rendered).is_none());
        let quotes = verified_quotes_with_breaks(shown, &rendered, &[Some(span.clone())])
            .ok_or("verified quote")?;
        assert!(quotes[0].text.contains("and\r\n * preserves"));
        assert!(
            quotes[0]
                .line_url(&selected)
                .is_some_and(|url| url.ends_with("#L4-L5"))
        );
        assert!(
            java_doc_quotes::paragraph_span(&selected, &folded.replace("null", "empty")).is_none()
        );
        assert!(
            java_doc_quotes::paragraph_span(
                &selected,
                &folded.replace("{@link Existing}", "Existing")
            )
            .is_none()
        );
        assert!(!java_doc_quotes::is_javadoc_span(
            &selected,
            &span,
            &document.replace("résumé", "header")
        ));
        let other = java("public class Elsewhere { }", 0, 1)?;
        assert!(java_doc_quotes::paragraph_span(&other, folded).is_none());
        assert!(verified_quotes(&[other, selected], &rendered).is_none());

        let duplicate = format!(
            "/**\r\n{prose}\r\n * The résumé contains {{@code null}} labels and\r\n * preserves {{@link Existing}} values.\r\n */"
        );
        assert!(java_doc_quotes::paragraph_span(&java(&duplicate, 0, 1)?, folded).is_none());
        for wrong in [
            format!("class Demo {{ String x = \"\"\"\n{comment}\n\"\"\"; }}"),
            format!("class Demo {{ /********** \n <pre>\n{prose}\n </pre>\n */ }}"),
            format!("class Demo {{ /*\n{prose}\n */ }}"),
        ] {
            let source = java(&wrong, 0, 1)?;
            if let Some(range) = java_doc_quotes::paragraph_span(&source, folded) {
                assert!(!java_doc_quotes::is_javadoc_span(&source, &range, &wrong));
            }
        }
        Ok(())
    }

    #[test]
    fn rendered_markdown_quotes_reopen_the_original_line() -> Result<(), Box<dyn Error>> {
        let original = "- **Local storage** — Export the database as `.sqlite`.";
        let rendered = "- Local storage — Export the database as .sqlite.";
        let revision = "a".repeat(40);
        let url = pinned_file_url("owner/board", &revision, "README.md");
        let mut source = excerpt(original, 100, 24, "main")?;
        source.evidence.revision = revision;
        let location = source.evidence.location.as_mut().ok_or("fixture path")?;
        location.path = "README.md".into();
        location.file_url.clone_from(&url);
        let draft = |quote: &str| Draft {
            answer: "Export the database as .sqlite.".into(),
            citations: vec![Reference {
                passage: 1,
                quote: quote.into(),
            }],
        };
        let rendered_draft = draft(rendered);
        let quotes = verified_quotes(std::slice::from_ref(&source), &rendered_draft)
            .ok_or("rendered Markdown should identify its exact source line")?;
        assert_eq!(quotes[0].text, original);
        assert_eq!(quotes[0].line_url(&source), Some(format!("{url}#L24")));
        assert!(
            verified_quotes(
                std::slice::from_ref(&source),
                &draft("- Local storage — Export the database as .csv.")
            )
            .is_none()
        );
        source.text = format!("{original}\n{original}");
        assert!(verified_quotes(std::slice::from_ref(&source), &draft(rendered)).is_none());
        Ok(())
    }

    #[test]
    fn permalink_opens_the_quoted_lines_at_the_pinned_commit() -> Result<(), Box<dyn Error>> {
        let revision = "a".repeat(40);
        let text = "First line\r\nlet valeur = \"été\";\r\ncall();\r\nLast line\r\n";
        let mut source = excerpt(text, 800, 40, "main")?;
        source.evidence.revision.clone_from(&revision);
        let path = "src/été #.rs";
        let url = pinned_file_url("owner/blob", &revision, path);
        let location = source.evidence.location.as_mut().ok_or("source location")?;
        location.path = path.into();
        location.file_url.clone_from(&url);

        let link = |source: &Excerpt, text: &str| {
            VerifiedQuote::literal(source, text).and_then(|quote| quote.line_url(source))
        };
        let single = link(&source, "let valeur = \"été\";");
        assert_eq!(
            single,
            Some(format!(
                "https://github.com/owner/blob/blob/{revision}/src/%C3%A9t%C3%A9%20%23.rs#L41"
            ))
        );
        assert_eq!(
            link(&source, "let valeur = \"été\";\r\ncall();\r\n"),
            Some(format!("{url}#L41-L42"))
        );
        assert_eq!(link(&source, "unverified quote"), None);
        source.evidence.revision = "b".repeat(40);
        assert_eq!(link(&source, "call();"), None);

        let location = source.evidence.location.as_mut().ok_or("source location")?;
        location.file_url = url.replace("https://github.com/", "https://github.com.example/");
        source.evidence.revision = revision;
        assert_eq!(link(&source, "call();"), None);
        Ok(())
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
        assert!(cited[0].excerpt.evidence.same_evidence(&code[1].evidence));
        assert_eq!(cited[0].text, leading.trim_end());
        assert!(cited[1].excerpt.evidence.same_evidence(&code[0].evidence));
        assert_eq!(cited[1].text, "|| value == '+'");
        Ok(())
    }

    #[test]
    fn folded_rust_doc_paragraph_reopens_at_the_pinned_lines() -> Result<(), Box<dyn Error>> {
        let before = "/// A wrapper that represents a canonical version of an original URL.\n///\n";
        let original = "/// This is **only** for internal purposes. All fetching should happen within the context of the\r\n/// original URL, since canonicalization can remove credentials and change paths.";
        let text = format!("{before}{original}\r\npub struct CanonicalUrl(SafeUrl);\r\n");
        let revision = "a".repeat(40);
        let url = pinned_file_url("owner/blob", &revision, "src/lib.rs");
        let mut source = excerpt(&text, 800, 40, "main")?;
        source.evidence.revision = revision;
        source
            .evidence
            .location
            .as_mut()
            .ok_or("source location")?
            .file_url = url.clone();
        let draft = |quote: &str| {
            Draft {
            answer: "Fetch the original URL because canonicalization may remove credentials or change paths.".into(),
            citations: vec![Reference { passage: 1, quote: quote.into() }],
        }
        };
        let proposal = "This is **only** for internal purposes. All fetching should happen within the context of the original URL, since canonicalization can remove credentials and change paths.";
        let recorded = draft(proposal);
        let quotes = verified_quotes(std::slice::from_ref(&source), &recorded)
            .ok_or("folded documentation quote did not reopen")?;
        assert_eq!(quotes[0].text, original);
        assert_eq!(quotes[0].line_url(&source), Some(format!("{url}#L42-L43")));

        for rejected in [
            draft(&proposal.replace("can remove", "cannot remove")),
            draft("All fetching should happen within the context of the original URL"),
        ] {
            assert!(verified_quotes(std::slice::from_ref(&source), &rejected).is_none());
        }

        source.text =
            format!("{before}{original}\n///\n{original}\npub struct CanonicalUrl(SafeUrl);\n");
        assert!(verified_quotes(std::slice::from_ref(&source), &recorded).is_none());
        Ok(())
    }

    #[test]
    fn folded_rust_doc_paragraph_rejects_clipped_delimiters() -> Result<(), Box<dyn Error>> {
        let first = "Fetching should use the original URL.";
        let second = "Canonicalization may remove credentials.";
        let draft = Draft {
            answer: "Fetch the original URL.".into(),
            citations: vec![Reference {
                passage: 1,
                quote: format!("{first} {second}"),
            }],
        };
        let leading = format!(
            "/// {} Except for the following case:\n/// {first}\n/// {second}\n///\n",
            "x".repeat(512)
        );
        let start = leading.find("Except").ok_or("slice start")?;
        let clipped = excerpt(&leading[start..], u32::try_from(start)?, 1, "main")?;
        assert!(verified_quotes(&[clipped], &draft).is_none());

        let trailing =
            format!("///\n/// {first}\n/// {second}\n/// This applies only when authenticated.\n");
        let last = trailing.find("/// This").ok_or("slice end")? + 1;
        let clipped = excerpt(&trailing[..last], 0, 1, "main")?;
        assert!(verified_quotes(&[clipped], &draft).is_none());
        Ok(())
    }

    #[test]
    fn wrapped_permalink_targets_the_reopened_occurrence_when_a_line_repeats()
    -> Result<(), Box<dyn Error>> {
        let leading = "next_call();\r\nother();\r\nnext_call();\r\n";
        let revision = "a".repeat(40);
        let file = pinned_file_url("owner/project", &revision, "src/lib.rs");
        let mut code = [
            excerpt(leading, 0, 1, "main")?,
            excerpt("finish();\n", u32::try_from(leading.len())?, 4, "main")?,
        ];
        for source in &mut code {
            source.evidence.revision.clone_from(&revision);
            let location = source.evidence.location.as_mut().ok_or("source location")?;
            location.file_url.clone_from(&file);
        }
        let draft = Draft {
            answer: "The next call precedes finish.".into(),
            citations: vec![Reference {
                passage: 1,
                quote: "next_call(); finish();".into(),
            }],
        };
        let citations = verified_quotes(&code, &draft).ok_or("reopened quotes")?;
        let [first, second] = citations.as_slice() else {
            return Err("expected one citation for each passage".into());
        };
        assert_eq!(first.text, "next_call();");
        assert_eq!(first.line_url(&code[0]), Some(format!("{file}#L3")));
        assert_eq!(second.text, "finish();");
        assert_eq!(second.line_url(&code[1]), Some(format!("{file}#L4")));
        let ambiguous = VerifiedQuote::literal(&code[0], first.text).ok_or("literal quote")?;
        assert_eq!(ambiguous.line_url(&code[0]), None);
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
        fn context<'a>(source: &'a Excerpt, quote: &'a str) -> Option<&'a str> {
            VerifiedQuote::literal(source, quote).map(|verified| verified.with_context().text)
        }

        let text = "if allowed(value)\n  || value == '-'\r\n  || value == ':'\n  || value == '+'\n";
        let mut code = excerpt(text, 0, 1, "main")?;
        let quote = "|| value == ':'";
        assert_eq!(
            context(&code, quote),
            Some("  || value == '-'\r\n  || value == ':'")
        );
        assert_eq!(
            context(&code, "if allowed(value)"),
            Some("if allowed(value)")
        );
        code.evidence.location = None;
        assert_eq!(context(&code, quote), Some(quote));
        let repeated = excerpt("marker\nababababab", 0, 1, "main")?;
        assert_eq!(context(&repeated, "abababab"), Some("abababab"));
        let partial = excerpt("start of the passage\nstop_here();", 37, 3, "main")?;
        assert_eq!(context(&partial, "stop_here();"), Some("stop_here();"));
        Ok(())
    }
}
