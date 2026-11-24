//! Independent local rankings interleaved under one generation budget.
use std::collections::BTreeSet;

use ctxfab_inference::generation::{
    GenerationResult, MAX_PASSAGE_CITATIONS, PassagePacking, Reference,
};
use serde_json::Value;

use crate::application::telemetry::{Event, RetrievalCounts};

use super::actions_scope::AttemptRequest;
use super::evidence_review::{self, Candidate, EvidenceReview, RankedCandidate};
use super::{
    AnswerError, Answers, Citation, CodeScope, Draft, EvidenceStore, Excerpt, MAX_SOURCES,
    SearchQuery, actions, history, json,
};
use actions::LocatedRun;
use history::{LocatedPair, Side};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Source {
    Code(usize),
    Change(usize),
    Run(usize),
}

#[derive(PartialEq, Eq)]
enum ContextScope {
    Ordinary,
    RootReadme,
}

pub(super) struct Sources {
    code: Vec<Excerpt>,
    changes: Vec<LocatedPair>,
    runs: Vec<LocatedRun>,
    searched_history: bool,
    searched_actions: bool,
    requires_attempt: bool,
    context_scope: ContextScope,
    pub(super) order: Vec<Source>,
}

impl Sources {
    pub(super) fn discover(
        answers: &Answers,
        query: &SearchQuery,
        code: Vec<Excerpt>,
    ) -> Result<Self, AnswerError> {
        let (history, actions) = (answers.history.as_deref(), answers.actions.as_deref());
        let attempt = AttemptRequest::parse(query);
        let target = attempt.as_ref().and_then(|request| request.target.as_ref());
        let changes = match history {
            Some(history) => history.discover(query)?,
            None => Vec::new(),
        };
        let runs = match actions {
            Some(actions) if attempt.is_none() || target.is_some() => {
                actions.discover(query, target)?
            }
            _ => Vec::new(),
        };
        let mut order = Vec::new();
        for i in 0..MAX_SOURCES {
            if i < code.len() {
                order.push(Source::Code(i));
            }
            if i < changes.len() {
                order.push(Source::Change(i));
            }
            if i < runs.len() {
                order.push(Source::Run(i));
            }
        }
        order.truncate(MAX_SOURCES);
        Ok(Self {
            code,
            changes,
            runs,
            searched_history: history.is_some(),
            searched_actions: actions.is_some(),
            context_scope: ContextScope::Ordinary,
            requires_attempt: attempt.is_some_and(|attempt| attempt.required),
            order,
        })
    }

    pub(super) fn from_authoritative_readme(
        evidence: &dyn EvidenceStore,
        query: &SearchQuery,
    ) -> Result<Option<Self>, AnswerError> {
        if AttemptRequest::parse(query).is_some_and(|request| request.required) {
            return Ok(None);
        }
        let Some(code) = evidence.authoritative_readme(query)? else {
            return Ok(None);
        };
        let order = (0..code.len()).map(Source::Code).collect();
        Ok(Some(Self {
            code,
            changes: Vec::new(),
            runs: Vec::new(),
            searched_history: false,
            searched_actions: false,
            requires_attempt: false,
            context_scope: ContextScope::RootReadme,
            order,
        }))
    }

    pub(super) fn prefer_java_method(
        &mut self,
        evidence: &dyn EvidenceStore,
        query: &SearchQuery,
    ) -> Result<Option<Event>, AnswerError> {
        if self.requires_attempt || !self.changes.is_empty() || !self.runs.is_empty() {
            return Ok(None);
        }
        let super::JavaEvidence {
            passages: mut promoted,
            class_local,
        } = evidence.java_evidence(query)?;
        if promoted.is_empty() {
            return Ok(None);
        }
        self.code.retain(|excerpt| {
            !promoted
                .iter()
                .any(|head| head.evidence.same_evidence(&excerpt.evidence))
        });
        promoted.append(&mut self.code);
        promoted.truncate(MAX_SOURCES);
        self.order = (0..promoted.len()).map(Source::Code).collect();
        self.code = promoted;
        Ok(Some(if class_local {
            Event::NamedJavaClassPassagesSelected
        } else {
            Event::ExactJavaMethodPassagesSelected
        }))
    }

