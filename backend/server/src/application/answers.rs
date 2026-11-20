use std::sync::Arc;

use ctxfab_inference::generation::{Draft, Generation, GenerationError, Models, Request};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use super::{
    AnswerLanguage, CodeScope, SearchHit, SearchQuery,
    ports::Repository,
    retrieval::{Mode, Retrieval, RetrievalError},
    telemetry::{Event, Observer, Stage, TraceLink},
};

const MAX_SOURCES: usize = 5;
const MIN_QUOTE_BYTES: usize = 8;

mod code_quotes;

pub struct Excerpt {
    pub evidence: SearchHit,
    pub text: String,
}

pub trait EvidenceStore: Send + Sync {
    /// Reads available passages at the requested provenance identities.
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Answer {
    pub outcome: &'static str,
    pub model: &'static str,
    pub mode: Mode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub citations: Vec<Citation>,
}

pub struct Answers {
    repository: Arc<dyn Repository>,
    evidence: Arc<dyn EvidenceStore>,
    retrieval: Option<Arc<Retrieval>>,
    generation: Arc<Generation>,
}

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
            generation,
        }
    }

    /// # Errors
    /// Returns a typed error if evidence or local generation is unavailable.
    pub fn ask(
        &self,
        query: &SearchQuery,
        id: Option<Uuid>,
        observer: Option<&dyn Observer>,
        trace: Option<TraceLink>,
    ) -> Result<Answer, AnswerError> {
        let request = self.generation.begin(id)?;
        let result = self.answer(query, &request, observer, trace);
        request.finish()?;
        result
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
    /// Returns an error if the request ID no longer belongs to the active job.
    pub fn cancel(&self, id: Uuid) -> Result<(), GenerationError> {
        self.generation.cancel(id)
    }

    fn answer(
        &self,
        query: &SearchQuery,
        request: &Request<'_>,
        observer: Option<&dyn Observer>,
        trace: Option<TraceLink>,
    ) -> Result<Answer, AnswerError> {
        let lookup = observer.map(|observer| observer.start(Stage::AnswerRetrieval, trace));
        let sources = self.retrieve(query);
        if let Some(mut lookup) = lookup {
            if let Ok((mode, sources)) = &sources {
                let plan = match mode {
                    Mode::Hybrid => Event::HybridSelected,
                    Mode::Bm25 if self.retrieval.is_some() => Event::Bm25Fallback,
                    Mode::Bm25 => Event::Bm25Selected,
                    Mode::Semantic => Event::SemanticSelected,
                };
                lookup.event(plan);
                if sources.is_empty() {
                    lookup.event(Event::NoEvidence);
                }
            }
            lookup.finish(sources.is_ok());
        }
        let (mode, sources) = sources?;
        let empty = Answer {
            outcome: "insufficientEvidence",
            model: request.model(),
            mode,
            text: None,
            citations: Vec::new(),
        };
        if sources.is_empty() {
            return Ok(empty);
        }

        let language = self
            .repository
            .settings()
            .map_err(|_| AnswerError::Storage)?
            .answer_language;
        let prompt = prompt(query, language, &sources);
        let generation = observer.map(|observer| observer.start(Stage::AnswerGeneration, trace));
        let draft = request.generate(&prompt);
        if let Some(mut generation) = generation {
            if matches!(draft, Err(GenerationError::Cancelled)) {
                generation.event(Event::GenerationCancelled);
            }
            generation.finish(draft.is_ok());
        }
        let draft = draft?;
        let verification = observer.map(|observer| observer.start(Stage::AnswerValidation, trace));
        let citations = self.validate(&sources, &draft, query.scope());
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
        let Some(citations) = citations? else {
            return Ok(empty);
        };
        Ok(Answer {
            outcome: "answered",
            text: Some(draft.answer.trim().to_owned()),
            citations,
            ..empty
        })
    }

    fn retrieve(&self, query: &SearchQuery) -> Result<(Mode, Vec<Excerpt>), AnswerError> {
        if let Some(retrieval) = &self.retrieval {
            match retrieval.search(query, Mode::Hybrid) {
                Ok(hits) => {
                    return self
                        .context(&hits, query.scope())
                        .map(|sources| (Mode::Hybrid, sources));
                }
                Err(
                    RetrievalError::Incomplete | RetrievalError::Runtime | RetrievalError::Busy,
                ) => {}
                Err(_) => return Err(AnswerError::Storage),
            }
        }
        let hits = self
            .repository
            .search(query)
            .map_err(|_| AnswerError::Storage)?;
        self.context(&hits, query.scope())
            .map(|sources| (Mode::Bm25, sources))
    }

    fn context(
        &self,
        hits: &[SearchHit],
        scope: Option<&CodeScope>,
    ) -> Result<Vec<Excerpt>, AnswerError> {
        let mut excerpts = self
            .evidence
            .excerpts(&hits[..hits.len().min(MAX_SOURCES)], scope)?;
        let anchors: Vec<_> = excerpts
            .iter()
            .take(2)
            .filter(|excerpt| excerpt.evidence.location.is_some())
            .map(|excerpt| excerpt.evidence.clone())
            .collect();
        if !anchors.is_empty() {
            let neighbors = self.evidence.following(&anchors, scope)?;
            let next = neighbors
                .into_iter()
                .filter(|neighbor| {
                    !excerpts
                        .iter()
                        .any(|excerpt| excerpt.evidence.same_evidence(&neighbor.evidence))
                })
                .collect::<Vec<_>>();
            let insertion = excerpts.len().min(2);
            excerpts.splice(insertion..insertion, next);
            excerpts.truncate(MAX_SOURCES);
        }
        Ok(excerpts)
    }

    fn validate(
        &self,
        sources: &[Excerpt],
        draft: &Draft,
        scope: Option<&CodeScope>,
    ) -> Result<Option<Vec<Citation>>, AnswerError> {
        if draft.answer.trim().is_empty()
            || draft.answer.len() > 2_048
            || draft.citations.is_empty()
            || draft.citations.len() > MAX_SOURCES
        {
            return Ok(None);
        }
        let Some(used) = verified_quotes(sources, draft) else {
            return Ok(None);
        };
        let hits: Vec<_> = used
            .iter()
            .map(|(source, _)| source.evidence.clone())
            .collect();
        // Recheck the exact source and text in a new snapshot after generation.
        let current = self.evidence.excerpts(&hits, scope)?;
        Ok(used
            .into_iter()
            .map(|(previous, quote)| {
                let found = current.iter().find(|excerpt| {
                    excerpt.evidence.same_evidence(&previous.evidence)
                        && excerpt.text == previous.text
                        && excerpt.text.contains(quote)
                })?;
                Some(Citation {
                    evidence: found.evidence.clone(),
                    quote: quote.to_owned(),
                })
            })
            .collect())
    }
}

