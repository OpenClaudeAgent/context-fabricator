use super::evidence_review::{Candidate, Disposition, RankedCandidate};
use super::{AnswerError, EvidenceStore, Excerpt, MAX_SOURCES, SearchHit, SearchQuery};

pub(super) fn select(
    evidence: &dyn EvidenceStore,
    query: &SearchQuery,
    hits: &[SearchHit],
    formulated: bool,
    audited: Option<&mut [RankedCandidate]>,
) -> Result<Vec<Excerpt>, AnswerError> {
    let promoted;
    let hits = if let Some(section) = evidence.readme_section(query)? {
        promoted = std::iter::once(section.clone())
            .chain(
                hits.iter()
                    .filter(|hit| !hit.same_evidence(&section))
                    .cloned(),
            )
            .collect::<Vec<_>>();
        &promoted
    } else {
        hits
    };
    let prose = formulated || query.is_scoped_prose();
    let ranked = if prose {
        hits
    } else {
        &hits[..hits.len().min(MAX_SOURCES)]
    };
    let mut excerpts = evidence.excerpts(ranked, query.scope())?;
    if let Some(audited) = audited {
        mark_missing(audited, ranked, &excerpts);
    }
    if prose {
        let mut distinct: Vec<Excerpt> = Vec::new();
        for passage in excerpts {
            if !distinct.iter().any(|other| redundant(other, &passage)) {
                distinct.push(passage);
            }
        }
        // Preserve the top hit and complementary evidence from the same file. When a
        // file dominates the ranking, let another source contribute before a third hit.
        let mut selected: Vec<Excerpt> = Vec::new();
        let mut deferred = Vec::new();
        for passage in distinct {
            let same_file = selected
                .iter()
                .filter(|other| {
                    other.evidence.source == passage.evidence.source
                        && other.evidence.revision == passage.evidence.revision
                        && other.evidence.tracked_ref == passage.evidence.tracked_ref
                })
                .count();
            if same_file < 2 {
                selected.push(passage);
            } else {
                deferred.push(passage);
            }
            if selected.len() == MAX_SOURCES {
                break;
            }
        }
        selected.extend(deferred.into_iter().take(MAX_SOURCES - selected.len()));
        excerpts = selected;
        if excerpts.len() == MAX_SOURCES {
            return Ok(excerpts);
        }
    }
    let anchors: Vec<_> = excerpts
        .iter()
        .take(2)
        .filter(|excerpt| excerpt.evidence.location.is_some())
        .map(|excerpt| excerpt.evidence.clone())
        .collect();
    if !anchors.is_empty() {
        let neighbors = evidence.following(&anchors, query.scope())?;
        let next = neighbors
            .into_iter()
            .filter(|neighbor| {
                !excerpts
                    .iter()
                    .any(|excerpt| excerpt.evidence.same_evidence(&neighbor.evidence))
            })
            .collect::<Vec<_>>();
        let insertion = if prose {
            excerpts.len()
        } else {
            excerpts.len().min(2)
        };
        excerpts.splice(insertion..insertion, next);
        excerpts.truncate(MAX_SOURCES);
    }
    Ok(excerpts)
}

fn redundant(a: &Excerpt, b: &Excerpt) -> bool {
    let (x, y) = (&a.evidence, &b.evidence);
    if x.same_evidence(y) {
        return true;
    }
    let (Some(left), Some(right)) = (&x.location, &y.location) else {
        return false;
    };
    if x.source != y.source
        || x.revision != y.revision
        || x.tracked_ref != y.tracked_ref
        || x.kind != y.kind
        || left.blob != right.blob
        || left.path != right.path
    {
        return false;
    }
    // Containment has no novel source bytes. Adjacent passages and partial overlaps
    // can carry complementary statements, even when both share an entire source line.
    (left.start_byte <= right.start_byte && right.end_byte <= left.end_byte)
        || (right.start_byte <= left.start_byte
            && left.end_byte <= right.end_byte
            && a.text == b.text)
}

fn mark_missing(candidates: &mut [RankedCandidate], attempted: &[SearchHit], excerpts: &[Excerpt]) {
    for candidate in candidates {
        if attempted
            .iter()
            .any(|hit| hit.same_evidence(&candidate.hit))
            && !excerpts
                .iter()
                .any(|excerpt| excerpt.evidence.same_evidence(&candidate.hit))
        {
            candidate.review.mark(Disposition::PassageUnavailable);
        }
    }
}

