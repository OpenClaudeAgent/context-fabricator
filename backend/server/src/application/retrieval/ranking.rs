use super::MAX_RESULTS;
use crate::application::SearchHit;

pub(super) fn fuse(lexical: Vec<SearchHit>, dense: Vec<SearchHit>) -> Vec<SearchHit> {
    let mut candidates: Vec<(SearchHit, f64)> = Vec::new();
    for branch in [lexical, dense] {
        for (rank, hit) in (61_u32..).zip(branch) {
            let score = 1.0 / f64::from(rank);
            if let Some((_, aggregate)) = candidates
                .iter_mut()
                .find(|(previous, _)| previous.same_evidence(&hit))
            {
                *aggregate += score;
            } else {
                candidates.push((hit, score));
            }
        }
    }
    // The stable sort preserves lexical order when the combined scores tie.
    candidates.sort_by(|left, right| right.1.total_cmp(&left.1));
    candidates
        .into_iter()
        .take(MAX_RESULTS)
        .map(|(hit, _)| hit)
        .collect()
}
