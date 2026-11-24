use std::time::Instant;

use ctxfab_inference::Embedding;
use rusqlite::{Connection, OptionalExtension, named_params};

use super::{SqliteStore, access, hit, location, progress};
use crate::adapters::outbound::sqlite::{CODE_COLLECTION, scope_parameters};
use crate::application::{
    CodeScope, SearchHit,
    retrieval::{MAX_RESULTS, RetrievalError, VectorMatch, VectorRanking},
};

pub(super) fn rank(
    store: &SqliteStore,
    profile: &str,
    vector: &Embedding,
    mut lexical: Vec<SearchHit>,
    scope: Option<&CodeScope>,
    formulation: Option<(&Embedding, Instant)>,
    inspect: bool,
) -> Result<VectorRanking, RetrievalError> {
    let mut database = store.read().map_err(|_| RetrievalError::Storage)?;
    let tx = database
        .transaction()
        .map_err(|_| RetrievalError::Storage)?;
    let counts = progress(&tx, profile, scope).map_err(|_| RetrievalError::Storage)?;
    if counts.indexed != counts.total {
        return Err(RetrievalError::Incomplete);
    }
    retain_visible(&tx, &mut lexical, scope).map_err(|_| RetrievalError::Storage)?;
    let (dense, alternative) = nearest(&tx, profile, vector, scope, formulation, inspect)?;
    tx.commit().map_err(|_| RetrievalError::Storage)?;
    Ok(VectorRanking {
        dense: dense.hits,
        lexical,
        formulation: alternative.hits,
        nearest: dense.matches,
        reformulated_nearest: alternative.matches,
    })
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

struct DenseResult {
    hits: Vec<SearchHit>,
    matches: Vec<VectorMatch>,
}

fn nearest(
    database: &Connection,
    profile: &str,
    vector: &Embedding,
    scope: Option<&CodeScope>,
    mut formulation: Option<(&Embedding, Instant)>,
    inspect: bool,
) -> Result<(DenseResult, DenseResult), RetrievalError> {
    let (repository_id, branch) = scope_parameters(scope);
    let sql = format!("SELECT ranked.id, v.embedding
         FROM (
            SELECT d.id, d.source, d.revision, d.kind, nullif(d.tracked_ref, '') AS ref,
                   d.passage_start,
                   row_number() OVER (PARTITION BY d.source, d.revision, d.kind, d.tracked_ref, d.passage_start
                                      ORDER BY c.access_lost, d.id) AS position
            FROM document_vectors v JOIN documents d ON d.id = v.document_id
            JOIN collections c ON c.name = d.collection WHERE v.profile = :profile
             AND (:repository_id IS NULL OR (d.collection = {CODE_COLLECTION} AND d.publication_id IS NOT NULL))
         ) ranked
         JOIN document_vectors v ON v.document_id = ranked.id AND v.profile = :profile
         WHERE position = 1 ORDER BY ranked.source, ranked.revision, ranked.kind, ranked.ref, ranked.passage_start");
    let mut query = database
        .prepare(&sql)
        .map_err(|_| RetrievalError::Storage)?;
    let mut rows = query
        .query(
            named_params! {":profile": profile, ":repository_id": repository_id, ":branch": branch},
        )
        .map_err(|_| RetrievalError::Storage)?;
    let mut best = Vec::new();
    let mut auxiliary = Vec::new();
    while let Some(row) = rows.next().map_err(|_| RetrievalError::Storage)? {
        let id: i64 = row.get(0).map_err(|_| RetrievalError::Storage)?;
        let blob = row
            .get_ref(1)
            .map_err(|_| RetrievalError::Storage)?
            .as_blob()
            .map_err(|_| RetrievalError::Vector)?;
        if let Some((alternative, deadline)) = formulation {
            if Instant::now() < deadline {
                retain(&mut auxiliary, id, similarity(alternative, blob)?);
            } else {
                formulation = None;
                auxiliary.clear();
            }
        }
        retain(&mut best, id, similarity(vector, blob)?);
    }
    if formulation.is_some_and(|(_, deadline)| Instant::now() >= deadline) {
        auxiliary.clear();
    }
    Ok((
        materialize(database, profile, best, inspect)?,
        materialize(database, profile, auxiliary, inspect)?,
    ))
}

// Use the same read transaction as ranking to reopen the source bytes and vector.
fn materialize(
    database: &Connection,
    profile: &str,
    ranked: Vec<(i64, f32)>,
    inspect: bool,
) -> Result<DenseResult, RetrievalError> {
    let read = |_| RetrievalError::Storage;
    let mut details = database
        .prepare(
            "SELECT d.source, d.revision, d.kind, nullif(d.tracked_ref, ''),
                d.title, c.access_lost, d.source_location
           FROM documents d JOIN collections c ON c.name = d.collection WHERE d.id = ?1",
        )
        .map_err(read)?;
    let mut capture = inspect
        .then(|| {
            database.prepare(
                "SELECT d.content, v.embedding FROM documents d
            JOIN document_vectors v ON v.document_id = d.id WHERE d.id = ?1 AND v.profile = ?2",
            )
        })
        .transpose()
        .map_err(read)?;
    let mut hits = Vec::with_capacity(ranked.len());
    let mut matches = Vec::new();
    for (id, similarity) in ranked {
        let evidence = details.query_row([id], hit).map_err(read)?;
        if let Some(statement) = &mut capture {
            let (text, vector) = statement
                .query_row(rusqlite::params![id, profile], |row| {
                    let blob = row.get_ref(1)?.as_blob()?;
                    let vector = blob
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .copied()
                        .map(f32::from_le_bytes)
                        .collect();
                    Ok((row.get(0)?, vector))
                })
                .map_err(read)?;
            matches.push(VectorMatch {
                evidence: evidence.clone(),
                similarity,
                text,
                vector,
            });
        }
        hits.push(evidence);
    }
    Ok(DenseResult { hits, matches })
}

fn similarity(query: &Embedding, blob: &[u8]) -> Result<f32, RetrievalError> {
    if blob.len() != std::mem::size_of_val(query.values()) {
        return Err(RetrievalError::Vector);
    }
    let values = blob
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(f32::from_le_bytes);
    let score = query
        .values()
        .iter()
        .zip(values)
        .map(|(query, value)| query * value)
        .sum::<f32>();
    score
        .is_finite()
        .then_some(score)
        .ok_or(RetrievalError::Vector)
}

fn retain(ranking: &mut Vec<(i64, f32)>, id: i64, score: f32) {
    // Equal scores keep the original deterministic SQL order.
    let position = ranking.partition_point(|(_, previous)| previous.total_cmp(&score).is_ge());
    if position < MAX_RESULTS {
        if ranking.len() == MAX_RESULTS {
            ranking.pop();
        }
        ranking.insert(position, (id, score));
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_RESULTS, retain};

    #[test]
    fn equal_scores_keep_scan_order_at_the_cutoff() -> Result<(), std::num::TryFromIntError> {
        let mut ranking = Vec::new();
        let cutoff = i64::try_from(MAX_RESULTS)?;
        for id in (0..cutoff + 5).rev() {
            retain(&mut ranking, id, 0.75);
        }
        assert_eq!(ranking.len(), MAX_RESULTS);
        assert_eq!(ranking.first(), Some(&(cutoff + 4, 0.75)));
        assert_eq!(ranking.last(), Some(&(5, 0.75)));
        retain(&mut ranking, 100, 0.9);
        assert_eq!(ranking.first(), Some(&(100, 0.9)));
        assert_eq!(ranking.last(), Some(&(6, 0.75)));
        Ok(())
    }
}