pub(super) fn merge_formulation(
    query: &SearchQuery,
    mut original: Vec<SearchHit>,
    mut auxiliary: Vec<SearchHit>,
    mut excluded: Option<&mut Vec<RankedCandidate>>,
) -> Vec<SearchHit> {
    if auxiliary.is_empty() {
        return original;
    }
    if !asks_for_fixtures(query) {
        let mut retain = |hit: &SearchHit| {
            let fixture = is_fixture(hit);
            if fixture
                && let Some(excluded) = &mut excluded
                && !excluded
                    .iter()
                    .any(|candidate| candidate.hit.same_evidence(hit))
            {
                let mut review = Candidate::passage(hit, None);
                review.mark(Disposition::FixtureExcluded);
                excluded.push(RankedCandidate {
                    hit: hit.clone(),
                    review,
                });
            }
            !fixture
        };
        original.retain(&mut retain);
        auxiliary.retain(retain);
    }
    let mut merged: Vec<SearchHit> = Vec::new();
    // Keep two places for the original search while diversifying the leading source files.
    for ranking in [
        auxiliary.as_slice(),
        original.as_slice(),
        auxiliary.as_slice(),
        original.as_slice(),
        auxiliary.as_slice(),
    ] {
        if let Some(hit) = ranking
            .iter()
            .find(|hit| !merged.iter().any(|previous| previous.source == hit.source))
        {
            merged.push(hit.clone());
        }
    }
    for hit in original.into_iter().chain(auxiliary) {
        if !merged.iter().any(|previous| previous.same_evidence(&hit)) {
            merged.push(hit);
        }
    }
    merged
}

pub(super) fn distinct_ranked_passages(original: &[SearchHit], auxiliary: &[SearchHit]) -> usize {
    let mut distinct: Vec<&SearchHit> = Vec::new();
    for hit in original.iter().chain(auxiliary) {
        if !distinct
            .iter()
            .any(|candidate| candidate.same_evidence(hit))
        {
            distinct.push(hit);
        }
    }
    distinct.len()
}

pub(super) fn asks_for_fixtures(query: &SearchQuery) -> bool {
    query.terms().iter().any(|word| {
        [
            "test",
            "tests",
            "fixture",
            "fixtures",
            "mock",
            "mocks",
            "benchmark",
            "benchmarks",
        ]
        .iter()
        .any(|term| word.eq_ignore_ascii_case(term))
    })
}

