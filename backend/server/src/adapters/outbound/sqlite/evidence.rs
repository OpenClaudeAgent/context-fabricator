use rusqlite::{OptionalExtension, named_params};

use super::{CODE_COLLECTION, SqliteStore, retrieval::location, scope_parameters};
use crate::application::{
    CodeScope, SearchHit,
    answers::{AnswerError, EvidenceStore, Excerpt},
};

const EXCERPT_BYTES: u16 = 600;

impl EvidenceStore for SqliteStore {
    fn excerpts(
        &self,
        hits: &[SearchHit],
        scope: Option<&CodeScope>,
    ) -> Result<Vec<Excerpt>, AnswerError> {
        self.read_excerpts(hits, scope, false)
    }

    fn following(
        &self,
        hits: &[SearchHit],
        scope: Option<&CodeScope>,
    ) -> Result<Vec<Excerpt>, AnswerError> {
        self.read_excerpts(hits, scope, true)
    }
}

impl SqliteStore {
    fn read_excerpts(
        &self,
        hits: &[SearchHit],
        scope: Option<&CodeScope>,
        following: bool,
    ) -> Result<Vec<Excerpt>, AnswerError> {
        let mut connection = self.read().map_err(|_| AnswerError::Storage)?;
        let snapshot = connection.transaction().map_err(|_| AnswerError::Storage)?;
        let (repository_id, branch) = scope_parameters(scope);
        let mut query = snapshot
            .prepare(&format!(
                "SELECT d.title, c.access_lost, d.source_location,
                        CASE WHEN d.passage_start < 0 THEN substr(d.content, 1, :excerpt_bytes) ELSE d.content END
             FROM documents d JOIN collections c ON c.name = d.collection
             WHERE d.source = :source AND d.revision = :revision AND d.kind = :kind AND d.tracked_ref = :ref
               AND d.passage_start = coalesce(:start, -1)
               AND (:repository_id IS NULL OR (d.collection = {CODE_COLLECTION} AND d.publication_id IS NOT NULL))
             ORDER BY c.access_lost, d.id LIMIT 1",
            ))
            .map_err(|_| AnswerError::Storage)?;
        let mut excerpts = Vec::with_capacity(hits.len());
        for hit in hits {
            let start = if following {
                let Some(location) = &hit.location else {
                    continue;
                };
                Some(location.end_byte)
            } else {
                hit.passage_start()
            };
            let found = query
                .query_row(
                    named_params! {
                        ":source": hit.source, ":revision": hit.revision, ":kind": hit.kind,
                        ":ref": hit.tracked_ref.as_deref().unwrap_or(""), ":start": start,
                        ":excerpt_bytes": EXCERPT_BYTES, ":repository_id": repository_id, ":branch": branch,
                    },
                    |row| {
                        let mut evidence = hit.clone();
                        evidence.title = row.get(0)?;
                        evidence.access = if row.get(1)? { "retained" } else { "available" };
                        evidence.location = location(row, 2)?;
                        let mut text: String = row.get(3)?;
                        if evidence.location.is_none() {
                            text.truncate(text.floor_char_boundary(usize::from(EXCERPT_BYTES)));
                        }
                        Ok(Excerpt { evidence, text })
                    },
                )
                .optional()
                .map_err(|_| AnswerError::Storage)?;
            if let Some(excerpt) = found
                && is_matching_passage(hit, &excerpt.evidence, following)
            {
                excerpts.push(excerpt);
            }
        }
        Ok(excerpts)
    }
}

fn is_matching_passage(previous: &SearchHit, found: &SearchHit, following: bool) -> bool {
    if following {
        return matches!((&previous.location, &found.location), (Some(previous), Some(found))
            if previous.blob == found.blob && previous.path == found.path && previous.end_byte == found.start_byte);
    }
    found.same_evidence(previous)
}
