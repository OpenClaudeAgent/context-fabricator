use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{SpanOutcome, SpanTiming};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientKind {
    Desktop,
    Cli,
    #[default]
    Other,
}

impl ClientKind {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Cli => "cli",
            Self::Other => "other",
        }
    }

    pub(crate) fn from_code(code: &str) -> Option<Self> {
        match code {
            "desktop" => Some(Self::Desktop),
            "cli" => Some(Self::Cli),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    first_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_at: Option<i64>,
    groups: Vec<Group>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Group {
    client: ClientKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_profile: Option<String>,
    published: usize,
    abstained: usize,
    cancelled: usize,
    failed: usize,
    unknown: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    p50_micros: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    p95_micros: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_trace_id: Option<String>,
}

pub(crate) struct Record {
    pub timestamp: i64,
    pub duration_micros: Option<i64>,
    pub trace_id: Option<String>,
    pub timing: Option<SpanTiming>,
}

impl AnswerUsage {
    // Records arrive newest first, ordered by the telemetry store's insertion ID.
    pub(crate) fn from_records(records: impl IntoIterator<Item = Record>) -> Self {
        let mut usage = Self::default();
        let mut groups: BTreeMap<_, (Group, Vec<_>)> = BTreeMap::new();
        for record in records {
            let timing = record.timing.as_ref();
            let status = timing.and_then(|timing| timing.http_status);
            let client = timing
                .and_then(|timing| timing.client_kind)
                .unwrap_or_default();
            let model_profile = timing.and_then(|timing| timing.model_profile.clone());
            let (group, durations) = groups
                .entry((client, model_profile.clone()))
                .or_insert_with(|| {
                    (
                        Group {
                            client,
                            model_profile,
                            ..Group::default()
                        },
                        Vec::new(),
                    )
                });
            if group.latest_trace_id.is_none() {
                group.latest_trace_id = record.trace_id;
            }
            usage.first_at = Some(
                usage
                    .first_at
                    .map_or(record.timestamp, |first| first.min(record.timestamp)),
            );
            usage.last_at = Some(
                usage
                    .last_at
                    .map_or(record.timestamp, |last| last.max(record.timestamp)),
            );
            match timing.and_then(SpanOutcome::answer) {
                Some(SpanOutcome::Succeeded) => {
                    group.published += 1;
                    durations.extend(record.duration_micros.filter(|micros| *micros >= 0));
                }
                Some(SpanOutcome::InsufficientEvidence) => group.abstained += 1,
                Some(SpanOutcome::Cancelled) => group.cancelled += 1,
                Some(SpanOutcome::Failed) => group.failed += 1,
                _ if status.is_some_and(|status| status >= 400) => group.failed += 1,
                _ => group.unknown += 1,
            }
        }
        usage.groups = groups
            .into_values()
            .map(|(mut group, mut durations)| {
                durations.sort_unstable();
                let nearest_rank = |percentile: usize| {
                    (durations.len() * percentile)
                        .div_ceil(100)
                        .checked_sub(1)
                        .and_then(|index| durations.get(index).copied())
                };
                group.p50_micros = nearest_rank(50);
                group.p95_micros = nearest_rank(95);
                group
            })
            .collect();
        usage
    }
}