    pub(super) fn missing_required_attempt(&self) -> bool {
        self.requires_attempt && self.runs.is_empty()
    }

    pub(super) fn review(
        &self,
        query: &SearchQuery,
        mut ranked: Vec<RankedCandidate>,
        inspect: bool,
    ) -> EvidenceReview {
        use evidence_review::Source as Searched;
        let mut review = EvidenceReview::new(query);
        review.search(
            Searched::LocalPassages,
            if self.context_scope == ContextScope::RootReadme {
                self.code.len()
            } else {
                ranked.len()
            },
            (self.context_scope == ContextScope::RootReadme)
                .then(|| "published root README".to_owned()),
        );
        if self.searched_history {
            review.search(Searched::GitHubHistory, self.changes.len(), None);
        }
        if self.searched_actions {
            review.search(Searched::GitHubActions, self.runs.len(), None);
        }
        for (index, excerpt) in self.code.iter().enumerate() {
            if let Some(existing) = ranked
                .iter_mut()
                .find(|candidate| candidate.hit.same_evidence(&excerpt.evidence))
            {
                existing.review.preview(&excerpt.text);
                if let Some(slot) = self.slot(Source::Code(index)) {
                    existing.review.propose(slot);
                }
            } else {
                let mut candidate = Candidate::passage(&excerpt.evidence, None);
                candidate.preview(&excerpt.text);
                if self.context_scope != ContextScope::RootReadme {
                    candidate.mark_context_added();
                }
                if inspect {
                    candidate.capture_source(&excerpt.evidence, excerpt.text.clone());
                }
                if let Some(slot) = self.slot(Source::Code(index)) {
                    candidate.propose(slot);
                }
                review.add(candidate);
            }
        }
        review.extend(ranked.into_iter().map(|candidate| candidate.review));
        for (index, change) in self.changes.iter().enumerate() {
            let mut candidate = change.pair.review_candidate(&change.child);
            if let Some(slot) = self.slot(Source::Change(index)) {
                candidate.propose(slot);
            }
            review.add(candidate);
        }
        for (index, run) in self.runs.iter().enumerate() {
            let mut candidate = run.observation.review_candidate();
            if let Some(slot) = self.slot(Source::Run(index)) {
                candidate.propose(slot);
            }
            review.add(candidate);
        }
        review
    }

    pub(super) fn counts(&self, passage_candidates: usize) -> RetrievalCounts {
        let count = |value| u16::try_from(value).unwrap_or(u16::MAX);
        RetrievalCounts {
            passage_candidates: Some(count(passage_candidates)),
            history_pairs: self.searched_history.then(|| count(self.changes.len())),
            workflow_attempts: self.searched_actions.then(|| count(self.runs.len())),
            proposed_sources: count(self.order.len()),
        }
    }

    fn slot(&self, source: Source) -> Option<usize> {
        self.order
            .iter()
            .position(|entry| *entry == source)
            .map(|index| index + 1)
    }

    pub(super) fn current_code_head(&self) -> Option<&Excerpt> {
        if self.requires_attempt {
            return None;
        }
        let head = self.code.first()?;
        let owner = head.repository_id?;
        self.order
            .iter()
            .all(|entry| {
                let Source::Code(index) = entry else {
                    return false;
                };
                let excerpt = &self.code[*index];
                excerpt.repository_id == Some(owner)
                    && excerpt.evidence.kind == "current"
                    && excerpt.evidence.tracked_ref == head.evidence.tracked_ref
                    && excerpt.evidence.revision == head.evidence.revision
            })
            .then_some(head)
    }

