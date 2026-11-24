//! Retrieval and citation verification for a selected immutable GitHub change.

use ctxfab_inference::generation::{
    ComparisonDraft, ComparisonEvidence, GenerationAttempt, Request,
};
use serde::Serialize;
use serde_json::json;

use super::evidence_review::{Candidate, EvidenceReview, Source};
use super::{
    Answer, AnswerError, AnswerMode, AnswerRequest, Answers, Citation, GenerationEvidence,
    ModelContext, observe_generation,
};
use crate::application::{
    AnswerLanguage, CodeScope, Failure, SearchHit, SearchQuery,
    github::history::Reopened,
    telemetry::{Event, Observer, RetrievalCounts, Stage, TraceLink},
};
use crate::domain::{
    corpus::snapshot::Location,
    github::{
        history::hunks::{AlignedLine, Anchor},
        source::{pinned_file_url, pinned_line_url, valid_object_id},
    },
};

pub const MAX_PAIRS: u16 = 2;

#[derive(Clone, Copy)]
pub struct Selection<'a> {
    pub scope: &'a CodeScope,
    pub child: &'a str,
}

impl<'a> Selection<'a> {
    /// # Errors
    /// Requires a validated repository and branch, and a full Git object ID.
    pub fn parse(query: &'a SearchQuery, child: &'a str) -> Result<Self, Failure> {
        Ok(Self {
            scope: query.scope().ok_or(Failure::InvalidHistoryScope)?,
            child: valid_object_id(child)
                .then_some(child)
                .ok_or(Failure::InvalidHistoryScope)?,
        })
    }
}

#[derive(Clone, Copy)]
pub struct HunkKey {
    pub change: u32,
    pub hunk: u32,
}

pub struct Pair {
    pub key: HunkKey,
    pub repository: String,
    pub parent: String,
    pub before: Anchor,
    pub after: Anchor,
}

impl Pair {
    pub(super) fn review_candidate(&self, child: &str) -> Candidate {
        let mut candidate = Candidate::new(
            Source::GitHubHistory,
            format!("{} · {}", self.repository, self.after.path),
            Some(child.to_owned()),
        );
        candidate.preview(&self.after.text);
        candidate
    }
}

pub struct LocatedPair {
    pub scope: CodeScope,
    pub child: String,
    pub pair: Pair,
}

pub trait HistoryStore: Send + Sync {
    /// Find indexed pairs across locally published lineages.
    ///
    /// # Errors
    /// Returns a storage error if the evidence cannot be read.
    fn discover(&self, query: &SearchQuery) -> Result<Vec<LocatedPair>, AnswerError>;

    /// Read ranked complete pairs within the current publication of a selected commit.
    ///
    /// # Errors
    /// Returns a storage error if the scoped snapshot cannot be read.
    fn pairs(
        &self,
        query: &SearchQuery,
        selection: Selection<'_>,
    ) -> Result<Vec<Pair>, AnswerError>;

