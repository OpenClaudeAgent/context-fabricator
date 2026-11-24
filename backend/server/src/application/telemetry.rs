use ctxfab_inference::generation::{ContextBudget, GenerationTiming};
use serde::{Deserialize, Serialize};

mod activity;
mod answer_usage;
pub use activity::{SpanOutcome, TraceRequestKind};
pub(crate) use answer_usage::Record as AnswerUsageRecord;
pub use answer_usage::{AnswerUsage, ClientKind};

use super::{Failure, Settings};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    Startup,
    Shutdown,
    ApiRequest,
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
    GitHubCurrentJob,
    GitHubCurrentCollection,
    GitHubProviderRequest,
    GitHubHistory,
    GitHubHistoryJob,
    GitHubHistoryCollection,
    GitHubHistoryPreparation,
    GitHubActions,
    GitHubActionsCollection,
}

impl Stage {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Startup => "backend_startup",
            Self::Shutdown => "backend_shutdown",
            Self::ApiRequest => "api_request",
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
            Self::GitHubCurrentJob => "github_current_job",
            Self::GitHubCurrentCollection => "github_current_collection",
            Self::GitHubProviderRequest => "github_provider_request",
            Self::GitHubHistory => "github_history",
            Self::GitHubHistoryJob => "github_history_job",
            Self::GitHubHistoryCollection => "github_history_collection",
            Self::GitHubHistoryPreparation => "github_history_preparation",
            Self::GitHubActions => "github_actions",
            Self::GitHubActionsCollection => "github_actions_collection",
        }
    }

    pub const ALL: [Self; 26] = [
        Self::Startup,
        Self::Shutdown,
        Self::ApiRequest,
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
        Self::GitHubCurrentJob,
        Self::GitHubCurrentCollection,
        Self::GitHubProviderRequest,
        Self::GitHubHistory,
        Self::GitHubHistoryJob,
        Self::GitHubHistoryCollection,
        Self::GitHubHistoryPreparation,
        Self::GitHubActions,
        Self::GitHubActionsCollection,
    ];

    pub(crate) fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|stage| stage.code() == code)
    }

    pub(crate) const fn descriptor(self) -> (&'static str, &'static str) {
        match self {
            Self::Startup => ("Start backend", "backend.start"),
            Self::Shutdown => ("Stop backend", "backend.stop"),
            Self::ApiRequest => ("Local API request", "http.request"),
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
            Self::GitHubCurrentJob => ("Scheduled GitHub code job", "github.current.job"),
            Self::GitHubCurrentCollection => {
                ("Collect GitHub code and graph", "github.current.collect")
            }
            Self::GitHubProviderRequest => ("GitHub request", "github.http"),
            Self::GitHubHistory => (
                "Refresh GitHub history",
                "POST /v1/github/repositories/{id}/history",
            ),
            Self::GitHubHistoryJob => ("Scheduled GitHub history job", "github.history.job"),
            Self::GitHubHistoryCollection => ("Collect GitHub history", "github.history.collect"),
            Self::GitHubHistoryPreparation => {
                ("Prepare GitHub change evidence", "github.history.prepare")
            }
            Self::GitHubActions => (
                "Refresh GitHub Actions",
                "POST /v1/github/repositories/{id}/actions",
            ),
            Self::GitHubActionsCollection => ("Collect GitHub Actions", "github.actions.collect"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Event {
    SearchRejected,
    Bm25Selected,
    HybridSelected,
    SearchFormulationRanked,
    RustGraphPassagesSelected,
    ExactJavaMethodPassagesSelected,
    NamedJavaClassPassagesSelected,
    RootReadmeAuthoritySelected,
    SemanticSelected,
    Bm25Fallback,
    EmbeddingIndexDisabled,
    EmbeddingIndexIncomplete,
    EmbeddingWorkerUnavailable,
    EmbeddingWorkerBusy,
    HistoryPairsSelected,
    ActionsAttemptSelected,
    LatestActionsRunSelected,
    HistoryPairNotCited,
    HistoryPairChanged,
    HistoryQuoteRejected,
    NoEvidence,
    CollectionCooldown,
    CollectionRetry,
    CollectionPublished,
    CollectionUnchanged,
    CollectionScopeChanged,
    ProviderRequestDeadline,
    AnswerPublished,
    AnswerAbstained,
    AnswerCancelled,
    AnswerFailed,
    GenerationCancelled,
    GenerationContextLimit,
    GenerationDeadline,
    GenerationWorkerUnavailable,
    GenerationWorkerBusy,
    GenerationInvalidOutput,
    CitationsRejected,
    CitationsVerified,
    LinkedEvidenceVerified,
}

impl Event {
    pub(crate) const MAX_PER_SPAN: u32 = 8;

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::SearchRejected => "Invalid search request",
            Self::Bm25Selected => "BM25 lookup selected",
            Self::HybridSelected => "Hybrid lookup selected",
            Self::RustGraphPassagesSelected => "Rust graph passages selected",
            Self::RootReadmeAuthoritySelected => "Published root README searched",
            Self::NamedJavaClassPassagesSelected => "Named Java class passages selected",
            Self::ExactJavaMethodPassagesSelected => "Exact Java method passages selected",
            Self::SearchFormulationRanked => "Auxiliary search formulation ranked",
            Self::SemanticSelected => "Semantic lookup selected",
            Self::Bm25Fallback => "Fell back to BM25",
            Self::EmbeddingIndexDisabled => "Semantic index disabled",
            Self::EmbeddingIndexIncomplete => "Semantic index incomplete",
            Self::EmbeddingWorkerUnavailable => "Embedding worker unavailable",
            Self::EmbeddingWorkerBusy => "Embedding worker busy",
            Self::HistoryPairsSelected => "Paired history lookup selected",
            Self::ActionsAttemptSelected => "Selected the requested workflow run attempt",
            Self::LatestActionsRunSelected => "Latest recorded workflow run selected",
            Self::HistoryPairNotCited => "Draft did not select a supported history pair",
            Self::HistoryPairChanged => "History pair was unavailable during verification",
            Self::HistoryQuoteRejected => {
                "Selected lines could not be located in both history sides"
            }
            Self::NoEvidence => "No evidence found",
            Self::CollectionCooldown => "Collection waiting on provider cooldown",
            Self::CollectionRetry => "Collection retry scheduled",
            Self::CollectionPublished => "Collection publication updated",
            Self::CollectionUnchanged => "Collection publication already current",
            Self::CollectionScopeChanged => "Collection scope changed before job completion",
            Self::ProviderRequestDeadline => "Provider request deadline expired",
            Self::AnswerPublished => "Cited answer published",
            Self::AnswerAbstained => "Answer withheld for insufficient evidence",
            Self::AnswerCancelled => "Answer request cancelled",
            Self::AnswerFailed => "Answer request failed",
            Self::GenerationCancelled => "Generation cancelled",
            Self::GenerationContextLimit => "Generation context limit applied",
            Self::GenerationDeadline => "Generation deadline expired",
            Self::GenerationWorkerUnavailable => "Generation worker unavailable",
            Self::GenerationWorkerBusy => "Generation worker busy",
            Self::GenerationInvalidOutput => "Generation returned invalid output",
            Self::CitationsRejected => "Citations could not be verified",
            Self::CitationsVerified => "Citations verified",
            Self::LinkedEvidenceVerified => "Declaration and usage verified",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        [
            Self::SearchRejected,
            Self::Bm25Selected,
            Self::HybridSelected,
            Self::SearchFormulationRanked,
            Self::RustGraphPassagesSelected,
            Self::ExactJavaMethodPassagesSelected,
            Self::NamedJavaClassPassagesSelected,
            Self::RootReadmeAuthoritySelected,
            Self::SemanticSelected,
            Self::Bm25Fallback,
            Self::EmbeddingIndexDisabled,
            Self::EmbeddingIndexIncomplete,
            Self::EmbeddingWorkerUnavailable,
            Self::EmbeddingWorkerBusy,
            Self::HistoryPairsSelected,
            Self::ActionsAttemptSelected,
            Self::LatestActionsRunSelected,
            Self::HistoryPairNotCited,
            Self::HistoryPairChanged,
            Self::HistoryQuoteRejected,
            Self::NoEvidence,
            Self::CollectionCooldown,
            Self::CollectionRetry,
            Self::CollectionPublished,
            Self::CollectionUnchanged,
            Self::CollectionScopeChanged,
            Self::ProviderRequestDeadline,
            Self::AnswerPublished,
            Self::AnswerAbstained,
            Self::AnswerCancelled,
            Self::AnswerFailed,
            Self::GenerationCancelled,
            Self::GenerationContextLimit,
            Self::GenerationDeadline,
            Self::GenerationWorkerUnavailable,
            Self::GenerationWorkerBusy,
            Self::GenerationInvalidOutput,
            Self::CitationsRejected,
            Self::CitationsVerified,
            Self::LinkedEvidenceVerified,
        ]
        .into_iter()
        .find(|event| event.name() == name)
    }
}

pub use crate::domain::telemetry::TraceLink;

#[derive(Clone, Copy)]
pub struct TraceContext<'a> {
    pub observer: &'a dyn Observer,
    pub parent: TraceLink,
}

#[derive(Clone, Copy)]
pub struct TraceOrigin<'a> {
    pub observer: &'a dyn Observer,
    pub parent: Option<TraceLink>,
}