    /// A constrained comparison can recover an abstention or an imprecise historical
    /// quote. Keep rejected references to other evidence families and dropped IDs rejected.
    pub(super) fn comparison_candidate(&self, count: usize, draft: &Draft) -> Option<&LocatedPair> {
        if self.requires_attempt {
            return None;
        }
        let admitted = &self.order[..count];
        if !draft.citations.iter().all(|r| {
            r.passage
                .checked_sub(1)
                .and_then(|i| admitted.get(i))
                .is_some_and(|s| matches!(s, Source::Change(_)))
        }) {
            return None;
        }
        let index = match draft.citations.first() {
            Some(reference) => reference.passage.checked_sub(1)?,
            None => admitted
                .iter()
                .position(|s| matches!(s, Source::Change(_)))?,
        };
        match admitted[index] {
            Source::Change(i) => Some(&self.changes[i]),
            _ => None,
        }
    }

    pub(super) fn repository_ids(&self, limit: usize) -> Vec<i64> {
        let ids: BTreeSet<_> = self.order[..limit]
            .iter()
            .filter_map(|source| self.repository_id(*source))
            .collect();
        ids.into_iter().collect()
    }

    fn repository_id(&self, source: Source) -> Option<i64> {
        match source {
            Source::Code(i) => self.code[i].repository_id,
            Source::Change(i) => Some(self.changes[i].scope.repository_id),
            Source::Run(i) => Some(self.runs[i].scope.repository_id),
        }
    }

    fn passage(&self, source: Source) -> Value {
        match source {
            Source::Code(i) => code_passage(&self.code[i]),
            Source::Change(i) => {
                let located = &self.changes[i];
                let pair = &located.pair;
                json!({"kind":"history","repository":pair.repository,"branch":located.scope.branch,
                    "beforePath":pair.before.path,"afterPath":pair.after.path,"parent":pair.parent,"child":located.child,
                    "text":format!("Before:\n{}\nAfter:\n{}",pair.before.text,pair.after.text)})
            }
            Source::Run(i) => {
                let located = &self.runs[i];
                json!({"kind":"workflow","repository":located.observation.repository,"branch":located.scope.branch,
                    "text":located.observation.fields()})
            }
        }
    }

    pub(super) fn passage_packing(&self) -> PassagePacking {
        let count = self.order.len();
        if self.context_scope == ContextScope::Ordinary && self.current_code_head().is_some() {
            PassagePacking::RankPrefixFirst(count)
        } else {
            PassagePacking::Compact(count)
        }
    }

    // Only current code from one immutable scope can be repacked. Mixed families
    // retain their fair interleave and comparison-specific admission rules.
    fn packing_order(&self, count: usize) -> Vec<usize> {
        let mut indices: Vec<_> = (0..self.order.len()).collect();
        if self.context_scope == ContextScope::RootReadme {
            indices[..count]
                .sort_unstable_by_key(|&index| self.code[index].evidence.passage_start());
            return indices;
        }
        if count == indices.len() || self.current_code_head().is_none() {
            return indices;
        }
        let sizes: Vec<_> = self.code.iter().map(|excerpt| excerpt.text.len()).collect();
        pack_indices(&sizes, count)
    }

    pub(super) fn apply_packing(
        &mut self,
        result: &GenerationResult<Draft>,
        review: Option<&mut EvidenceReview>,
    ) -> usize {
        let count = result.evidence_count;
        if result.optional_context {
            count
        } else {
            self.apply_compaction(count, review)
        }
    }

    fn apply_compaction(&mut self, count: usize, review: Option<&mut EvidenceReview>) -> usize {
        let indices = self.packing_order(count);
        if indices.iter().copied().eq(0..indices.len()) {
            return count;
        }
        if let Some(review) = review {
            review.reorder_slots(&indices);
        }
        let mut original: Vec<_> = self.code.drain(..).map(Some).collect();
        self.code = indices
            .iter()
            .filter_map(|&index| original[index].take())
            .collect();
        // All entries are Code(0..n), now pointing at the exact rendered order.
        count
    }

