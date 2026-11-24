use opentelemetry::{KeyValue, Value};

use crate::application::{
    retrieval::MAX_RESULTS,
    telemetry::{FusionRanking, RetrievalCounts},
};

const KEY: &str = "ctxfab.retrieval.fusion_ranking";
const MAX_BYTES: usize = 8192;
const PASSAGE_CANDIDATES: &str = "ctxfab.retrieval.passage_candidates";
const HISTORY_PAIRS: &str = "ctxfab.retrieval.history_pairs";
const WORKFLOW_ATTEMPTS: &str = "ctxfab.retrieval.workflow_attempts";
const PROPOSED_SOURCES: &str = "ctxfab.retrieval.proposed_sources";

pub(super) fn attribute(ranking: &FusionRanking) -> Option<KeyValue> {
    let encoded = serde_json::to_string(ranking).ok()?;
    (encoded.len() <= MAX_BYTES).then(|| KeyValue::new(KEY, encoded))
}

pub(super) fn read(attributes: &[KeyValue]) -> Option<FusionRanking> {
    let value = attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == KEY)?;
    let Value::String(encoded) = &value.value else {
        return None;
    };
    if encoded.as_str().len() > MAX_BYTES {
        return None;
    }
    let ranking: FusionRanking = serde_json::from_str(encoded.as_str()).ok()?;
    (ranking.candidates.len() <= 2 * MAX_RESULTS).then_some(ranking)
}

pub(super) fn count_attributes(counts: RetrievalCounts) -> impl Iterator<Item = KeyValue> {
    [
        (PASSAGE_CANDIDATES, counts.passage_candidates),
        (HISTORY_PAIRS, counts.history_pairs),
        (WORKFLOW_ATTEMPTS, counts.workflow_attempts),
        (PROPOSED_SOURCES, Some(counts.proposed_sources)),
    ]
    .into_iter()
    .filter_map(|(key, count)| count.map(|count| KeyValue::new(key, i64::from(count))))
}

pub(super) fn read_counts(attributes: &[KeyValue]) -> Option<RetrievalCounts> {
    let count = |key| {
        attributes.iter().find_map(|attribute| {
            if attribute.key.as_str() != key {
                return None;
            }
            match attribute.value {
                Value::I64(count) => u16::try_from(count).ok(),
                _ => None,
            }
        })
    };
    Some(RetrievalCounts {
        passage_candidates: count(PASSAGE_CANDIDATES),
        history_pairs: count(HISTORY_PAIRS),
        workflow_attempts: count(WORKFLOW_ATTEMPTS),
        proposed_sources: count(PROPOSED_SOURCES)?,
    })
}