fn is_fixture(hit: &SearchHit) -> bool {
    hit.location.as_ref().is_some_and(|location| {
        let mut parts = location.path.split('/');
        let file = parts.next_back().unwrap_or_default();
        parts.any(|part| {
            [
                "test",
                "tests",
                "__tests__",
                "fixtures",
                "testdata",
                "benchmarks",
            ]
            .iter()
            .any(|name| part.eq_ignore_ascii_case(name))
        }) || (location.path.starts_with("quality/")
            && (file == "corpus.json" || file.ends_with("-judgments.json")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::CodeScope;
    use crate::domain::corpus::snapshot::Location;

    struct Fixture {
        missing: &'static str,
    }

    impl EvidenceStore for Fixture {
        fn excerpts(
            &self,
            hits: &[SearchHit],
            scope: Option<&CodeScope>,
        ) -> Result<Vec<Excerpt>, AnswerError> {
            Ok(hits
                .iter()
                .filter(|hit| hit.title != self.missing)
                .map(|hit| Excerpt {
                    evidence: hit.clone(),
                    text: hit.title.clone(),
                    repository_id: scope.map(|scope| scope.repository_id),
                })
                .collect())
        }

        fn following(
            &self,
            hits: &[SearchHit],
            scope: Option<&CodeScope>,
        ) -> Result<Vec<Excerpt>, AnswerError> {
            let followers: Vec<_> = hits
                .iter()
                .map(|hit| {
                    let mut next = hit.clone();
                    next.title = format!("{} continuation", hit.title);
                    if let Some(location) = &mut next.location {
                        location.start_byte = location.end_byte;
                        location.end_byte += 10;
                        location.start_line = location.end_line + 1;
                        location.end_line += 1;
                    }
                    next
                })
                .collect();
            self.excerpts(&followers, scope)
        }
    }

    fn hits() -> Vec<SearchHit> {
        (1..=6)
            .map(|rank| SearchHit {
                source: format!("github:42:source-{rank}.rs"),
                title: rank.to_string(),
                revision: "pin".into(),
                kind: "current".into(),
                tracked_ref: Some("refs/heads/main".into()),
                access: "available",
                location: Some(Location {
                    path: format!("source-{rank}.rs"),
                    blob: format!("file-{rank}"),
                    file_url: String::new(),
                    start_byte: 0,
                    end_byte: 10,
                    start_line: 1,
                    end_line: 1,
                }),
            })
            .collect()
    }

    fn titles(excerpts: &[Excerpt]) -> Vec<&str> {
        excerpts
            .iter()
            .map(|excerpt| excerpt.evidence.title.as_str())
            .collect()
    }

    #[test]
    fn reformulation_keeps_two_original_hits_and_reaches_the_third_auxiliary_hit()
    -> Result<(), Box<dyn std::error::Error>> {
        let original = hits();
        let auxiliary: Vec<_> = original
            .iter()
            .cloned()
            .map(|mut hit| {
                hit.source = format!("alternative-{}", hit.source);
                hit.title = format!("alternative-{}", hit.title);
                hit
            })
            .collect();
        let query = SearchQuery::parse("Quel délai suit une livraison incertaine ?")?;
        let fixture = Fixture { missing: "" };
        let merged = merge_formulation(&query, original.clone(), auxiliary.clone(), None);
        let context = select(&fixture, &query, &merged, true, None)?;
        assert_eq!(
            titles(&context),
            ["alternative-1", "1", "alternative-2", "2", "alternative-3"]
        );
        let mut repeated_file = auxiliary;
        repeated_file[1].source = repeated_file[0].source.clone();
        let diversified = merge_formulation(&query, original.clone(), repeated_file, None);
        let context = select(&fixture, &query, &diversified, true, None)?;
        assert_eq!(
            titles(&context),
            ["alternative-1", "1", "alternative-3", "2", "alternative-4"]
        );
        let fallback = merge_formulation(&query, original.clone(), Vec::new(), None);
        assert!(
            fallback
                .iter()
                .zip(original)
                .all(|(a, b)| a.same_evidence(&b))
        );
        Ok(())
    }

    #[test]
    fn reformulation_admits_fixtures_for_explicit_test_questions()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut auxiliary = hits();
        for (hit, path) in auxiliary.iter_mut().zip([
            "backend/tests/outbound.rs",
            "backend/tests/outbound.rs",
            "Client/Tests/Answers.swift",
            "testdata/response.json",
            "backend/domain/sync.rs",
        ]) {
            hit.location
                .as_mut()
                .ok_or("missing fixture location")?
                .path = path.to_owned();
        }
        let fixture = SearchQuery::parse("What does the retry test fixture report?")?;
        let fixture_hits = merge_formulation(&fixture, Vec::new(), auxiliary.clone(), None);
        assert_eq!(fixture_hits[0].title, "1");
        let production = SearchQuery::parse("Quel délai applique le service ?")?;
        let production_hits = merge_formulation(&production, Vec::new(), auxiliary.clone(), None);
        let ranked: Vec<_> = production_hits
            .iter()
            .map(|hit| hit.title.as_str())
            .collect();
        assert_eq!(ranked, ["5", "6"]);
        auxiliary.truncate(4);
        assert!(merge_formulation(&production, Vec::new(), auxiliary, None).is_empty());
        Ok(())
    }

    #[test]
    fn scoped_prose_prioritizes_ranked_passages_and_backfills_a_missing_hit()
    -> Result<(), Box<dyn std::error::Error>> {
        let question = SearchQuery::parse_scoped(
            "What delay applies after an uncertain request?",
            Some(42),
            Some("main".into()),
        )?;
        let hits = hits();
        let context = select(&Fixture { missing: "" }, &question, &hits, false, None)?;
        assert_eq!(titles(&context), ["1", "2", "3", "4", "5"]);
        let context = select(&Fixture { missing: "2" }, &question, &hits, false, None)?;
        assert_eq!(titles(&context), ["1", "3", "4", "5", "6"]);
        assert!(
            context
                .iter()
                .all(|excerpt| excerpt.repository_id == Some(42))
        );
        let context = select(&Fixture { missing: "" }, &question, &hits[..4], false, None)?;
        assert_eq!(titles(&context), ["1", "2", "3", "4", "1 continuation"]);
        Ok(())
    }

    #[test]
    fn scoped_prose_preserves_adjacent_evidence_and_defers_file_dominance()
    -> Result<(), Box<dyn std::error::Error>> {
        let query = SearchQuery::parse_scoped(
            "Which delay applies after a request?",
            Some(42),
            Some("main".into()),
        )?;
        let mut ranked = hits();
        for index in 1..4 {
            let mut same = ranked[0].clone();
            same.title = (index + 1).to_string();
            let location = same.location.as_mut().ok_or("missing test location")?;
            // One contained hit, then two distinct adjacent intervals on the same line.
            let (start, end) = [(0, 0), (2, 6), (10, 20), (20, 30)][index];
            location.start_byte = start;
            location.end_byte = end;
            ranked[index] = same;
        }
        let context = select(&Fixture { missing: "" }, &query, &ranked, false, None)?;
        assert_eq!(titles(&context), ["1", "3", "5", "6", "4"]);
        Ok(())
    }

    #[test]
    fn explicit_source_and_global_queries_keep_contiguous_followers()
    -> Result<(), Box<dyn std::error::Error>> {
        let literal = SearchQuery::parse_scoped(
            "Explain UNCERTAIN_COOLDOWN_MS",
            Some(42),
            Some("main".into()),
        )?;
        let global = SearchQuery::parse("What delay applies after an uncertain request?")?;
        let hits = hits();
        for question in [literal, global] {
            let context = select(&Fixture { missing: "" }, &question, &hits, false, None)?;
            assert_eq!(
                titles(&context),
                ["1", "2", "1 continuation", "2 continuation", "3"]
            );
        }
        Ok(())
    }
}