    pub(super) fn prompt(
        &self,
        query: &SearchQuery,
        language: &str,
        count: usize,
        rank_prefix: bool,
    ) -> Option<String> {
        let indices = if rank_prefix {
            (0..count).collect::<Vec<_>>()
        } else {
            let indices = self.packing_order(count);
            if count > 1
                && self.context_scope == ContextScope::Ordinary
                && self.current_code_head().is_some()
                && indices.iter().take(count).copied().eq(0..count)
            {
                // The worker already measured this exact prefix.
                return None;
            }
            indices
        };
        let passages: Vec<_> = indices
            .into_iter()
            .take(count)
            .map(|index| &self.order[index])
            .enumerate()
            .map(|(index, source)| {
                let mut value = self.passage(*source);
                value["id"] = json!(index + 1);
                value
            })
            .collect();
        Some(
            json!({"question":query.original(),"answerLanguage":language,"passages":passages})
                .to_string()
                .replace('<', "\\u003c")
                .replace('>', "\\u003e"),
        )
    }

    fn validate_code(
        &self,
        answers: &Answers,
        count: usize,
        draft: &Draft,
        scope: Option<&CodeScope>,
    ) -> Result<Option<Vec<Citation>>, AnswerError> {
        let validate = if self.context_scope == ContextScope::RootReadme {
            Answers::validate_root_readme
        } else {
            Answers::validate
        };
        validate(answers, &self.code[..count], draft, scope)
    }

    pub(super) fn validate(
        &self,
        answers: &Answers,
        count: usize,
        draft: &Draft,
        scope: Option<&CodeScope>,
    ) -> Result<Option<Vec<Citation>>, AnswerError> {
        if draft.answer.trim().is_empty()
            || draft.answer.len() > 2048
            || draft.citations.is_empty()
            || draft.citations.len() > MAX_PASSAGE_CITATIONS
            || draft
                .citations
                .iter()
                .any(|r| r.passage == 0 || r.passage > count)
        {
            return Ok(None);
        }
        if self.order[..count]
            .iter()
            .all(|source| matches!(source, Source::Code(_)))
        {
            if self.requires_attempt {
                return Ok(None);
            }
            return self.validate_code(answers, count, draft, scope);
        }
        let mut citations = Vec::new();
        let mut code_count = 0;
        let mut code_draft = Draft {
            answer: draft.answer.clone(),
            citations: Vec::new(),
        };
        for (index, source) in self.order[..count].iter().enumerate() {
            let quotes: Vec<_> = draft
                .citations
                .iter()
                .filter(|r| r.passage == index + 1)
                .map(|r| r.quote.trim())
                .collect();
            match *source {
                Source::Code(i) => {
                    code_count += 1;
                    for quote in quotes {
                        code_draft.citations.push(Reference {
                            passage: i + 1,
                            quote: quote.into(),
                        });
                    }
                }
                Source::Change(i) => {
                    let located = &self.changes[i];
                    let selection = history::Selection {
                        scope: &located.scope,
                        child: &located.child,
                    };
                    let pair = &located.pair;
                    let Some(current) = answers
                        .history
                        .as_ref()
                        .ok_or(AnswerError::Storage)?
                        .reopen(selection, pair)?
                    else {
                        return Ok(None);
                    };
                    if current.hunk.before.as_ref() != Some(&pair.before)
                        || current.hunk.after.as_ref() != Some(&pair.after)
                    {
                        return Ok(None);
                    }
                    if !quotes.is_empty() {
                        let Some(pair_citations) =
                            change_quotes(located, &quotes, current.retained)
                        else {
                            return Ok(None);
                        };
                        citations.extend(pair_citations);
                    }
                }
                Source::Run(i) => {
                    let located = &self.runs[i];
                    let Some(run_citations) = actions::reopen_citations(
                        answers.actions.as_deref().ok_or(AnswerError::Storage)?,
                        &located.selection(),
                        &located.observation,
                        &located.observation.fields(),
                        &quotes,
                    )?
                    else {
                        return Ok(None);
                    };
                    citations.extend(run_citations);
                }
            }
        }
        if code_count > 0 {
            let Some(mut code) = answers.validate(&self.code[..code_count], &code_draft, scope)?
            else {
                return Ok(None);
            };
            code.append(&mut citations);
            citations = code;
        }
        let supported =
            !self.requires_attempt || citations.iter().any(|c| c.workflow_run.is_some());
        Ok((supported && citations.len() <= MAX_SOURCES).then_some(citations))
    }
}

