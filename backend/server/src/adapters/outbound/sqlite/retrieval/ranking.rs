use ctxfab_inference::Embedding;
use rusqlite::{Connection, OptionalExtension, named_params};

use super::{SqliteStore, access, hit, location, progress};
use crate::adapters::outbound::sqlite::{CODE_COLLECTION, scope_parameters};
use crate::application::{
    CodeScope, SearchHit,
    retrieval::{MAX_RESULTS, RetrievalError},
};

pub(super) fn rank(
    store: &SqliteStore,
    profile: &'static str,
    vector: &Embedding,
    mut lexical: Vec<SearchHit>,
    scope: Option<&CodeScope>,
) -> Result<(Vec<SearchHit>, Vec<SearchHit>), RetrievalError> {
    let mut database = store.read().map_err(|_| RetrievalError::Storage)?;
    let tx = database
        .transaction()
        .map_err(|_| RetrievalError::Storage)?;
    let counts = progress(&tx, profile, scope).map_err(|_| RetrievalError::Storage)?;
    if counts.indexed != counts.total {
        return Err(RetrievalError::Incomplete);
    }
    retain_visible(&tx, &mut lexical, scope).map_err(|_| RetrievalError::Storage)?;
    let dense = nearest(&tx, profile, vector, scope)?;
    tx.commit().map_err(|_| RetrievalError::Storage)?;
    Ok((dense, lexical))
}

fn retain_visible(
    database: &Connection,
    lexical: &mut Vec<SearchHit>,
    scope: Option<&CodeScope>,
) -> rusqlite::Result<()> {
    let (repository_id, branch) = scope_parameters(scope);
    let mut check = database.prepare(&format!(
        "SELECT c.access_lost, d.source_location FROM documents d JOIN collections c ON c.name = d.collection
         WHERE d.source = :source AND d.revision = :revision AND d.kind = :kind AND d.tracked_ref = :ref
               AND d.passage_start = coalesce(:start, -1)
               AND (:repository_id IS NULL OR (d.collection = {CODE_COLLECTION} AND d.publication_id IS NOT NULL))
         ORDER BY c.access_lost, d.id LIMIT 1",
    ))?;
    let mut visible = Vec::with_capacity(lexical.len());
    for mut hit in lexical.drain(..) {
        let owned = check
            .query_row(
                named_params! {
                    ":source": hit.source, ":revision": hit.revision, ":kind": hit.kind,
                    ":ref": hit.tracked_ref.as_deref().unwrap_or(""), ":start": hit.passage_start(),
                    ":repository_id": repository_id, ":branch": branch,
                },
                |row| Ok((row.get::<_, bool>(0)?, location(row, 1)?)),
            )
            .optional()?;
        if let Some((retained, current)) = owned {
            let previous = std::mem::replace(&mut hit.location, current);
            let unchanged = match (&previous, &hit.location) {
                (Some(previous), Some(current)) => previous.same_range(current),
                (None, None) => true,
                _ => false,
            };
            if unchanged {
                hit.access = access(retained);
                visible.push(hit);
            }
        }
    }
    *lexical = visible;
    Ok(())
}

fn nearest(
    database: &Connection,
    profile: &'static str,
    vector: &Embedding,
    scope: Option<&CodeScope>,
) -> Result<Vec<SearchHit>, RetrievalError> {
    let (repository_id, branch) = scope_parameters(scope);
    let sql = format!("SELECT source, revision, kind, ref, title, access_lost, source_location, embedding
         FROM (
            SELECT d.source, d.revision, d.kind, nullif(d.tracked_ref, '') AS ref,
                   d.title, c.access_lost, d.source_location, d.passage_start, v.embedding,
                   row_number() OVER (PARTITION BY d.source, d.revision, d.kind, d.tracked_ref, d.passage_start
                                      ORDER BY c.access_lost, d.id) AS position
            FROM document_vectors v JOIN documents d ON d.id = v.document_id
            JOIN collections c ON c.name = d.collection WHERE v.profile = :profile
             AND (:repository_id IS NULL OR (d.collection = {CODE_COLLECTION} AND d.publication_id IS NOT NULL))
         ) WHERE position = 1 ORDER BY source, revision, kind, ref, passage_start");
    let mut query = database
        .prepare(&sql)
        .map_err(|_| RetrievalError::Storage)?;
    let mut rows = query
        .query(
            named_params! {":profile": profile, ":repository_id": repository_id, ":branch": branch},
        )
        .map_err(|_| RetrievalError::Storage)?;
    let mut best = Vec::new();
    while let Some(row) = rows.next().map_err(|_| RetrievalError::Storage)? {
        let blob: Vec<u8> = row.get(7).map_err(|_| RetrievalError::Storage)?;
        let values = blob
            .as_chunks::<4>()
            .0
            .iter()
            .copied()
            .map(f32::from_le_bytes);
        let similarity = vector
            .values()
            .iter()
            .zip(values)
            .map(|(query, value)| query * value)
            .sum::<f32>();
        if !similarity.is_finite() {
            return Err(RetrievalError::Vector);
        }
        best.push((hit(row).map_err(|_| RetrievalError::Storage)?, similarity));
        best.sort_by(|left, right| right.1.total_cmp(&left.1));
        best.truncate(MAX_RESULTS);
    }
    Ok(best.into_iter().map(|(hit, _)| hit).collect())
}
