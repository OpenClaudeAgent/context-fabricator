use std::{path::Path, sync::Arc, time::Duration};

use ctxfab_inference::generation::{
    Draft, Generation, GenerationAttempt, GenerationError, GenerationResult, GenerationStop,
    MAX_PASSAGE_CITATIONS, Models, Request,
};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;
use whichlang::Lang;

use super::{
    AnswerLanguage, CodeScope, SearchHit, SearchQuery,
    ports::Repository,
    retrieval::{Mode, Retrieval, RetrievalError},
    telemetry::{
        Event, FusionRanking, GenerationContext, GenerationEvidence, Observer, Stage, TraceLink,
    },
};
use code_quotes::VerifiedQuote;
pub use evidence_review::EvidenceReview;
use evidence_review::RankedCandidate;
pub use model_context::ModelContext;

const MAX_SOURCES: usize = 5;
const MIN_QUOTE_BYTES: usize = 8;

pub mod actions;
mod actions_capability;
mod actions_scope;
mod code_quotes;
mod context;
mod evidence_review;
mod graph_context;
pub mod history;
mod java_doc_quotes;
mod latest_workflow;
mod linked;
mod markdown_quotes;
mod mixed;
mod model_context;
mod proof;
mod quantities;
mod submission;

pub use submission::Submission;

pub struct Excerpt {
    pub evidence: SearchHit,
    pub text: String,
    pub repository_id: Option<i64>,
}

#[derive(Default)]
pub struct JavaEvidence {
    pub passages: Vec<Excerpt>,
    pub class_local: bool,
}

pub trait EvidenceStore: Send + Sync {
    /// Find ordinary indexed passages covering a uniquely named Java method.
    ///
    /// # Errors
    /// Returns a storage error when the publication cannot be read.
    fn java_method_passages(&self, _query: &SearchQuery) -> Result<Vec<Excerpt>, AnswerError> {
        Ok(Vec::new())
    }

    /// Retrieve Java passages from the exact method or a verified named class.
    /// # Errors
    /// Returns a storage error when the publication cannot be read.
    fn java_evidence(&self, query: &SearchQuery) -> Result<JavaEvidence, AnswerError> {
        Ok(JavaEvidence {
            passages: self.java_method_passages(query)?,
            class_local: false,
        })
    }

    /// Load bounded published document bytes for exact same-slot quote validation.
    /// # Errors
    /// Returns a storage error when the immutable source cannot be read.
    fn current_document(
        &self,
        _excerpt: &Excerpt,
        _scope: Option<&CodeScope>,
    ) -> Result<Option<String>, AnswerError> {
        Ok(None)
    }

    /// Read an explicitly authoritative root README from a unique publication.
    /// # Errors
    /// Returns a storage error when the publication cannot be read.
    fn authoritative_readme(
        &self,
        _query: &SearchQuery,
    ) -> Result<Option<Vec<Excerpt>>, AnswerError> {
        Ok(None)
    }

    /// Resolve an explicitly requested section of the current root Markdown README.
    ///
    /// # Errors
    /// Returns a storage error when a local publication cannot be read.
    fn readme_section(&self, _query: &SearchQuery) -> Result<Option<SearchHit>, AnswerError> {
        Ok(None)
    }

    /// Resolve a qualified Rust name to passages in a published Branch files snapshot.
    ///
    /// # Errors
    /// Returns a storage error when a local publication cannot be read.
    fn graph_passages(
        &self,
        _symbol: &str,
        _root: Option<&str>,
        _scope: Option<&CodeScope>,
    ) -> Result<Vec<Excerpt>, AnswerError> {
        Ok(Vec::new())
    }

    /// Reads available passages in the requested order, skipping missing identities.
    ///
    /// # Errors
    /// Returns a storage error if a coherent snapshot cannot be read.
    fn excerpts(
        &self,
        hits: &[SearchHit],
        scope: Option<&CodeScope>,
    ) -> Result<Vec<Excerpt>, AnswerError>;