// Keep the first ranked anchor. When the complete payload exceeds the actual
// tokenizer limit, discard the largest remaining body first (lower rank breaks
// ties). Preserve ranked order within the admitted set and append omitted slots
// for the evidence-review mapping. Each resulting prompt is measured by llama.
fn pack_indices(sizes: &[usize], count: usize) -> Vec<usize> {
    let mut remaining: Vec<_> = (0..sizes.len()).collect();
    let mut omitted = Vec::new();
    while remaining.len() > count.max(1) {
        let position = (1..remaining.len())
            .max_by_key(|&i| (sizes[remaining[i]], i))
            .unwrap_or(0);
        omitted.push(remaining.remove(position));
    }
    remaining.extend(omitted);
    remaining
}

pub(super) fn code_passage(excerpt: &Excerpt) -> Value {
    let mut value = json!({"kind":excerpt.evidence.kind,"text":excerpt.text});
    if let Some(location) = &excerpt.evidence.location {
        value["source"] = json!({"path": location.path, "revision": excerpt.evidence.revision,
            "startLine": location.start_line, "endLine": location.end_line});
    }
    value
}

fn change_quotes(located: &LocatedPair, quotes: &[&str], retained: bool) -> Option<Vec<Citation>> {
    let pair = &located.pair;
    let selection = history::Selection {
        scope: &located.scope,
        child: &located.child,
    };
    let mut citations = Vec::new();
    let mut before = false;
    let mut after = false;
    for &quote in quotes {
        if quote.is_empty() {
            return None;
        }
        let mut matches = [(Side::Before, &pair.before), (Side::After, &pair.after)]
            .into_iter()
            .flat_map(|(side, anchor)| {
                anchor
                    .text
                    .lines()
                    .enumerate()
                    .filter(move |(_, line)| line.trim() == quote)
                    .map(move |(offset, _)| (side, anchor.start_line + offset))
            });
        let (side, line) = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        let citation = history::citation(selection, pair, side, line, retained)?;
        match side {
            Side::Before => before = true,
            Side::After => after = true,
        }
        citations.push(citation);
    }
    (before && after).then_some(citations)
}

#[cfg(test)]
mod packing_tests {
    use super::*;
    use crate::application::SearchHit;

    #[test]
    fn rendered_compact_ids_and_validation_order_refer_to_the_same_passages()
    -> Result<(), Box<dyn std::error::Error>> {
        let code = ["top", "long secondary passage", "tail"]
            .into_iter()
            .map(|text| Excerpt {
                evidence: SearchHit {
                    source: text.into(),
                    title: text.into(),
                    revision: "pin".into(),
                    kind: "current".into(),
                    tracked_ref: Some("refs/heads/main".into()),
                    access: "available",
                    location: None,
                },
                text: text.into(),
                repository_id: Some(7),
            })
            .collect();
        let mut sources = Sources {
            code,
            changes: vec![],
            runs: vec![],
            searched_history: false,
            searched_actions: false,
            context_scope: ContextScope::Ordinary,
            requires_attempt: false,
            order: (0..3).map(Source::Code).collect(),
        };
        let query =
            SearchQuery::parse_scoped("What does the tail show?", Some(7), Some("main".into()))?;
        let mut review = sources.review(&query, Vec::new(), false);
        let prompt: Value = serde_json::from_str(
            &sources
                .prompt(&query, "en", 2, false)
                .ok_or("distinct compact layout")?,
        )?;
        assert_eq!(prompt["passages"][1]["id"], 2);
        assert_eq!(prompt["passages"][1]["text"], "tail");
        assert_eq!(sources.apply_compaction(2, Some(&mut review)), 2);
        assert_eq!(sources.code[1].text, "tail");
        review.unverified(2, &[2].into_iter());
        let value = serde_json::to_value(review)?;
        assert_eq!(value["candidates"][1]["slot"], 3);
        assert_eq!(value["candidates"][2]["slot"], 2);
        Ok(())
    }

