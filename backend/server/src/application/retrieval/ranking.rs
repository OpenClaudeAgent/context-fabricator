use super::MAX_RESULTS;
use crate::application::{
    SearchHit, SearchQuery,
    telemetry::{FusionCandidate, FusionRanking},
};

const PROSE_DENSE_WEIGHT: f64 = 1.25;
const LEXICAL_CUTOFF: usize = 5;
const RANK_OFFSET: u16 = 60;

#[derive(Clone, Copy)]
enum Channel {
    Lexical,
    Dense,
}

struct Candidate {
    hit: SearchHit,
    lexical_rank: Option<u16>,
    dense_rank: Option<u16>,
    score: f64,
    score_rank: u16,
    lexical_promoted: bool,
}

impl Candidate {
    fn record(&mut self, channel: Channel, rank: u16, weight: f64) {
        match channel {
            Channel::Lexical => self.lexical_rank.get_or_insert(rank),
            Channel::Dense => self.dense_rank.get_or_insert(rank),
        };
        self.score += weight / f64::from(u32::from(RANK_OFFSET) + u32::from(rank));
    }
}

pub(super) fn fuse(
    query: &SearchQuery,
    lexical: Vec<SearchHit>,
    dense: Vec<SearchHit>,
    observe: bool,
) -> (Vec<SearchHit>, Option<FusionRanking>) {
    let prose = query.is_scoped_prose();
    let dense_weight = if prose { PROSE_DENSE_WEIGHT } else { 1.0 };
    let lexical_candidates = count(lexical.len());
    let dense_candidates = count(dense.len());
    let mut candidates: Vec<Candidate> = Vec::new();
    for (channel, branch, weight) in [
        (Channel::Lexical, lexical, 1.0),
        (Channel::Dense, dense, dense_weight),
    ] {
        for (rank, hit) in (1_u16..).zip(branch) {
            if let Some(candidate) = candidates
                .iter_mut()
                .find(|candidate| candidate.hit.same_evidence(&hit))
            {
                candidate.record(channel, rank, weight);
            } else {
                let mut candidate = Candidate {
                    hit,
                    lexical_rank: None,
                    dense_rank: None,
                    score: 0.0,
                    score_rank: 0,
                    lexical_promoted: false,
                };
                candidate.record(channel, rank, weight);
                candidates.push(candidate);
            }
        }
    }
    // The stable sort preserves lexical order when the combined scores tie.
    candidates.sort_by(|left, right| right.score.total_cmp(&left.score));
    for (index, candidate) in candidates.iter_mut().enumerate() {
        candidate.score_rank = count(index + 1);
    }
    if prose
        && let Some(position) = candidates
            .iter()
            .position(|candidate| candidate.lexical_rank == Some(1))
        && position >= LEXICAL_CUTOFF
    {
        let mut candidate = candidates.remove(position);
        candidate.lexical_promoted = true;
        candidates.insert(LEXICAL_CUTOFF - 1, candidate);
    }
    let ranking = observe.then(|| FusionRanking {
        rank_offset: RANK_OFFSET,
        dense_weight,
        lexical_candidates,
        dense_candidates,
        candidates: candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| FusionCandidate {
                lexical_rank: candidate.lexical_rank,
                dense_rank: candidate.dense_rank,
                score: candidate.score,
                score_rank: candidate.score_rank,
                final_rank: (index < MAX_RESULTS).then(|| count(index + 1)),
                lexical_promoted: candidate.lexical_promoted,
            })
            .collect(),
    });
    let hits = candidates
        .into_iter()
        .take(MAX_RESULTS)
        .map(|candidate| candidate.hit)
        .collect();
    (hits, ranking)
}

fn count(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str) -> Vec<SearchHit> {
        (1..=MAX_RESULTS)
            .map(|rank| SearchHit {
                source: format!("{name}-{rank}"),
                revision: "pin".into(),
                kind: "current".into(),
                tracked_ref: Some("refs/heads/main".into()),
                title: name.into(),
                access: "available",
                location: None,
            })
            .collect()
    }

    #[test]
    fn scoped_prose_ranks_dense_hits_and_keeps_the_first_lexical_candidate()
    -> Result<(), crate::application::Failure> {
        let query = SearchQuery::parse_scoped(
            "Quelle limite applique ce dépôt.",
            Some(2),
            Some("main".into()),
        )?;
        let (hits, ranking) = fuse(&query, branch("lexical"), branch("dense"), true);
        let ranking = ranking.ok_or(crate::application::Failure::Storage)?;
        assert_eq!(ranking.candidates.len(), 40);
        let promoted = &ranking.candidates[4];
        assert_eq!(promoted.lexical_rank, Some(1));
        assert_eq!(promoted.dense_rank, None);
        assert_eq!(promoted.final_rank, Some(5));
        assert_eq!(promoted.score_rank, 17);
        assert!(promoted.lexical_promoted);
        assert!((promoted.score - 1.0 / 61.0).abs() < f64::EPSILON);
        assert!(
            ranking.candidates[20..]
                .iter()
                .all(|candidate| candidate.final_rank.is_none())
        );
        assert_eq!(hits.len(), MAX_RESULTS);
        let head: Vec<_> = hits.iter().take(5).map(|hit| hit.source.as_str()).collect();
        assert_eq!(
            head,
            ["dense-1", "dense-2", "dense-3", "dense-4", "lexical-1"]
        );
        assert_eq!(
            hits.iter().filter(|hit| hit.source == "lexical-1").count(),
            1
        );
        Ok(())
    }

    #[test]
    fn global_search_and_source_literals_keep_stable_equal_weight_order()
    -> Result<(), crate::application::Failure> {
        for text in [
            "get_adapter",
            "Where is MAX_REQUESTS_PER_MINUTE applied?",
            "Where does the client use `getAdapter`?",
            "Where does the client use getAdapter?",
            "Compare the setting in src/adapter.rs",
            "Explain provider.rs",
            "How is crate::option resolved?",
        ] {
            let query = SearchQuery::parse_scoped(text, Some(2), Some("main".into()))?;
            let (hits, ranking) = fuse(&query, branch("lexical"), branch("dense"), false);
            assert!(ranking.is_none());
            assert_eq!(hits[0].source, "lexical-1", "{text}");
            assert_eq!(hits[1].source, "dense-1", "{text}");
        }
        let query = SearchQuery::parse("Which records are retained?")?;
        let (hits, _) = fuse(&query, branch("lexical"), branch("dense"), false);
        assert_eq!(hits[0].source, "lexical-1");
        assert_eq!(hits[1].source, "dense-1");
        Ok(())
    }
}
