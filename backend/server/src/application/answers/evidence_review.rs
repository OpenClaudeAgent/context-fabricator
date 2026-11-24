//! Transient evidence for explaining why a local answer was withheld.

use ctxfab_inference::generation::{Draft, GenerationStop};
use serde::Serialize;

use super::{SearchHit, SearchQuery};
use crate::application::{Location, retrieval::EmbeddingCapture, telemetry::FusionRanking};

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Reason {
    NoCandidates,
    NoUsableCandidates,
    ContextLimit,
    GenerationDeadline,
    WorkerUnavailable,
    InvalidModelOutput,
    NoCitations,
    CitationsUnverified,
    EvidenceChanged,
    Verified,
}

impl From<GenerationStop> for Reason {
    fn from(stop: GenerationStop) -> Self {
        match stop {
            GenerationStop::ContextLimit => Self::ContextLimit,
            GenerationStop::Deadline => Self::GenerationDeadline,
            GenerationStop::WorkerUnavailable => Self::WorkerUnavailable,
            GenerationStop::InvalidOutput => Self::InvalidModelOutput,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Source {
    LocalPassages,
    #[serde(rename = "githubHistory")]
    GitHubHistory,
    #[serde(rename = "githubActions")]
    GitHubActions,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum Disposition {
    CandidateLimit,
    PassageUnavailable,
    FixtureExcluded,
    ContextLimit,
    NoUsableDraft,
    NotCited,
    CitationUnverified,
    EvidenceChanged,
    CitationVerified,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Search {
    source: Source,
    candidate_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Scope {
    repository_id: i64,
    branch: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Candidate {
    source: Source,
    reference: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rank: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    slot: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fusion_rank: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_added: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<Location>,
    disposition: Disposition,
}

impl Candidate {
    pub(super) fn new(source: Source, reference: String, revision: Option<String>) -> Self {
        Self {
            source,
            reference,
            revision,
            start_line: None,
            end_line: None,
            rank: None,
            slot: None,
            preview: None,
            fusion_rank: None,
            context_added: None,
            text: None,
            location: None,
            disposition: Disposition::CandidateLimit,
        }
    }

    pub(super) fn passage(hit: &SearchHit, rank: Option<usize>) -> Self {
        let location = hit.location.as_ref();
        let reference = location.map_or(&hit.source, |location| &location.path);
        let mut candidate = Self::new(
            Source::LocalPassages,
            reference.clone(),
            Some(hit.revision.clone()),
        );
        candidate.rank = rank;
        candidate.start_line = location.map(|location| location.start_line);
        candidate.end_line = location.map(|location| location.end_line);
        candidate
    }

    pub(super) fn mark(&mut self, disposition: Disposition) {
        self.disposition = disposition;
    }

    pub(super) fn preview(&mut self, text: &str) {
        self.preview = Some(text.trim().chars().take(180).collect());
    }

    pub(super) fn propose(&mut self, slot: usize) {
        self.slot = Some(slot);
    }

    pub(super) fn mark_context_added(&mut self) {
        self.context_added = Some(true);
    }

    pub(super) fn capture_source(&mut self, hit: &SearchHit, text: String) {
        self.text = Some(text);
        self.location.clone_from(&hit.location);
    }
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Pipeline {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) fusion: Option<FusionRanking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) embedding: Option<EmbeddingCapture>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) fallback: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) completion_input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) draft: Option<Draft>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceReview {
    question: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<Scope>,
    searches: Vec<Search>,
    candidates: Vec<Candidate>,
    reason: Reason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pipeline: Option<Pipeline>,
}

impl EvidenceReview {
    pub(super) fn new(query: &SearchQuery) -> Self {
        Self {
            question: query.original().to_owned(),
            scope: query.scope().map(|scope| Scope {
                repository_id: scope.repository_id,
                branch: scope.branch.clone(),
            }),
            searches: Vec::new(),
            candidates: Vec::new(),
            reason: Reason::NoCandidates,
            pipeline: None,
        }
    }

    pub(super) fn search(
        &mut self,
        source: Source,
        candidate_count: usize,
        selection: Option<String>,
    ) {
        self.searches.push(Search {
            source,
            candidate_count,
            selection,
        });
        if candidate_count > 0 {
            self.reason = Reason::NoUsableCandidates;
        }
    }

    pub(super) fn add(&mut self, candidate: Candidate) {
        self.candidates.push(candidate);
    }

    pub(super) fn extend(&mut self, candidates: impl IntoIterator<Item = Candidate>) {
        self.candidates.extend(candidates);
    }

    pub(super) fn stopped(&mut self, stop: GenerationStop) {
        self.reason = stop.into();
        for candidate in self
            .candidates
            .iter_mut()
            .filter(|candidate| candidate.slot.is_some())
        {
            candidate.disposition = if stop == GenerationStop::ContextLimit {
                Disposition::ContextLimit
            } else {
                Disposition::NoUsableDraft
            };
        }
    }

    pub(super) fn invalidated(&mut self) {
        self.reason = Reason::EvidenceChanged;
        for candidate in self
            .candidates
            .iter_mut()
            .filter(|candidate| candidate.slot.is_some())
        {
            candidate.disposition = Disposition::EvidenceChanged;
        }
    }

    pub(super) fn reorder_slots(&mut self, previous_indices: &[usize]) {
        for candidate in &mut self.candidates {
            if let Some(slot) = candidate.slot {
                candidate.slot = previous_indices
                    .iter()
                    .position(|&index| index + 1 == slot)
                    .map(|i| i + 1);
                if candidate.slot.is_none() {
                    candidate.mark(Disposition::ContextLimit);
                }
            }
        }
    }

    pub(super) const fn capturing(&self) -> bool {
        self.pipeline.is_some()
    }

    pub(super) fn verified(&mut self, admitted: usize, draft: &Draft) {
        self.reason = Reason::Verified;
        for candidate in &mut self.candidates {
            let Some(slot) = candidate.slot else { continue };
            candidate.disposition = if slot > admitted {
                Disposition::ContextLimit
            } else if draft
                .citations
                .iter()
                .any(|reference| reference.passage == slot)
            {
                Disposition::CitationVerified
            } else {
                Disposition::NotCited
            };
        }
    }

    pub(super) fn unverified(
        &mut self,
        admitted: usize,
        cited: &(impl Iterator<Item = usize> + Clone),
    ) {
        self.reason = if cited.clone().next().is_none() {
            Reason::NoCitations
        } else {
            Reason::CitationsUnverified
        };
        for candidate in &mut self.candidates {
            let Some(slot) = candidate.slot else { continue };
            candidate.disposition = if slot > admitted {
                Disposition::ContextLimit
            } else if cited.clone().any(|reference| reference == slot) {
                Disposition::CitationUnverified
            } else {
                Disposition::NotCited
            };
        }
    }
}

pub(super) struct RankedCandidate {
    pub(super) hit: SearchHit,
    pub(super) review: Candidate,
}

impl RankedCandidate {
    pub(super) fn set_fusion_rank(&mut self, rank: Option<usize>) {
        self.review.fusion_rank = rank;
    }

    pub(super) fn set_text(&mut self, text: String) {
        self.review.capture_source(&self.hit, text);
    }

    pub(super) fn new(hit: &SearchHit, rank: usize) -> Self {
        Self {
            hit: hit.clone(),
            review: Candidate::passage(hit, Some(rank)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compacted_candidates_report_budget_exclusion_and_keep_reordered_slots()
    -> Result<(), Box<dyn std::error::Error>> {
        let query = SearchQuery::parse("Explain the source behavior")?;
        let mut review = EvidenceReview::new(&query);
        for slot in [Some(1), Some(2), Some(3), None] {
            let mut candidate = Candidate::new(Source::LocalPassages, "source".into(), None);
            if let Some(slot) = slot {
                candidate.propose(slot);
            }
            review.add(candidate);
        }

        review.reorder_slots(&[2, 0]);
        review.unverified(2, &[1].iter().copied());

        let json = serde_json::to_value(review)?;
        let candidates = json["candidates"].as_array().ok_or("missing candidates")?;
        assert_eq!(candidates[0]["slot"], 2);
        assert_eq!(candidates[0]["disposition"], "notCited");
        assert!(candidates[1].get("slot").is_none());
        assert_eq!(candidates[1]["disposition"], "contextLimit");
        assert_eq!(candidates[2]["slot"], 1);
        assert_eq!(candidates[2]["disposition"], "citationUnverified");
        assert!(candidates[3].get("slot").is_none());
        assert_eq!(candidates[3]["disposition"], "candidateLimit");
        Ok(())
    }
}