    #[test]
    fn a_large_middle_passage_can_yield_to_two_later_passages() {
        assert_eq!(pack_indices(&[100, 900, 190, 140], 3), [0, 2, 3, 1]);
        assert_eq!(pack_indices(&[100, 900, 190, 140], 4), [0, 1, 2, 3]);
        assert_eq!(pack_indices(&[10, 10, 10], 2), [0, 1, 2]);
    }
}

#[cfg(test)]
mod java_method_tests {
    use super::*;
    use crate::{application::SearchHit, domain::github::history::hunks::Anchor};

    struct Promoted;
    impl EvidenceStore for Promoted {
        fn java_method_passages(&self, _: &SearchQuery) -> Result<Vec<Excerpt>, AnswerError> {
            Ok(vec![excerpt("Feature.java")])
        }

        fn excerpts(
            &self,
            _: &[SearchHit],
            _: Option<&CodeScope>,
        ) -> Result<Vec<Excerpt>, AnswerError> {
            Ok(Vec::new())
        }

        fn following(
            &self,
            _: &[SearchHit],
            _: Option<&CodeScope>,
        ) -> Result<Vec<Excerpt>, AnswerError> {
            Ok(Vec::new())
        }
    }

    fn excerpt(source: &str) -> Excerpt {
        Excerpt {
            evidence: SearchHit {
                source: source.into(),
                revision: "pin".into(),
                kind: "current".into(),
                tracked_ref: Some("refs/heads/main".into()),
                title: source.into(),
                access: "verified",
                location: None,
            },
            text: "original source".into(),
            repository_id: Some(42),
        }
    }

    fn code_sources() -> Sources {
        Sources {
            code: vec![excerpt("Distractor.java"), excerpt("Feature.java")],
            changes: Vec::new(),
            runs: Vec::new(),
            searched_history: false,
            searched_actions: false,
            context_scope: ContextScope::Ordinary,
            requires_attempt: false,
            order: vec![Source::Code(0), Source::Code(1)],
        }
    }

    #[test]
    fn exact_method_promotion_preserves_lexical_rank_and_remaining_source_order()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut sources = code_sources();
        let query = SearchQuery::parse("How does Feature equals work?")?;
        let lexical = vec![
            RankedCandidate::new(&sources.code[0].evidence, 1),
            RankedCandidate::new(&sources.code[1].evidence, 6),
        ];
        assert!(sources.prefer_java_method(&Promoted, &query)?.is_some());
        assert_eq!(sources.code.len(), 2);
        assert_eq!(sources.code[0].evidence.source, "Feature.java");
        assert_eq!(sources.code[1].evidence.source, "Distractor.java");
        let audit = serde_json::to_value(sources.review(&query, lexical, false))?;
        assert_eq!(audit["candidates"][0]["rank"], 1);
        assert_eq!(audit["candidates"][0]["slot"], 2);
        assert_eq!(audit["candidates"][1]["rank"], 6);
        assert_eq!(audit["candidates"][1]["slot"], 1);

        let lexical = vec![RankedCandidate::new(&sources.code[1].evidence, 1)];
        let audit = serde_json::to_value(sources.review(&query, lexical, true))?;
        assert_eq!(audit["searches"][0]["candidateCount"], 1);
        assert_eq!(audit["candidates"][0]["contextAdded"], true);
        assert_eq!(audit["candidates"][0]["slot"], 1);
        assert_eq!(audit["candidates"][0]["text"], sources.code[0].text);
        assert!(audit["candidates"][1].get("contextAdded").is_none());
        Ok(())
    }

    #[test]
    fn exact_method_promotion_does_not_displace_history_or_required_actions()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut sources = code_sources();
        let query = SearchQuery::parse("How did Feature equals change?")?;
        sources.requires_attempt = true;
        assert!(sources.prefer_java_method(&Promoted, &query)?.is_none());
        sources.requires_attempt = false;
        let anchor = Anchor {
            path: "Feature.java".into(),
            blob: "blob".into(),
            start_byte: 0,
            end_byte: 8,
            start_line: 1,
            end_line: 1,
            text: "contents".into(),
        };
        sources.changes.push(LocatedPair {
            scope: CodeScope {
                repository_id: 42,
                branch: "main".into(),
            },
            child: "child".into(),
            pair: history::Pair {
                key: history::HunkKey { change: 0, hunk: 0 },
                repository: "org/repo".into(),
                parent: "parent".into(),
                before: anchor.clone(),
                after: anchor,
            },
        });
        sources.order.insert(1, Source::Change(0));
        assert!(sources.prefer_java_method(&Promoted, &query)?.is_none());
        assert!(sources.order == [Source::Code(0), Source::Change(0), Source::Code(1)]);
        assert_eq!(sources.code[0].evidence.source, "Distractor.java");
        Ok(())
    }
}