    /// Reads contiguous following passages for immutable code hits in the same scope.
    ///
    /// # Errors
    /// Returns a storage error if a coherent snapshot cannot be read.
    fn following(
        &self,
        hits: &[SearchHit],
        scope: Option<&CodeScope>,
    ) -> Result<Vec<Excerpt>, AnswerError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AnswerError {
    #[error("Evidence storage is unavailable.")]
    Storage,
    #[error(transparent)]
    Generation(#[from] GenerationError),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub evidence: SearchHit,
    pub quote: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comparison: Option<history::ComparisonCitation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_run: Option<actions::RunCitation>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnswerMode {
    Bm25,
    Semantic,
    Hybrid,
    Selected,
}

impl From<Mode> for AnswerMode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Bm25 => Self::Bm25,
            Mode::Semantic => Self::Semantic,
            Mode::Hybrid => Self::Hybrid,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    pub outcome: &'static str,
    pub model: String,
    pub mode: AnswerMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub citations: Vec<Citation>,
    pub evidence_repository_ids: Vec<i64>,
    #[serde(skip)]
    pub model_context: Option<ModelContext>,
    #[serde(skip)]
    pub evidence_review: Option<EvidenceReview>,
    #[serde(skip)]
    pub(crate) journal_repository_ids: Vec<i64>,
}

#[derive(Clone)]
pub struct Answers {
    repository: Arc<dyn Repository>,
    evidence: Arc<dyn EvidenceStore>,
    retrieval: Option<Arc<Retrieval>>,
    history: Option<Arc<dyn history::HistoryStore>>,
    actions: Option<Arc<dyn actions::ActionsStore>>,
    generation: Arc<Generation>,
}

impl Answer {
    fn withheld(
        request: &Request<'_>,
        mode: AnswerMode,
        evidence_review: Option<EvidenceReview>,
        journal_repository_ids: Vec<i64>,
    ) -> Self {
        Self {
            outcome: "insufficientEvidence",
            model: request.model().into(),
            mode,
            text: None,
            citations: Vec::new(),
            evidence_repository_ids: Vec::new(),
            model_context: None,
            evidence_review,
            journal_repository_ids,
        }
    }

    fn stopped(mut self, stop: GenerationStop) -> Self {
        if let Some(review) = &mut self.evidence_review {
            review.stopped(stop);
        }
        self
    }
}

pub struct AnswerRequest<'a> {
    pub id: Option<Uuid>,
    pub observer: Option<&'a dyn Observer>,
    pub trace: Option<TraceLink>,
    pub include_model_context: bool,
    pub inspect_pipeline: bool,
    pub record_model: &'a dyn Fn(&str),
}

struct Retrieved {
    mode: Mode,
    excerpts: Vec<Excerpt>,
    formulated: bool,
    fusion: Option<FusionRanking>,
    fallback: Option<Event>,
    passage_candidates: usize,
    audited: Option<Vec<RankedCandidate>>,
    embedding: Option<super::retrieval::EmbeddingCapture>,
}

struct PassageAnswer {
    answer: Answer,
    sources: mixed::Sources,
    language: AnswerLanguage,
}

type ReviewedSources = (Mode, mixed::Sources, bool, Option<EvidenceReview>);

impl Answers {
    #[must_use]
    pub fn new(
        repository: Arc<dyn Repository>,
        evidence: Arc<dyn EvidenceStore>,
        retrieval: Option<Arc<Retrieval>>,
        generation: Arc<Generation>,
    ) -> Self {
        Self {
            repository,
            evidence,
            retrieval,
            history: None,
            actions: None,
            generation,
        }
    }

    #[must_use]
    pub(crate) fn for_corpus(
        &self,
        repository: Arc<dyn Repository>,
        evidence: Arc<dyn EvidenceStore>,
        retrieval: Option<Arc<Retrieval>>,
    ) -> Self {
        Self::new(repository, evidence, retrieval, self.generation.clone())
    }

    #[must_use]
    pub fn with_history(mut self, history: Arc<dyn history::HistoryStore>) -> Self {
        self.history = Some(history);
        self
    }

