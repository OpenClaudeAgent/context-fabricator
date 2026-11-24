use rusqlite::{OptionalExtension, named_params};

use super::{CODE_COLLECTION, SqliteStore, retrieval::location, scope_parameters, search};
use crate::application::{
    CodeScope, SearchHit, SearchQuery,
    answers::{AnswerError, EvidenceStore, Excerpt, JavaEvidence},
};

mod graph;

const EXCERPT_BYTES: u16 = 600;

impl EvidenceStore for SqliteStore {
    fn java_method_passages(&self, query: &SearchQuery) -> Result<Vec<Excerpt>, AnswerError> {
        search::resolve_java_method(self, query, false)
            .map(|evidence| evidence.passages)
            .map_err(|_| AnswerError::Storage)
    }

    fn java_evidence(&self, query: &SearchQuery) -> Result<JavaEvidence, AnswerError> {
        search::resolve_java_method(self, query, true).map_err(|_| AnswerError::Storage)
    }

    fn current_document(
        &self,
        excerpt: &Excerpt,
        scope: Option<&CodeScope>,
    ) -> Result<Option<String>, AnswerError> {
        let Some(location) = &excerpt.evidence.location else {
            return Ok(None);
        };
        let Some(owner) = excerpt.repository_id else {
            return Ok(None);
        };
        let (repository_id, branch) = scope_parameters(scope);
        let db = self.read().map_err(|_| AnswerError::Storage)?;
        let mut statement = db
            .prepare(&format!(
                "SELECT b.content FROM documents d
             JOIN source_snapshots s ON s.id=d.publication_id
             JOIN source_snapshot_files f ON f.snapshot_id=s.id AND f.source=d.source
             JOIN source_blobs b ON b.collection=f.collection AND b.blob=f.blob
             JOIN github_code_scopes scope ON scope.collection=f.collection
             WHERE d.kind='current' AND s.phase='published' AND scope.repository_id=:owner
               AND s.tracked_ref=d.tracked_ref AND s.revision=d.revision
               AND d.source=:source AND d.revision=:revision AND d.tracked_ref=:ref
               AND f.path=:path AND f.blob=:blob AND d.passage_start=:start
               AND (:repository_id IS NULL OR d.collection = {CODE_COLLECTION})
               AND length(CAST(b.content AS BLOB))<=262144
             LIMIT 2"
            ))
            .map_err(|_| AnswerError::Storage)?;
        let result = statement.query_map(named_params! {
            ":owner":owner, ":source":excerpt.evidence.source, ":revision":excerpt.evidence.revision,
            ":ref":excerpt.evidence.tracked_ref, ":path":location.path, ":blob":location.blob,
            ":start":location.start_byte, ":repository_id":repository_id, ":branch":branch,
        }, |row| row.get::<_,String>(0)).map_err(|_| AnswerError::Storage)?
            .collect::<rusqlite::Result<Vec<_>>>().map_err(|_| AnswerError::Storage)?;
        let [document] = result.as_slice() else {
            return Ok(None);
        };
        Ok(Some(document.clone()))
    }

    fn authoritative_readme(
        &self,
        query: &SearchQuery,
    ) -> Result<Option<Vec<Excerpt>>, AnswerError> {
        search::authoritative_readme(self, query).map_err(|_| AnswerError::Storage)
    }

    fn readme_section(&self, query: &SearchQuery) -> Result<Option<SearchHit>, AnswerError> {
        search::readme_section(self, query).map_err(|_| AnswerError::Storage)
    }

    fn graph_passages(
        &self,
        symbol: &str,
        root: Option<&str>,
        scope: Option<&CodeScope>,
    ) -> Result<Vec<Excerpt>, AnswerError> {
        graph::resolve(self, symbol, root, scope)
    }

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
                        CASE WHEN d.passage_start < 0 THEN substr(d.content, 1, :excerpt_bytes) ELSE d.content END,
                        (SELECT repository_id FROM github_code_scopes WHERE collection = d.collection)
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
                        Ok(Excerpt {
                            evidence,
                            text,
                            repository_id: row.get(4)?,
                        })
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