#[cfg(test)]
mod readme_authority_tests {
    use super::*;
    use crate::{application::SearchHit, domain::corpus::snapshot::Location};

    struct Readme;
    impl EvidenceStore for Readme {
        fn authoritative_readme(
            &self,
            _: &SearchQuery,
        ) -> Result<Option<Vec<Excerpt>>, AnswerError> {
            Ok(Some(
                [
                    ("closing evidence", 20),
                    ("opening context", 0),
                    ("middle context", 10),
                ]
                .into_iter()
                .map(|(text, start)| Excerpt {
                    evidence: SearchHit {
                        source: "github:81:README.md".into(),
                        revision: "pin".into(),
                        kind: "current".into(),
                        tracked_ref: Some("refs/heads/main".into()),
                        title: "README.md".into(),
                        access: "available",
                        location: Some(Location {
                            path: "README.md".into(),
                            blob: "one".into(),
                            file_url: "url".into(),
                            start_byte: start,
                            end_byte: start + u32::try_from(text.len()).unwrap_or_default(),
                            start_line: start + 1,
                            end_line: start + 1,
                        }),
                    },
                    text: text.into(),
                    repository_id: Some(81),
                })
                .collect(),
            ))
        }
        fn excerpts(
            &self,
            _: &[SearchHit],
            _: Option<&CodeScope>,
        ) -> Result<Vec<Excerpt>, AnswerError> {
            Ok(Vec::new())
        }
        fn following(
            &self,
            _: &[SearchHit],
            _: Option<&CodeScope>,
        ) -> Result<Vec<Excerpt>, AnswerError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn authority_uses_only_readme_and_keeps_relevant_passage_when_compacted()
    -> Result<(), Box<dyn std::error::Error>> {
        let query = SearchQuery::parse("According to the README, what is documented?")?;
        let mut scoped = Sources::from_authoritative_readme(&Readme, &query)?.ok_or("scope")?;
        assert!(!scoped.searched_history && !scoped.searched_actions);
        assert!(matches!(
            scoped.passage_packing(),
            PassagePacking::Compact(3)
        ));
        let full: Value =
            serde_json::from_str(&scoped.prompt(&query, "en", 3, false).ok_or("full")?)?;
        assert_eq!(full["passages"][0]["text"], "opening context");
        assert_eq!(full["passages"][1]["text"], "middle context");
        assert_eq!(full["passages"][2]["text"], "closing evidence");
        let short: Value =
            serde_json::from_str(&scoped.prompt(&query, "en", 2, false).ok_or("compact")?)?;
        assert_eq!(short["passages"][0]["text"], "opening context");
        assert_eq!(short["passages"][1]["text"], "closing evidence");
        let mut audit = scoped.review(&query, Vec::new(), false);
        assert_eq!(scoped.apply_compaction(2, Some(&mut audit)), 2);
        assert_eq!(scoped.code[1].text, "closing evidence");
        let audit = serde_json::to_value(audit)?;
        assert_eq!(audit["searches"].as_array().map(Vec::len), Some(1));
        assert_eq!(audit["searches"][0]["selection"], "published root README");
        Ok(())
    }
}