    /// Reopen one pair from independently verified parent and child blobs.
    ///
    /// # Errors
    /// Returns a storage error if the requested evidence cannot be read coherently.
    fn reopen(
        &self,
        selection: Selection<'_>,
        pair: &Pair,
    ) -> Result<Option<Reopened>, AnswerError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Side {
    Before,
    After,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonCitation {
    side: Side,
    child: String,
    parent: String,
    change_index: u32,
    hunk_index: u32,
}

impl Answers {
    /// # Errors
    /// Returns a typed failure if the local store or inference worker cannot complete.
    pub fn ask_history(
        &self,
        query: &SearchQuery,
        selection: Selection<'_>,
        metadata: &AnswerRequest<'_>,
    ) -> Result<Answer, AnswerError> {
        let request = self.begin_answer(metadata)?;
        let result = self.history_answer(
            query,
            selection,
            &request,
            metadata.observer,
            metadata.trace,
        );
        request.finish()?;
        result
    }

    fn history_answer(
        &self,
        query: &SearchQuery,
        selection: Selection<'_>,
        request: &Request<'_>,
        observer: Option<&dyn Observer>,
        trace: Option<TraceLink>,
    ) -> Result<Answer, AnswerError> {
        let history = self.history.as_deref().ok_or(AnswerError::Storage)?;
        let lookup = observer.map(|observer| observer.start(Stage::AnswerRetrieval, trace));
        let pairs = history.pairs(query, selection);
        if let Some(mut lookup) = lookup {
            lookup.event(Event::HistoryPairsSelected);
            if let Ok(pairs) = &pairs {
                let count = u16::try_from(pairs.len()).unwrap_or(u16::MAX);
                lookup.retrieval_counts(RetrievalCounts {
                    passage_candidates: None,
                    history_pairs: Some(count),
                    workflow_attempts: None,
                    proposed_sources: count,
                });
            }
            if pairs.as_ref().is_ok_and(Vec::is_empty) {
                lookup.event(Event::NoEvidence);
            }
            lookup.finish(pairs.is_ok());
        }
        self.compare_history(query, selection, &pairs?, request, observer, trace)
    }

    pub(super) fn compare_history(
        &self,
        query: &SearchQuery,
        selection: Selection<'_>,
        pairs: &[Pair],
        request: &Request<'_>,
        observer: Option<&dyn Observer>,
        trace: Option<TraceLink>,
    ) -> Result<Answer, AnswerError> {
        let history = self.history.as_deref().ok_or(AnswerError::Storage)?;
        let mut answer = Answer::withheld(
            request,
            AnswerMode::Bm25,
            request
                .captures_context()
                .then(|| review_history(query, selection.child, pairs)),
            Vec::new(),
        );
        if pairs.is_empty() {
            return Ok(answer);
        }

        let adjacent = prepare_windows(history, selection, pairs, query, request)?;
        let language = self
            .repository
            .settings()
            .map_err(|_| AnswerError::Storage)?
            .preferred_answer_language();
        let generated = observe_generation(
            observer,
            trace,
            GenerationEvidence::HistoryPairs,
            |record| {
                let render = |count: usize, optional: bool| {
                    prompt(
                        query,
                        selection,
                        &pairs[..count],
                        optional.then_some(&adjacent[..count]),
                        language,
                    )
                };
                if query.scope().is_none() {
                    request.compare_remaining(pairs.len(), record, render)
                } else {
                    request.compare(pairs.len(), record, render)
                }
            },
        )?;
        let result = match generated {
            GenerationAttempt::Completed(result) => result,
            GenerationAttempt::WithoutDraft(stop) => return Ok(answer.stopped(stop)),
        };
        let admitted = &pairs[..result.evidence_count];
        let draft = result.draft;
        let verification = observer.map(|observer| observer.start(Stage::AnswerValidation, trace));
        let citations = verify(
            history,
            selection,
            admitted,
            result
                .optional_context
                .then_some(&adjacent[..result.evidence_count]),
            &draft,
        );
        if let Some(mut verification) = verification {
            if let Ok(citations) = &citations {
                verification.event(match citations {
                    Verified::Cited { .. } => Event::CitationsVerified,
                    Verified::MissingPair => Event::HistoryPairNotCited,
                    Verified::ScopeChanged => Event::HistoryPairChanged,
                    Verified::QuoteMismatch => Event::HistoryQuoteRejected,
                });
            }
            verification.finish(matches!(citations, Ok(Verified::Cited { .. })));
        }
        let citations = citations?;
        if let Some(review) = &mut answer.evidence_review {
            if matches!(&citations, Verified::ScopeChanged) {
                review.invalidated();
            } else {
                review.unverified(
                    admitted.len(),
                    &draft.supported.then_some(draft.pair).into_iter(),
                );
            }
        }
        if let Verified::Cited { citations, kind } = citations {
            answer.outcome = "answered";
            answer.text = Some(if kind == ComparisonEvidence::Unchanged {
                unchanged_answer(&citations[0].quote, language, query)
            } else {
                draft.answer.trim().to_owned()
            });
            answer.citations = citations;
            answer.evidence_review = None;
            answer.model_context =
                ModelContext::from_completion(result.completion_prompt.as_deref());
            answer
                .evidence_repository_ids
                .push(selection.scope.repository_id);
        }
        Ok(answer)
    }
}

fn review_history(query: &SearchQuery, child: &str, pairs: &[Pair]) -> EvidenceReview {
    let mut review = EvidenceReview::new(query);
    review.search(Source::GitHubHistory, pairs.len(), Some(child.to_owned()));
    for (index, pair) in pairs.iter().enumerate() {
        let mut candidate = pair.review_candidate(child);
        candidate.propose(index + 1);
        review.add(candidate);
    }
    review
}

fn prepare_windows(
    history: &dyn HistoryStore,
    selection: Selection<'_>,
    pairs: &[Pair],
    query: &SearchQuery,
    request: &Request<'_>,
) -> Result<Vec<Option<Window>>, AnswerError> {
    let identifiers = query.referenced_identifiers();
    if identifiers.is_empty() {
        return Ok((0..pairs.len()).map(|_| None).collect());
    }
    pairs
        .iter()
        .map(|pair| {
            request.check_cancelled()?;
            Ok(history
                .reopen(selection, pair)?
                .filter(|current| matches_pair(current, pair))
                .and_then(|current| {
                    let witness = unchanged_witness(pair, &current.adjacent, &identifiers)?;
                    Some(Window {
                        lines: current.adjacent,
                        witness,
                    })
                }))
        })
        .collect()
}

fn prompt(
    query: &SearchQuery,
    selection: Selection<'_>,
    pairs: &[Pair],
    adjacent: Option<&[Option<Window>]>,
    language: AnswerLanguage,
) -> Option<String> {
    if adjacent.is_some_and(|windows| windows.iter().all(Option::is_none)) {
        return None;
    }
    let changes: Vec<_> = pairs
        .iter()
        .enumerate()
        .map(|(index, pair)| {
            let window = adjacent.and_then(|windows| windows[index].as_ref());
            json!({
                "pair": index + 1,
                "before": prompt_side(Side::Before, &pair.parent, &pair.before, window),
                "after": prompt_side(Side::After, selection.child, &pair.after, window),
            })
        })
        .collect();
    Some(
        json!({
            "question": query.original(),
            "language": super::answer_language(language),
            "changes": changes,
        })
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e"),
    )
}

fn prompt_side(
    side: Side,
    commit: &str,
    anchor: &Anchor,
    adjacent: Option<&Window>,
) -> serde_json::Value {
    let mut lines: Vec<_> = anchor
        .text
        .split_inclusive('\n')
        .enumerate()
        .map(|(offset, line)| {
            let index = anchor.start_line + offset;
            (
                index,
                json!({"line": index, "text": line.trim_end_matches(['\r', '\n'])}),
            )
        })
        .collect();
    lines.extend(
        adjacent
            .into_iter()
            .flat_map(|window| {
                window
                    .lines
                    .iter()
                    .enumerate()
                    .map(|(group, line)| (group == window.witness, line))
            })
            .map(|(witness, aligned)| {
                let line = match side {
                    Side::Before => &aligned.before,
                    Side::After => &aligned.after,
                };
                let mut value = json!({
                    "line": line.start_line,
                    "text": line.text.trim_end_matches(['\r', '\n']),
                    "context": true,
                });
                if witness {
                    value["unchangedGroup"] = json!(1);
                }
                (line.start_line, value)
            }),
    );
    lines.sort_unstable_by_key(|(line, _)| *line);
    let lines = lines
        .into_iter()
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    json!({"role": side, "revision": commit, "path": anchor.path, "lines": lines})
}

enum Verified {
    Cited {
        citations: Vec<Citation>,
        kind: ComparisonEvidence,
    },
    MissingPair,
    ScopeChanged,
    QuoteMismatch,
}

struct Window {
    lines: Vec<AlignedLine>,
    witness: usize,
}

fn unchanged_witness(pair: &Pair, lines: &[AlignedLine], identifiers: &[&str]) -> Option<usize> {
    if identifiers.is_empty()
        || [&pair.before, &pair.after].into_iter().any(|anchor| {
            anchor.text.lines().any(|line| {
                identifiers
                    .iter()
                    .any(|name| starts_call(line, name).is_some())
            })
        })
    {
        return None;
    }
    let mut matches = lines.iter().enumerate().filter(|(_, line)| {
        line.before.text == line.after.text
            && !line.before.text.contains(['"', '\''])
            && !line.before.text.contains("/*")
            && identifiers
                .iter()
                .all(|name| starts_call(&line.before.text, name).is_some_and(ends_call))
    });
    let (index, _) = matches.next()?;
    matches.next().is_none().then_some(index)
}

fn starts_call<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let line = line.trim();
    if line.starts_with(['#', '*', '/']) {
        return None;
    }
    let line = line.split("//").next()?;
    line.match_indices(name).find_map(|(start, _)| {
        let prefix = &line[..start];
        if prefix
            .chars()
            .next_back()
            .is_some_and(|c| c == '_' || c.is_alphanumeric())
            || prefix.split_whitespace().next_back() == Some("fn")
        {
            return None;
        }
        line[start + name.len()..].trim_start().strip_prefix('(')
    })
}

fn ends_call(arguments: &str) -> bool {
    let mut depth = 1_usize;
    for token in arguments.bytes() {
        match token {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return true;
        }
    }
    false
}

fn unchanged_answer(quote: &str, configured: AnswerLanguage, query: &SearchQuery) -> String {
    let language = configured.resolve(Some(whichlang::detect_language(query.original())));
    let label = match language {
        AnswerLanguage::Fr => "La ligne citée est identique dans les deux révisions.",
        AnswerLanguage::De => "Die zitierte Zeile ist in beiden Revisionen identisch.",
        AnswerLanguage::Es => "La línea citada es idéntica en ambas revisiones.",
        AnswerLanguage::It => "La riga citata è identica in entrambe le revisioni.",
        AnswerLanguage::Pt => "A linha citada é idêntica nas duas revisões.",
        AnswerLanguage::Ja => "引用した行は両方のリビジョンで同一です。",
        AnswerLanguage::Zh => "引用的这一行在两个修订版本中完全相同。",
        AnswerLanguage::Auto | AnswerLanguage::En => {
            "The cited line is identical in both revisions."
        }
    };
    format!("{label}\n\n{}", quote.trim())
}

fn verify(
    history: &dyn HistoryStore,
    selection: Selection<'_>,
    pairs: &[Pair],
    adjacent: Option<&[Option<Window>]>,
    draft: &ComparisonDraft,
) -> Result<Verified, AnswerError> {
    if !draft.supported || draft.answer.trim().is_empty() || draft.answer.len() > 2_048 {
        return Ok(Verified::MissingPair);
    }
    let Some(index) = draft
        .pair
        .checked_sub(1)
        .filter(|index| *index < pairs.len())
    else {
        return Ok(Verified::MissingPair);
    };
    let pair = &pairs[index];
    let Some(current) = history.reopen(selection, pair)? else {
        return Ok(Verified::ScopeChanged);
    };
    let shown = adjacent.and_then(|windows| windows[index].as_ref());
    if !matches_pair(&current, pair) || shown.is_some_and(|window| window.lines != current.adjacent)
    {
        return Ok(Verified::ScopeChanged);
    }
    // Exact selectors of the admitted, reopened witness determine its evidence kind.
    let witness = shown.and_then(|window| {
        window.lines.get(window.witness).filter(|line| {
            line.before.start_line == draft.before_line && line.after.start_line == draft.after_line
        })
    });
    if let Some(reopened) = witness {
        let before = citation_at(
            selection,
            pair,
            Side::Before,
            &reopened.before,
            draft.before_line,
            current.retained,
        );
        let after = citation_at(
            selection,
            pair,
            Side::After,
            &reopened.after,
            draft.after_line,
            current.retained,
        );
        return Ok(match (before, after) {
            (Some(before), Some(after)) if before.quote == after.quote => Verified::Cited {
                citations: vec![before, after],
                kind: ComparisonEvidence::Unchanged,
            },
            _ => Verified::QuoteMismatch,
        });
    }
    if draft.evidence_kind == ComparisonEvidence::Unchanged {
        return Ok(Verified::QuoteMismatch);
    }
    let before = citation(
        selection,
        pair,
        Side::Before,
        draft.before_line,
        current.retained,
    );
    let after = citation(
        selection,
        pair,
        Side::After,
        draft.after_line,
        current.retained,
    );
    let (Some(before), Some(after)) = (before, after) else {
        return Ok(Verified::QuoteMismatch);
    };
    if before.quote.trim() == after.quote.trim() {
        return Ok(Verified::QuoteMismatch);
    }
    Ok(Verified::Cited {
        citations: vec![before, after],
        kind: ComparisonEvidence::Changed,
    })
}

fn matches_pair(current: &Reopened, pair: &Pair) -> bool {
    current.hunk.before.as_ref() == Some(&pair.before)
        && current.hunk.after.as_ref() == Some(&pair.after)
}

pub(super) fn citation(
    selection: Selection<'_>,
    pair: &Pair,
    side: Side,
    line: usize,
    retained: bool,
) -> Option<Citation> {
    let anchor = match side {
        Side::Before => &pair.before,
        Side::After => &pair.after,
    };
    citation_at(selection, pair, side, anchor, line, retained)
}

fn citation_at(
    selection: Selection<'_>,
    pair: &Pair,
    side: Side,
    anchor: &Anchor,
    line: usize,
    retained: bool,
) -> Option<Citation> {
    let revision = match side {
        Side::Before => pair.parent.as_str(),
        Side::After => selection.child,
    };
    let mut lines = anchor.text.split_inclusive('\n');
    let offset = lines
        .by_ref()
        .take(line.checked_sub(anchor.start_line)?)
        .map(str::len)
        .sum::<usize>();
    let quote = lines.next()?.trim_end_matches(['\r', '\n']);
    if quote.trim().is_empty() {
        return None;
    }

    let start = u32::try_from(anchor.start_byte + offset).ok()?;
    let end = start.checked_add(u32::try_from(quote.len()).ok()?)?;
    let line = u32::try_from(line).ok()?;
    let location = Location {
        path: anchor.path.clone(),
        blob: anchor.blob.clone(),
        file_url: pinned_file_url(&pair.repository, revision, &anchor.path),
        start_byte: start,
        end_byte: end,
        start_line: line,
        end_line: line,
    };
    let line_url = pinned_line_url(&location, revision, line, line);
    Some(Citation {
        evidence: SearchHit {
            source: format!("github:{}:{}", selection.scope.repository_id, anchor.path),
            revision: revision.into(),
            kind: "history".into(),
            tracked_ref: None,
            title: anchor.path.clone(),
            access: if retained { "retained" } else { "available" },
            location: Some(location),
        },
        quote: quote.into(),
        line_url,
        comparison: Some(ComparisonCitation {
            side,
            child: selection.child.into(),
            parent: pair.parent.clone(),
            change_index: pair.key.change,
            hunk_index: pair.key.hunk,
        }),
        workflow_run: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Anchor, AnswerError, ComparisonDraft, ComparisonEvidence, HistoryStore, HunkKey, Pair,
        Reopened, SearchQuery, Selection, Verified, verify,
    };
    use crate::domain::github::history::hunks::{AlignedLine, Hunk};

    const CHILD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const PARENT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const OLD: &str = "const LIMIT: u32 = 60;";
    const NEW: &str = "const LIMIT: u32 = 90;";

    struct VerifiedStore(Option<Hunk>, Vec<AlignedLine>);

    impl HistoryStore for VerifiedStore {
        fn discover(&self, _: &SearchQuery) -> Result<Vec<super::LocatedPair>, AnswerError> {
            Ok(Vec::new())
        }

        fn pairs(&self, _: &SearchQuery, _: Selection<'_>) -> Result<Vec<Pair>, AnswerError> {
            Ok(Vec::new())
        }

        fn reopen(
            &self,
            selection: Selection<'_>,
            pair: &Pair,
        ) -> Result<Option<Reopened>, AnswerError> {
            assert_eq!(selection.child, CHILD);
            assert_eq!(selection.scope.branch, "main");
            assert_eq!(pair.parent, PARENT);
            Ok(self.0.as_ref().map(|hunk| Reopened {
                hunk: hunk.clone(),
                retained: true,
                adjacent: self.1.clone(),
            }))
        }
    }

    fn pair() -> Pair {
        let anchor = |text: &str| Anchor {
            path: "src/limit file.rs".into(),
            blob: CHILD.into(),
            start_byte: 20,
            end_byte: 20 + text.len() + 2,
            start_line: 4,
            end_line: 4,
            text: format!(" {text}\n"),
        };
        Pair {
            key: HunkKey { change: 0, hunk: 2 },
            repository: "owner/repo".into(),
            parent: PARENT.into(),
            before: anchor(OLD),
            after: anchor(NEW),
        }
    }

    fn draft(pair: usize, before_line: usize, after_line: usize) -> ComparisonDraft {
        ComparisonDraft {
            supported: true,
            evidence_kind: ComparisonEvidence::Changed,
            pair,
            before_line,
            after_line,
            answer: "The limit increased from 60 to 90.".into(),
        }
    }

    #[test]
    fn history_requires_one_coherent_pair_and_reopens_quoted_source_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let query = SearchQuery::parse_scoped("What changed?", Some(41), Some("main".into()))?;
        let selection = Selection::parse(&query, CHILD)?;
        let pairs = [pair(), pair()];
        let store = VerifiedStore(
            Some(Hunk {
                before: Some(pairs[0].before.clone()),
                after: Some(pairs[0].after.clone()),
            }),
            Vec::new(),
        );
        let result = verify(&store, selection, &pairs, None, &draft(1, 4, 4))?;
        let Verified::Cited {
            citations,
            kind: ComparisonEvidence::Changed,
        } = result
        else {
            return Err("expected both changed citations".into());
        };
        assert_eq!(citations[0].evidence.revision, PARENT);
        assert_eq!(citations[1].evidence.revision, CHILD);
        assert_eq!(citations[0].evidence.access, "retained");
        let location = citations[0]
            .evidence
            .location
            .as_ref()
            .ok_or("exact location")?;
        assert_eq!(location.start_byte, 20);
        assert_eq!(location.start_line, 4);
        assert_eq!(citations[0].quote, format!(" {OLD}"));
        assert!(location.file_url.ends_with("/src/limit%20file.rs"));
        assert!(matches!(
            verify(&store, selection, &pairs, None, &draft(3, 4, 4))?,
            Verified::MissingPair
        ));
        assert!(matches!(
            verify(
                &store,
                selection,
                &pairs,
                None,
                &ComparisonDraft {
                    supported: false,
                    ..draft(1, 4, 4)
                }
            )?,
            Verified::MissingPair
        ));
        assert!(matches!(
            verify(&store, selection, &pairs, None, &draft(1, 4, 5))?,
            Verified::QuoteMismatch
        ));
        assert!(matches!(
            verify(
                &VerifiedStore(None, Vec::new()),
                selection,
                &pairs,
                None,
                &draft(1, 4, 4)
            )?,
            Verified::ScopeChanged
        ));
        Ok(())
    }

    #[test]
    fn captured_selector_reopens_the_admitted_witness() -> Result<(), Box<dyn std::error::Error>> {
        // Exact comparison draft captured on both ordinary and selected sender requests.
        let raw = r#"{"afterLine":3,"answer":"payload","beforeLine":3,"evidenceKind":"changed","pair":1,"supported":true}"#;
        let draft: ComparisonDraft = serde_json::from_str(raw)?;
        let query = SearchQuery::parse_scoped(
            "In src/sender.rs, what argument appears on the write_all call line next to the revised sender comment in the parent and child?",
            Some(41),
            Some("main".into()),
        )?;
        let selection = Selection::parse(&query, CHILD)?;
        let signature = "fn send(writer: &mut impl Write, payload: &[u8]) -> io::Result<()> {\n";
        let old = "    // write_all sends the packet from the sender.\n";
        let new = "    // write_all sends the entire packet from the sender before returning.\n";
        let line = "    writer.write_all(payload)?;\n";
        let anchor = |blob: &str, preceding: &[&str], text: &str| {
            let start_byte = preceding.iter().map(|line| line.len()).sum::<usize>();
            let line = preceding.len() + 1;
            Anchor {
                path: "src/sender.rs".into(),
                blob: blob.into(),
                start_byte,
                end_byte: start_byte + text.len(),
                start_line: line,
                end_line: line,
                text: text.into(),
            }
        };
        let pair = Pair {
            before: anchor(PARENT, &[signature], old),
            after: anchor(CHILD, &[signature], new),
            ..pair()
        };
        let lines = vec![AlignedLine {
            before: anchor(PARENT, &[signature, old], line),
            after: anchor(CHILD, &[signature, new], line),
        }];
        let store = VerifiedStore(
            Some(Hunk {
                before: Some(pair.before.clone()),
                after: Some(pair.after.clone()),
            }),
            lines.clone(),
        );
        let windows = [Some(super::Window {
            witness: super::unchanged_witness(&pair, &lines, &["write_all"])
                .ok_or("eligible sender witness")?,
            lines,
        })];
        let pairs = [pair];
        for rejected in [
            ComparisonDraft {
                before_line: 2,
                ..serde_json::from_str(raw)?
            },
            ComparisonDraft {
                after_line: 4,
                ..serde_json::from_str(raw)?
            },
            ComparisonDraft {
                evidence_kind: ComparisonEvidence::Unchanged,
                before_line: 2,
                after_line: 2,
                ..serde_json::from_str(raw)?
            },
        ] {
            assert!(matches!(
                verify(&store, selection, &pairs, Some(&windows), &rejected)?,
                Verified::QuoteMismatch
            ));
        }
        assert!(matches!(
            verify(&store, selection, &pairs, None, &draft)?,
            Verified::QuoteMismatch
        ));
        let result = verify(&store, selection, &pairs, Some(&windows), &draft)?;
        let Verified::Cited {
            citations,
            kind: ComparisonEvidence::Unchanged,
        } = result
        else {
            return Err("captured exact selectors were rejected".into());
        };
        let quotes = citations
            .iter()
            .map(|citation| citation.quote.as_str())
            .collect::<Vec<_>>();
        assert_eq!(quotes, [line.trim_end(), line.trim_end()]);
        assert_eq!(citations[0].evidence.revision, PARENT);
        assert_eq!(citations[1].evidence.revision, CHILD);
        Ok(())
    }

    #[test]
    fn an_equal_line_cannot_certify_another_changed_call() {
        let mut pair = pair();
        let mut aligned = AlignedLine {
            before: pair.before.clone(),
            after: pair.after.clone(),
        };
        for text in [
            "    }",
            "    // write_all(payload);",
            "    log(\"write_all(payload)\");",
            "    write_all(",
            "    store(payload);",
        ] {
            aligned.before.text = text.into();
            aligned.after.text = text.into();
            assert_eq!(
                super::unchanged_witness(&pair, std::slice::from_ref(&aligned), &["write_all"]),
                None
            );
        }
        aligned.before.text = "    write_all(payload);".into();
        aligned.after.text = aligned.before.text.clone();
        assert_eq!(
            super::unchanged_witness(&pair, std::slice::from_ref(&aligned), &["write_all"]),
            Some(0)
        );
        pair.before.text = "    write_all(\"old\");".into();
        pair.after.text = "    write_all(\"new\");".into();
        assert_eq!(
            super::unchanged_witness(&pair, std::slice::from_ref(&aligned), &["write_all"]),
            None
        );
    }
}