impl<'a> From<TraceContext<'a>> for TraceOrigin<'a> {
    fn from(context: TraceContext<'a>) -> Self {
        Self {
            observer: context.observer,
            parent: Some(context.parent),
        }
    }
}

impl<'a> TraceOrigin<'a> {
    pub(crate) fn start(self, stage: Stage) -> (TraceContext<'a>, Box<dyn Observation>) {
        let span = self.observer.start(stage, self.parent);
        (
            TraceContext {
                observer: self.observer,
                parent: span.link(),
            },
            span,
        )
    }
}

pub trait Observation: Send {
    fn link(&self) -> TraceLink;
    fn event(&mut self, event: Event);
    fn generation_timing(&mut self, timing: GenerationTiming);
    fn context_budget(&mut self, context: &GenerationContext);
    fn fusion_ranking(&mut self, ranking: &FusionRanking);
    fn retrieval_counts(&mut self, counts: RetrievalCounts);
    fn http_status(&mut self, status: u16);
    fn http_request(&mut self, method: &'static str, route: Option<&str>);
    fn client_kind(&mut self, client: ClientKind);
    fn model_profile(&mut self, profile: &str);
    fn problem_code(&mut self, code: &'static str);
    fn work_trace(&mut self, link: TraceLink);
    fn finish(self: Box<Self>, success: bool);
}

pub trait Observer: Send + Sync {
    fn start(&self, stage: Stage, parent: Option<TraceLink>) -> Box<dyn Observation>;
}

pub trait Telemetry: Observer {
    fn snapshot(&self) -> Snapshot;

    /// # Errors
    /// Returns a storage error if retained answer requests cannot be read.
    fn answer_usage(&self) -> Result<AnswerUsage, Failure>;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span_outcome: Option<SpanOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_request_kind: Option<TraceRequestKind>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub timing: Option<SpanTiming>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpanTiming {
    pub started_at_unix_micros: i64,
    pub events: Vec<SpanEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer_outcome: Option<SpanOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inference_timing: Option<GenerationTiming>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_budget: Option<GenerationContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fusion_ranking: Option<FusionRanking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrieval_counts: Option<RetrievalCounts>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_kind: Option<ClientKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_trace_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetrievalCounts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passage_candidates: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_pairs: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_attempts: Option<u16>,
    pub proposed_sources: u16,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GenerationEvidence {
    Passages,
    LinkedPassages,
    HistoryPairs,
    WorkflowAttempt,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GenerationContext {
    pub kind: GenerationEvidence,
    #[serde(flatten)]
    pub budget: ContextBudget,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FusionRanking {
    pub rank_offset: u16,
    pub dense_weight: f64,
    pub lexical_candidates: u16,
    pub dense_candidates: u16,
    pub candidates: Vec<FusionCandidate>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FusionCandidate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lexical_rank: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dense_rank: Option<u16>,
    pub score: f64,
    pub score_rank: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_rank: Option<u16>,
    pub lexical_promoted: bool,
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
