use serde::{Deserialize, Serialize};

use super::{Event, SpanTiming, Stage};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SpanOutcome {
    Succeeded,
    Failed,
    InsufficientEvidence,
    Cancelled,
    Unknown,
}

impl SpanOutcome {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::InsufficientEvidence => "insufficientEvidence",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn from_code(code: &str) -> Option<Self> {
        [
            Self::Succeeded,
            Self::Failed,
            Self::InsufficientEvidence,
            Self::Cancelled,
            Self::Unknown,
        ]
        .into_iter()
        .find(|outcome| outcome.code() == code)
    }

    pub(crate) const fn from_event(event: Event) -> Option<Self> {
        match event {
            Event::AnswerPublished => Some(Self::Succeeded),
            Event::AnswerFailed => Some(Self::Failed),
            Event::AnswerAbstained => Some(Self::InsufficientEvidence),
            Event::AnswerCancelled => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub(crate) fn answer(timing: &SpanTiming) -> Option<Self> {
        timing.answer_outcome.or_else(|| {
            // Preserve these historical labels independently of future display wording.
            timing
                .events
                .iter()
                .rev()
                .find_map(|event| match event.name.as_str() {
                    "Cited answer published" => Some(Self::Succeeded),
                    "Answer request failed" => Some(Self::Failed),
                    "Answer withheld for insufficient evidence" => Some(Self::InsufficientEvidence),
                    "Answer request cancelled" => Some(Self::Cancelled),
                    _ => None,
                })
        })
    }

    pub(crate) fn classify(stage: Stage, level: Option<&str>, timing: Option<&SpanTiming>) -> Self {
        if stage == Stage::ApiAnswer
            && let Some(outcome) = timing.and_then(Self::answer)
        {
            return outcome;
        }
        match level {
            Some("ok") => Self::Succeeded,
            Some("error") => Self::Failed,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TraceRequestKind {
    BackgroundCheck,
}

impl TraceRequestKind {
    pub(crate) fn classify(
        stage: Stage,
        level: Option<&str>,
        timing: Option<&SpanTiming>,
    ) -> Option<Self> {
        let timing = timing?;
        (stage == Stage::ApiRequest
            && level == Some("ok")
            && timing.http_method.as_deref() == Some("GET")
            && matches!(
                timing.http_route.as_deref(),
                Some(
                    "/v1/status"
                        | "/v1/settings"
                        | "/v1/diagnostics"
                        | "/v1/github"
                        | "/v1/generation/models"
                        | "/v1/inference/setup"
                        | "/v1/inference/storage"
                        | "/v1/runtime"
                        | "/v1/retrieval/index"
                        | "/v1/retrieval"
                        | "/v1/models/downloads"
                )
            ))
        .then_some(Self::BackgroundCheck)
    }
}