fn verified_quotes<'a>(
    sources: &'a [Excerpt],
    draft: &'a Draft,
) -> Option<Vec<(&'a Excerpt, &'a str)>> {
    let mut used: Vec<(&Excerpt, &str)> = Vec::new();
    for reference in &draft.citations {
        let source = sources.get(reference.passage.checked_sub(1)?)?;
        let quote = reference.quote.trim();
        if quote.len() < MIN_QUOTE_BYTES {
            return None;
        }
        let candidates = if source.text.contains(quote) {
            vec![(source, quote)]
        } else {
            code_quotes::reopen_wrapped(sources, source, quote)?
        };
        for (excerpt, quote) in candidates {
            let quote = code_quotes::with_context(excerpt, quote);
            if !used.iter().any(|(other, text)| {
                *text == quote && other.evidence.same_evidence(&excerpt.evidence)
            }) {
                used.push((excerpt, quote));
            }
        }
        if used.len() > MAX_SOURCES {
            return None;
        }
    }
    Some(used)
}

fn prompt(query: &SearchQuery, language: AnswerLanguage, sources: &[Excerpt]) -> String {
    let language = match language {
        AnswerLanguage::Auto => "same language as the question",
        AnswerLanguage::En => "English",
        AnswerLanguage::Fr => "French",
    };
    let evidence: Vec<_> = sources.iter().enumerate().map(|(index, excerpt)| {
        let mut passage = json!({"id":index + 1,"kind":excerpt.evidence.kind,"text":excerpt.text});
        if let Some(location) = &excerpt.evidence.location {
            passage["source"] = json!({"path": location.path, "revision": excerpt.evidence.revision,
                "startLine": location.start_line, "endLine": location.end_line});
        }
        passage
    }).collect();
    json!({"question":query.original(),"answerLanguage":language,"passages":evidence})
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
}
