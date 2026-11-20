use ctxfab_inference::Embedding;
use rusqlite::{Connection, named_params, params, types::Type};

use super::{CODE_COLLECTION, SqliteStore};
use crate::application::{
    CodeScope, SearchHit,
    retrieval::{Passage, Progress, RetrievalError, VectorIndex},
};
use crate::domain::corpus::snapshot::Location;

mod ranking;

pub(super) const CANDIDATE_COLUMNS: &str = "
    d.source, d.revision, d.kind, nullif(d.tracked_ref, ''), d.title, c.access_lost, d.source_location";

impl VectorIndex for SqliteStore {
    fn progress(
        &self,
        profile: &'static str,
        scope: Option<&CodeScope>,
    ) -> Result<Progress, RetrievalError> {
        let database = self.read().map_err(|_| RetrievalError::Storage)?;
        progress(&database, profile, scope).map_err(|_| RetrievalError::Storage)
    }

    fn pending(&self, profile: &'static str, limit: u16) -> Result<Vec<Passage>, RetrievalError> {
        let database = self.read().map_err(|_| RetrievalError::Storage)?;
        let sql = format!(
            "SELECT {CANDIDATE_COLUMNS}, d.id, d.collection, d.content, c.generation
             FROM documents d JOIN collections c ON c.name = d.collection
             LEFT JOIN document_vectors v ON v.document_id = d.id AND v.profile = ?1
             WHERE v.document_id IS NULL ORDER BY d.id LIMIT ?2"
        );
        let mut statement = database
            .prepare(&sql)
            .map_err(|_| RetrievalError::Storage)?;
        let rows = statement
            .query_map(params![profile, limit], |row| {
                Ok(Passage {
                    evidence: hit(row)?,
                    id: row.get(7)?,
                    collection: row.get(8)?,
                    content: row.get(9)?,
                    generation: row.get(10)?,
                })
            })
            .map_err(|_| RetrievalError::Storage)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| RetrievalError::Storage)
    }

    fn save_vector(
        &self,
        profile: &'static str,
        passage: &Passage,
        vector: &Embedding,
    ) -> Result<bool, RetrievalError> {
        let blob: Vec<u8> = vector
            .values()
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        self.write().map_err(|_| RetrievalError::Storage)?.execute(
            "INSERT INTO document_vectors (document_id, profile, embedding)
             SELECT id, ?2, ?3 FROM documents WHERE id = ?1
               AND collection = ?4 AND source = ?5 AND revision = ?6 AND kind = ?7
               AND tracked_ref = ?8 AND title = ?9 AND content = ?10
               AND passage_start = coalesce(?12, -1)
               AND EXISTS (SELECT 1 FROM collections WHERE name = ?4 AND generation = ?11)
             ON CONFLICT(document_id) DO UPDATE SET profile = excluded.profile, embedding = excluded.embedding",
            params![
                passage.id, profile, blob,
                passage.collection, passage.evidence.source, passage.evidence.revision,
                passage.evidence.kind, passage.evidence.tracked_ref.as_deref().unwrap_or(""),
                passage.evidence.title, passage.content, passage.generation, passage.evidence.passage_start(),
            ],
        ).map(|rows| rows == 1).map_err(|_| RetrievalError::Storage)
    }

    fn rank(
        &self,
        profile: &'static str,
        vector: &Embedding,
        lexical: Vec<SearchHit>,
        scope: Option<&CodeScope>,
    ) -> Result<(Vec<SearchHit>, Vec<SearchHit>), RetrievalError> {
        ranking::rank(self, profile, vector, lexical, scope)
    }
}

fn progress(
    database: &Connection,
    profile: &'static str,
    scope: Option<&CodeScope>,
) -> rusqlite::Result<Progress> {
    if let Some(scope) = scope {
        let sql = format!(
            "SELECT count(v.document_id), count(*) FROM documents d
             LEFT JOIN document_vectors v ON v.document_id = d.id AND v.profile = :profile
             WHERE d.collection = {CODE_COLLECTION} AND d.publication_id IS NOT NULL"
        );
        return database.query_row(
            &sql,
            named_params! {":profile": profile, ":repository_id": scope.repository_id, ":branch": scope.branch},
            |row| Ok(Progress::new(profile, row.get(0)?, row.get(1)?)),
        );
    }
    database.query_row(
        "SELECT (SELECT count(*) FROM document_vectors WHERE profile = ?1),
                (SELECT count(*) FROM documents)",
        [profile],
        |row| Ok(Progress::new(profile, row.get(0)?, row.get(1)?)),
    )
}

pub(super) fn hit(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchHit> {
    Ok(SearchHit {
        source: row.get(0)?,
        revision: row.get(1)?,
        kind: row.get(2)?,
        tracked_ref: row.get(3)?,
        title: row.get(4)?,
        access: access(row.get(5)?),
        location: location(row, 6)?,
    })
}

pub(super) fn location(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<Location>> {
    row.get::<_, Option<String>>(index)?
        .map(|value| {
            serde_json::from_str(&value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
            })
        })
        .transpose()
}

const fn access(retained: bool) -> &'static str {
    if retained { "retained" } else { "available" }
}