    #[must_use]
    pub fn with_actions(mut self, actions: Arc<dyn actions::ActionsStore>) -> Self {
        self.actions = Some(actions);
        self
    }

    /// # Errors
    /// Returns a typed error if evidence or local generation is unavailable.
    pub fn ask(
        &self,
        query: &SearchQuery,
        metadata: &AnswerRequest<'_>,
    ) -> Result<Answer, AnswerError> {
        let request = self.begin_answer(metadata)?;
        let result = if actions_capability::needs_job_observation(query, false) {
            Ok(Answer::withheld(
                &request,
                AnswerMode::Selected,
                None,
                Vec::new(),
            ))
        } else {
            self.answer(query, &request, metadata)
        };
        request.finish()?;
        result
    }

    fn begin_answer(&self, metadata: &AnswerRequest<'_>) -> Result<Request<'_>, GenerationError> {
        let request = self.generation.begin(metadata.id)?;
        (metadata.record_model)(request.model());
        Ok(request.with_context_capture(metadata.include_model_context))
    }

    /// # Errors
    /// Returns an error if generation state is unavailable.
    pub fn models(&self) -> Result<Models, GenerationError> {
        self.generation.models()
    }

    /// # Errors
    /// Returns an error if the requested profile cannot be activated.
    pub fn select_model(&self, profile: &str) -> Result<Models, GenerationError> {
        self.generation.select(profile)
    }

    /// # Errors
    /// Returns an error when setup cannot warm or commit the selected profile.
    pub fn select_setup_model(
        &self,
        profile: &str,
        commit: impl FnOnce(&str, &str) -> Result<(), ctxfab_inference::generation::SelectionStoreError>,
    ) -> Result<Models, GenerationError> {
        self.generation.select_with_commit(profile, commit)
    }

    /// # Errors
    /// Returns an error if the request ID no longer belongs to the active job.
    pub fn cancel(&self, id: Uuid) -> Result<(), GenerationError> {
        self.generation.cancel(id)
    }

    fn answer(
        &self,
        query: &SearchQuery,
        request: &Request<'_>,
        metadata: &AnswerRequest<'_>,
    ) -> Result<Answer, AnswerError> {
        if let Some(answer) = self.answer_latest_workflow(query, request, metadata)? {
            return Ok(answer);
        }
        let (observer, trace) = (metadata.observer, metadata.trace);
        let detected = query
            .can_reformulate()
            .then(|| whichlang::detect_language(query.original()));
        let reformulate = detected.is_some_and(|language| language != Lang::Eng);
        let (mode, sources, formulated, evidence_review) =
            self.retrieve_with_trace(query, request, metadata, reformulate)?;
        let mut empty = Answer::withheld(
            request,
            mode.into(),
            evidence_review,
            sources.repository_ids(sources.order.len()),
        );
        if sources.order.is_empty() || sources.missing_required_attempt() {
            return Ok(empty);
        }

        let (language, language_instruction) = self.response_language(detected)?;
        let prefer_implementation = formulated && !context::asks_for_fixtures(query);
        let attempted_pair = if prefer_implementation
            && let Some(head) = sources.current_code_head()
        {
            match self.prove_linked(query, request, head, language_instruction, observer, trace)? {
                proof::Outcome::Verified(proof) => {
                    empty.model_context = proof.model_context;
                    return Ok(answer_from_citations(
                        empty,
                        &proof.draft,
                        language,
                        proof.citations,
                        vec![proof.repository_id],
                    ));
                }
                proof::Outcome::Invalidated => {
                    if let Some(review) = &mut empty.evidence_review {
                        review.invalidated();
                    }
                    return Ok(empty);
                }
                proof::Outcome::Skipped => false,
                proof::Outcome::Unsupported => true,
            }
        } else {
            false
        };
        let generated =
            observe_generation(observer, trace, GenerationEvidence::Passages, |record| {
                request.generate_with_packing(
                    sources.passage_packing(),
                    prefer_implementation && language == AnswerLanguage::Fr,
                    attempted_pair,
                    record,
                    |count, prefix| sources.prompt(query, language_instruction, count, prefix),
                )
            })?;
        let result = match generated {
            GenerationAttempt::Completed(result) => result,
            GenerationAttempt::WithoutDraft(stop) => return Ok(empty.stopped(stop)),
        };
        self.finish_passage_answer(
            query,
            request,
            metadata,
            result,
            PassageAnswer {
                answer: empty,
                sources,
                language,
            },
        )
    }

    fn finish_passage_answer(
        &self,
        query: &SearchQuery,
        request: &Request<'_>,
        metadata: &AnswerRequest<'_>,
        result: GenerationResult<Draft>,
        passage: PassageAnswer,
    ) -> Result<Answer, AnswerError> {
        let PassageAnswer {
            answer: mut empty,
            mut sources,
            language,
        } = passage;
        let (observer, trace) = (metadata.observer, metadata.trace);
        let admitted = sources.apply_packing(&result, empty.evidence_review.as_mut());
        let draft = result.draft;
        if let Some(capture) = empty
            .evidence_review
            .as_mut()
            .and_then(|review| review.pipeline.as_mut())
        {
            empty.model_context =
                ModelContext::from_completion(result.completion_prompt.as_deref());
            capture
                .completion_input
                .clone_from(&result.completion_prompt);
            capture.draft = Some(draft.clone());
        }
        let Some(citations) =
            self.verify_passage_draft(&sources, query, admitted, &draft, metadata)?
        else {
            if let Some(review) = &mut empty.evidence_review {
                review.unverified(
                    admitted,
                    &draft.citations.iter().map(|citation| citation.passage),
                );
            }
            if let Some(located) = sources.comparison_candidate(admitted, &draft) {
                return self
                    .compare_history(
                        query,
                        history::Selection {
                            scope: &located.scope,
                            child: &located.child,
                        },
                        std::slice::from_ref(&located.pair),
                        request,
                        observer,
                        trace,
                    )
                    .map(|mut answer| {
                        answer.journal_repository_ids = empty.journal_repository_ids;
                        answer
                    });
            }
            return Ok(empty);
        };
        empty.model_context = ModelContext::from_completion(result.completion_prompt.as_deref());
        if let Some(review) = empty
            .evidence_review
            .as_mut()
            .filter(|review| review.capturing())
        {
            review.verified(admitted, &draft);
        }
        Ok(answer_from_citations(
            empty,
            &draft,
            language,
            citations,
            sources.repository_ids(admitted),
        ))
    }

    fn verify_passage_draft(
        &self,
        sources: &mixed::Sources,
        query: &SearchQuery,
        admitted: usize,
        draft: &Draft,
        metadata: &AnswerRequest<'_>,
    ) -> Result<Option<Vec<Citation>>, AnswerError> {
        let verification = metadata
            .observer
            .map(|observer| observer.start(Stage::AnswerValidation, metadata.trace));
        let citations = sources.validate(self, admitted, draft, query.scope());
        if let Some(mut verification) = verification {
            if let Ok(citations) = &citations {
                verification.event(if citations.is_some() {
                    Event::CitationsVerified
                } else {
                    Event::CitationsRejected
                });
            }
            verification.finish(matches!(citations, Ok(Some(_))));
        }
        citations
    }

    fn retrieve_with_trace(
        &self,
        query: &SearchQuery,
        request: &Request<'_>,
        metadata: &AnswerRequest<'_>,
        reformulate: bool,
    ) -> Result<ReviewedSources, AnswerError> {
        let mut lookup = metadata
            .observer
            .map(|observer| observer.start(Stage::AnswerRetrieval, metadata.trace));
        if query.readme_authority_reference().is_some()
            && let Some(sources) =
                mixed::Sources::from_authoritative_readme(self.evidence.as_ref(), query)?
        {
            if let Some(mut span) = lookup {
                span.event(Event::RootReadmeAuthoritySelected);
                span.retrieval_counts(sources.counts(sources.order.len()));
                span.finish(!sources.order.is_empty());
            }
            let audit = metadata.include_model_context.then(|| {
                let mut review = sources.review(query, Vec::new(), metadata.inspect_pipeline);
                review.pipeline = metadata
                    .inspect_pipeline
                    .then(evidence_review::Pipeline::default);
                review
            });
            return Ok((Mode::Bm25, sources, false, audit));
        }
        let sources = self
            .retrieve(query, request, reformulate, metadata)
            .and_then(|mut retrieved| {
                let graph = graph_context::contribute(
                    self.evidence.as_ref(),
                    query,
                    &mut retrieved.excerpts,
                )?;
                if let Some(span) = &mut lookup {
                    if graph {
                        span.event(Event::RustGraphPassagesSelected);
                    }
                    if let Some(ranking) = &retrieved.fusion {
                        span.fusion_ranking(ranking);
                    }
                    if let Some(reason) = retrieved.fallback {
                        span.event(reason);
                    }
                }
                mixed::Sources::discover(self, query, retrieved.excerpts).and_then(|mut sources| {
                    let promoted = if graph {
                        None
                    } else {
                        sources.prefer_java_method(self.evidence.as_ref(), query)?
                    };
                    if let Some(span) = &mut lookup {
                        if let Some(event) = promoted {
                            span.event(event);
                        }
                        span.retrieval_counts(sources.counts(retrieved.passage_candidates));
                    }
                    let audit = retrieved.audited.map(|ranked| {
                        let mut review = sources.review(query, ranked, metadata.inspect_pipeline);
                        if metadata.inspect_pipeline {
                            review.pipeline = Some(evidence_review::Pipeline {
                                fusion: retrieved.fusion,
                                embedding: retrieved.embedding,
                                fallback: retrieved.fallback.map(Event::name),
                                ..evidence_review::Pipeline::default()
                            });
                        }
                        review
                    });
                    Ok((retrieved.mode, sources, retrieved.formulated, audit))
                })
            });
        if let Some(mut lookup) = lookup {
            if let Ok((mode, sources, formulated, _)) = &sources {
                let plan = match mode {
                    Mode::Hybrid => Event::HybridSelected,
                    Mode::Bm25 if self.retrieval.is_some() => Event::Bm25Fallback,
                    Mode::Bm25 => Event::Bm25Selected,
                    Mode::Semantic => Event::SemanticSelected,
                };
                lookup.event(plan);
                if *formulated {
                    lookup.event(Event::SearchFormulationRanked);
                }
                if sources.order.is_empty() {
                    lookup.event(Event::NoEvidence);
                }
            }
            lookup.finish(sources.is_ok());
        }
        sources
    }

    fn response_language(
        &self,
        detected: Option<Lang>,
    ) -> Result<(AnswerLanguage, &'static str), AnswerError> {
        let configured = self
            .repository
            .settings()
            .map_err(|_| AnswerError::Storage)?
            .preferred_answer_language();
        let language = configured.resolve(detected);
        Ok((language, answer_language(language)))
    }

    fn retrieve(
        &self,
        query: &SearchQuery,
        request: &Request<'_>,
        reformulate: bool,
        metadata: &AnswerRequest<'_>,
    ) -> Result<Retrieved, AnswerError> {
        let inspect = metadata.inspect_pipeline;
        let observe = metadata.observer.is_some() || inspect;
        let capture = metadata.include_model_context;
        let mut fallback = None;
        if let Some(retrieval) = &self.retrieval {
            let ranking = retrieval.search_ranked(query, Mode::Hybrid, observe, inspect, || {
                if !reformulate {
                    return None;
                }
                request
                    .search_formulation(query.original(), Duration::from_secs(3))
                    .ok()
                    .flatten()
            });
            request.check_cancelled()?;
            match ranking {
                Ok(ranking) => {
                    let formulated = !ranking.auxiliary.is_empty();
                    let passage_candidates = if observe && formulated {
                        context::distinct_ranked_passages(&ranking.original, &ranking.auxiliary)
                    } else {
                        ranking.original.len()
                    };
                    let mut excluded = capture.then(Vec::new);
                    let fusion_order = inspect.then(|| ranking.original.clone());
                    let hits = context::merge_formulation(
                        query,
                        ranking.original,
                        ranking.auxiliary,
                        excluded.as_mut(),
                    );
                    let mut audited = capture.then(|| ranked_candidates(&hits));
                    let excerpts = context::select(
                        self.evidence.as_ref(),
                        query,
                        &hits,
                        formulated,
                        audited.as_deref_mut(),
                    )?;
                    if let (Some(audited), Some(excluded)) = (&mut audited, excluded) {
                        audited.extend(excluded);
                    }
                    self.capture_candidates(query, &mut audited, fusion_order.as_deref(), inspect);
                    return Ok(Retrieved {
                        mode: Mode::Hybrid,
                        excerpts,
                        formulated,
                        fusion: ranking.fusion,
                        fallback: None,
                        passage_candidates,
                        audited,
                        embedding: ranking.embedding,
                    });
                }
                Err(error) => {
                    fallback = Some(match error {
                        RetrievalError::Disabled => Event::EmbeddingIndexDisabled,
                        RetrievalError::Incomplete => Event::EmbeddingIndexIncomplete,
                        RetrievalError::Runtime => Event::EmbeddingWorkerUnavailable,
                        RetrievalError::Busy => Event::EmbeddingWorkerBusy,
                        _ => return Err(AnswerError::Storage),
                    });
                }
            }
        }
        let hits = self
            .repository
            .search(query)
            .map_err(|_| AnswerError::Storage)?;
        let mut audited = capture.then(|| ranked_candidates(&hits));
        let excerpts = context::select(
            self.evidence.as_ref(),
            query,
            &hits,
            false,
            audited.as_deref_mut(),
        )?;
        self.capture_candidates(query, &mut audited, None, inspect);
        Ok(Retrieved {
            mode: Mode::Bm25,
            excerpts,
            formulated: false,
            fusion: None,
            fallback,
            passage_candidates: hits.len(),
            audited,
            embedding: None,
        })
    }

    fn capture_candidates(
        &self,
        query: &SearchQuery,
        audited: &mut Option<Vec<RankedCandidate>>,
        fusion_order: Option<&[SearchHit]>,
        inspect: bool,
    ) {
        let Some(candidates) = audited.as_mut().filter(|_| inspect) else {
            return;
        };
        let hits: Vec<_> = candidates
            .iter()
            .map(|candidate| candidate.hit.clone())
            .collect();
        let excerpts = self
            .evidence
            .excerpts(&hits, query.scope())
            .unwrap_or_default();
        for candidate in candidates {
            candidate.set_fusion_rank(fusion_order.and_then(|hits| {
                hits.iter()
                    .position(|hit| candidate.hit.same_evidence(hit))
                    .map(|rank| rank + 1)
            }));
            if let Some(excerpt) = excerpts
                .iter()
                .find(|hit| candidate.hit.same_evidence(&hit.evidence))
            {
                candidate.set_text(excerpt.text.clone());
            }
        }
    }

    fn validate(
        &self,
        sources: &[Excerpt],
        draft: &Draft,
        scope: Option<&CodeScope>,
    ) -> Result<Option<Vec<Citation>>, AnswerError> {
        if draft.answer.trim().is_empty()
            || draft.answer.len() > 2_048
            || draft.citations.len() > MAX_PASSAGE_CITATIONS
        {
            return Ok(None);
        }
        let used = if let Some(used) = verified_quotes(sources, draft) {
            used
        } else {
            let repaired = self.repair_quote_layout(sources, draft, scope)?;
            let Some(used) = verified_quotes_with_breaks(sources, draft, &repaired) else {
                return Ok(None);
            };
            used
        };
        self.reopen_citations(sources.iter(), used, scope)
    }

    fn validate_root_readme(
        &self,
        sources: &[Excerpt],
        draft: &Draft,
        scope: Option<&CodeScope>,
    ) -> Result<Option<Vec<Citation>>, AnswerError> {
        let original = self.validate(sources, draft, scope)?;
        if original.is_some() {
            return Ok(original);
        }
        let Some(head) = markdown_quotes::published_root(sources) else {
            return Ok(None);
        };
        let Some(document) = self.evidence.current_document(head, scope)? else {
            return Ok(None);
        };
        let Some(relocated) = markdown_quotes::relocate(sources, draft, &document) else {
            return Ok(None);
        };
        self.validate(sources, &relocated, scope)
    }

    fn repair_quote_layout(
        &self,
        sources: &[Excerpt],
        draft: &Draft,
        scope: Option<&CodeScope>,
    ) -> Result<Vec<Option<std::ops::Range<usize>>>, AnswerError> {
        let mut spans = Vec::new();
        for reference in &draft.citations {
            let mut verified = None;
            if let Some(source) = reference
                .passage
                .checked_sub(1)
                .and_then(|i| sources.get(i))
            {
                let quote = reference.quote.trim();
                if let Some(range) = markdown_quotes::escaped_lf_span(source, quote)
                    && let Some(document) = self.evidence.current_document(source, scope)?
                    && markdown_quotes::is_prose_span(source, &range, &document)
                {
                    verified = Some(range);
                } else if let Some(range) = java_doc_quotes::paragraph_span(source, quote)
                    && let Some(document) = self.evidence.current_document(source, scope)?
                    && java_doc_quotes::is_javadoc_span(source, &range, &document)
                {
                    verified = Some(range);
                }
            }
            spans.push(verified);
        }
        Ok(spans)
    }

    fn reopen_citations<'a>(
        &self,
        sources: impl ExactSizeIterator<Item = &'a Excerpt> + Clone,
        used: Vec<VerifiedQuote<'a>>,
        scope: Option<&CodeScope>,
    ) -> Result<Option<Vec<Citation>>, AnswerError> {
        let hits: Vec<_> = sources
            .clone()
            .map(|source| source.evidence.clone())
            .collect();
        // Any prompt passage can influence the answer, including uncited text.
        let current = self.evidence.excerpts(&hits, scope)?;
        if current.len() != sources.len()
            || sources.zip(&current).any(|(before, after)| {
                !before.evidence.same_evidence(&after.evidence)
                    || before.repository_id != after.repository_id
                    || before.text != after.text
            })
        {
            return Ok(None);
        }
        Ok(used
            .into_iter()
            .map(|quote| {
                let previous = quote.excerpt;
                let found = current.iter().find(|excerpt| {
                    excerpt.evidence.same_evidence(&previous.evidence)
                        && excerpt.repository_id == previous.repository_id
                        && excerpt.text == previous.text
                        && excerpt.text.contains(quote.text)
                })?;
                Some(Citation {
                    evidence: found.evidence.clone(),
                    quote: quote.text.to_owned(),
                    line_url: quote.line_url(found),
                    comparison: None,
                    workflow_run: None,
                })
            })
            .collect())
    }
}

fn answer_from_citations(
    mut empty: Answer,
    draft: &Draft,
    language: AnswerLanguage,
    citations: Vec<Citation>,
    evidence_repository_ids: Vec<i64>,
) -> Answer {
    let rust_quotes = citations.iter().filter_map(|citation| {
        let location = citation.evidence.location.as_ref()?;
        let rust = Path::new(&location.path)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rs"));
        (citation.evidence.kind == "current" && rust).then_some(citation.quote.as_str())
    });
    empty.evidence_review = empty
        .evidence_review
        .take()
        .filter(EvidenceReview::capturing);
    Answer {
        outcome: "answered",
        text: Some(quantities::with_verified_unit(
            &draft.answer,
            language,
            rust_quotes,
        )),
        citations,
        evidence_repository_ids,
        ..empty
    }
}

fn ranked_candidates(hits: &[SearchHit]) -> Vec<RankedCandidate> {
    hits.iter()
        .enumerate()
        .map(|(index, hit)| RankedCandidate::new(hit, index + 1))
        .collect()
}

fn observe_generation<T>(
    observer: Option<&dyn Observer>,
    trace: Option<TraceLink>,
    kind: GenerationEvidence,
    generate: impl FnOnce(
        &mut dyn FnMut(ctxfab_inference::generation::ContextBudget),
    ) -> Result<GenerationAttempt<T>, GenerationError>,
) -> Result<GenerationAttempt<T>, AnswerError> {
    let generation = observer.map(|observer| observer.start(Stage::AnswerGeneration, trace));
    let mut budget = None;
    let draft = generate(&mut |context| budget = Some(context));
    if let Some(mut generation) = generation {
        if let Some(budget) = budget {
            generation.context_budget(&GenerationContext { kind, budget });
        }
        let (event, success) = match &draft {
            Ok(GenerationAttempt::Completed(result)) => {
                if let Some(timing) = result.timing {
                    generation.generation_timing(timing);
                }
                (
                    budget
                        .is_some_and(|budget| budget.admitted < budget.proposed)
                        .then_some(Event::GenerationContextLimit),
                    true,
                )
            }
            Ok(GenerationAttempt::WithoutDraft(GenerationStop::ContextLimit)) => {
                (Some(Event::GenerationContextLimit), false)
            }
            Ok(GenerationAttempt::WithoutDraft(GenerationStop::Deadline))
            | Err(GenerationError::Deadline) => (Some(Event::GenerationDeadline), false),
            Ok(GenerationAttempt::WithoutDraft(GenerationStop::WorkerUnavailable))
            | Err(GenerationError::Runtime) => (Some(Event::GenerationWorkerUnavailable), false),
            Ok(GenerationAttempt::WithoutDraft(GenerationStop::InvalidOutput))
            | Err(GenerationError::Output) => (Some(Event::GenerationInvalidOutput), false),
            Err(GenerationError::Busy) => (Some(Event::GenerationWorkerBusy), false),
            Err(GenerationError::Cancelled) => (Some(Event::GenerationCancelled), false),
            Err(_) => (None, false),
        };
        if let Some(event) = event {
            generation.event(event);
        }
        generation.finish(success);
    }
    Ok(draft?)
}

fn verified_quotes<'a>(sources: &'a [Excerpt], draft: &'a Draft) -> Option<Vec<VerifiedQuote<'a>>> {
    verified_quotes_with_breaks(sources, draft, &[])
}

fn verified_quotes_with_breaks<'a>(
    sources: &'a [Excerpt],
    draft: &'a Draft,
    repaired: &[Option<std::ops::Range<usize>>],
) -> Option<Vec<VerifiedQuote<'a>>> {
    let mut used: Vec<VerifiedQuote<'_>> = Vec::new();
    for (index, reference) in draft.citations.iter().enumerate() {
        let source = sources.get(reference.passage.checked_sub(1)?)?;
        let quote = reference.quote.trim();
        if quote.len() < MIN_QUOTE_BYTES {
            return None;
        }
        let candidates = if let Some(literal) = VerifiedQuote::literal(source, quote) {
            vec![literal.with_context()]
        } else if let Some(line) = markdown_quotes::reopen_line(source, quote) {
            vec![line]
        } else if let Some(corrected) = VerifiedQuote::recover_passage(sources, source, quote) {
            vec![corrected.with_context()]
        } else if let Some(paragraph) = code_quotes::reopen_rust_doc_paragraph(source, quote) {
            vec![paragraph]
        } else if let Some(wrapped) = code_quotes::reopen_wrapped(sources, source, quote) {
            wrapped
                .into_iter()
                .map(VerifiedQuote::with_context)
                .collect()
        } else {
            vec![VerifiedQuote::exact_span(
                source,
                repaired.get(index)?.as_ref()?.clone(),
            )?]
        };
        for quote in candidates {
            if !used.iter().any(|other| {
                other.text == quote.text
                    && other
                        .excerpt
                        .evidence
                        .same_evidence(&quote.excerpt.evidence)
            }) {
                used.push(quote);
            }
        }
        if used.len() > MAX_PASSAGE_CITATIONS {
            return None;
        }
    }
    Some(used)
}

const fn answer_language(language: AnswerLanguage) -> &'static str {
    language.instruction()
}
