use serde::{Deserialize, Serialize};

use super::{Failure, Settings};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    Startup,
    Shutdown,
    ApiQuery,
    LexicalLookup,
    VectorRetrieval,
    EmbeddingRebuild,
    ApiAnswer,
    AnswerRetrieval,
    AnswerGeneration,
    AnswerValidation,
    AnswerCancellation,
    GenerationSelection,
    ApiImport,
    Collection,
    Index,
    GitHubCurrentCode,
}

impl Stage {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Startup => "backend_startup",
            Self::Shutdown => "backend_shutdown",
            Self::ApiQuery => "api_query",
            Self::LexicalLookup => "lexical_lookup",
            Self::VectorRetrieval => "vector_retrieval",
            Self::EmbeddingRebuild => "embedding_rebuild",
            Self::ApiAnswer => "api_answer",
            Self::AnswerRetrieval => "answer_retrieval",
            Self::AnswerGeneration => "answer_generation",
            Self::AnswerValidation => "answer_validation",
            Self::AnswerCancellation => "answer_cancellation",
            Self::GenerationSelection => "generation_selection",
            Self::ApiImport => "api_import",
            Self::Collection => "scheduler_collection",
            Self::Index => "index_commit",
            Self::GitHubCurrentCode => "github_current_code",
        }
    }

    pub const ALL: [Self; 16] = [
        Self::Startup,
        Self::Shutdown,
        Self::ApiQuery,
        Self::LexicalLookup,
        Self::VectorRetrieval,
        Self::EmbeddingRebuild,
        Self::ApiAnswer,
        Self::AnswerRetrieval,
        Self::AnswerGeneration,
        Self::AnswerValidation,
        Self::AnswerCancellation,
        Self::GenerationSelection,
        Self::ApiImport,
        Self::Collection,
        Self::Index,
        Self::GitHubCurrentCode,
    ];

    pub(crate) fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|stage| stage.code() == code)
    }

    pub(crate) const fn descriptor(self) -> (&'static str, &'static str) {
        match self {
            Self::Startup => ("Start backend", "backend.start"),
            Self::Shutdown => ("Stop backend", "backend.stop"),
            Self::ApiQuery => ("Search request", "GET /v1/search"),
            Self::LexicalLookup => ("Look up keywords", "search.bm25"),
            Self::VectorRetrieval => ("Retrieve local results", "search.retrieval"),
            Self::EmbeddingRebuild => ("Rebuild embeddings", "POST /v1/retrieval/rebuild"),
            Self::ApiAnswer => ("Answer request", "POST /v1/answers"),
            Self::AnswerRetrieval => ("Retrieve evidence", "answer.retrieve"),
            Self::AnswerGeneration => ("Generate answer", "inference.generate"),
            Self::AnswerValidation => ("Verify citations", "answer.verify_citations"),
            Self::AnswerCancellation => ("Cancel answer", "DELETE /v1/answers/{requestId}"),
            Self::GenerationSelection => ("Switch generation model", "PUT /v1/generation/models"),
            Self::ApiImport => ("Import fixtures", "POST /v1/fixtures/import"),
            Self::Collection => ("Collect changes", "scheduler.collect"),
            Self::Index => ("Commit index update", "index.commit"),
            Self::GitHubCurrentCode => (
                "Refresh GitHub current code",
                "POST /v1/github/repositories/{id}/refresh",
            ),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Event {
    SearchRejected,
    Bm25Selected,
    HybridSelected,
    SemanticSelected,
    Bm25Fallback,
    NoEvidence,
    GenerationCancelled,
    CitationsRejected,
    CitationsVerified,
}

impl Event {
    pub(crate) const MAX_PER_SPAN: u32 = 8;

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::SearchRejected => "Invalid search request",
            Self::Bm25Selected => "BM25 lookup selected",
            Self::HybridSelected => "Hybrid lookup selected",
            Self::SemanticSelected => "Semantic lookup selected",
            Self::Bm25Fallback => "Fell back to BM25",
            Self::NoEvidence => "No evidence found",
            Self::GenerationCancelled => "Generation cancelled",
            Self::CitationsRejected => "Citations could not be verified",
            Self::CitationsVerified => "Citations verified",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        [
            Self::SearchRejected,
            Self::Bm25Selected,
            Self::HybridSelected,
            Self::SemanticSelected,
            Self::Bm25Fallback,
            Self::NoEvidence,
            Self::GenerationCancelled,
            Self::CitationsRejected,
            Self::CitationsVerified,
        ]
        .into_iter()
        .find(|event| event.name() == name)
    }
}

#[derive(Clone, Copy)]
pub struct TraceLink {
    pub trace: [u8; 16],
    pub span: [u8; 8],
}

pub trait Observation: Send {
    fn link(&self) -> TraceLink;
    fn event(&mut self, event: Event);
    fn finish(self: Box<Self>, success: bool);
}

pub trait Observer: Send + Sync {
    fn start(&self, stage: Stage, parent: Option<TraceLink>) -> Box<dyn Observation>;
}

pub trait Telemetry: Observer {
    fn snapshot(&self) -> Snapshot;

    /// # Errors
    /// Returns a storage error if the retained log page cannot be read.
    fn logs(&self, filter: &LogFilter) -> Result<LogPage, Failure>;

    /// # Errors
    /// Returns a storage error if the retained trace page cannot be read.
    fn trace(&self, trace_id: &str, before_id: Option<i64>) -> Result<TracePage, Failure>;

    /// # Errors
    /// Returns a storage error when maintenance cannot complete.
    fn command(&self, command: Command) -> Result<Snapshot, Failure>;

    fn apply_settings(&self, settings: &Settings);
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogFilter {
    pub query: Option<String>,
    pub level: Option<LogLevel>,
    pub before_id: Option<i64>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Error,
}

impl LogLevel {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPage {
    pub records: Vec<Sample>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_before_id: Option<i64>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TracePage {
    pub spans: Vec<Sample>,
    pub logs: Vec<Sample>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_before_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Command {
    EnableDebug,
    DisableDebug,
    PurgeExpired,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub logs: Vec<Sample>,
    pub metrics: Vec<Sample>,
    pub traces: Vec<Sample>,
    pub health: Health,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    pub id: i64,
    pub timestamp: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<String>,
    #[serde(flatten)]
    pub span_details: Option<SpanDetails>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpanDetails {
    pub display_name: &'static str,
    pub operation: &'static str,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub timing: Option<SpanTiming>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpanTiming {
    pub started_at_unix_micros: i64,
    pub events: Vec<SpanEvent>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpanEvent {
    pub name: String,
    pub offset_micros: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub state: &'static str,
    pub dropped: u64,
    pub pruned: u64,
    pub queued: usize,
    pub file_bytes: u64,
    pub budget_bytes: u64,
    pub retention_hours: u16,
    pub debug_until: i64,
    pub oldest_at: Option<i64>,
}
