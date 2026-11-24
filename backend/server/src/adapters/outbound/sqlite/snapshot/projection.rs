//! Project one sealed manifest into current search rows and retained embeddings.

use ctxfab_inference::MAX_PASSAGE_BYTES;
use rusqlite::{Connection, params};

use super::{SnapshotError, Stored, storage};
use crate::domain::corpus::{passage::passages, snapshot::Location};

const TARGET_PASSAGE_BYTES: usize = 2_048;

pub(super) fn publish(
    tx: &Connection,
    stored: &Stored,
    general: bool,
) -> Result<(), SnapshotError> {
    verify_immutable_sources(tx, stored)?;
    if general {
        index_files(tx, stored)?;
    }
    replace_publication(tx, stored)
}

fn verify_immutable_sources(tx: &Connection, stored: &Stored) -> Result<(), SnapshotError> {
    let conflict = tx
        .query_row(
            "SELECT EXISTS (
            SELECT 1 FROM source_snapshot_files candidate
            JOIN source_snapshot_files previous ON previous.source = candidate.source
            JOIN source_snapshots active ON active.id = previous.snapshot_id
            JOIN source_blobs previous_blob ON (previous_blob.collection, previous_blob.blob) =
                                               (previous.collection, previous.blob)
            JOIN source_blobs candidate_blob ON (candidate_blob.collection, candidate_blob.blob) =
                                                (candidate.collection, candidate.blob)
            WHERE candidate.snapshot_id = ?1 AND active.phase = 'published'
              AND active.revision = ?2
              AND (candidate.path <> previous.path OR candidate.blob <> previous.blob
                   OR candidate_blob.content <> previous_blob.content)
         )",
            params![stored.snapshot.id, stored.snapshot.revision],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage)?;
    if conflict {
        Err(SnapshotError::Invalid)
    } else {
        Ok(())
    }
}

fn index_files(tx: &Connection, stored: &Stored) -> Result<(), SnapshotError> {
    let mut read = tx
        .prepare(
            "SELECT f.source, f.path, f.blob, f.file_url, b.content
         FROM source_snapshot_files f JOIN source_blobs b USING (collection, blob)
         WHERE f.snapshot_id = ?1 ORDER BY f.path",
        )
        .map_err(storage)?;
    let mut files = read.query([stored.snapshot.id]).map_err(storage)?;
    let mut insert = tx
        .prepare(
            "INSERT INTO documents
             (collection, source, revision, kind, tracked_ref, title, content, passage_start,
              source_location, publication_id)
         VALUES (?1, ?2, ?3, 'current', ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(collection, source, kind, tracked_ref, revision, passage_start) DO UPDATE
         SET publication_id = excluded.publication_id, source_location = excluded.source_location",
        )
        .map_err(storage)?;
    while let Some(file) = files.next().map_err(storage)? {
        let source: String = file.get(0).map_err(storage)?;
        let path: String = file.get(1).map_err(storage)?;
        let blob: String = file.get(2).map_err(storage)?;
        let file_url: String = file.get(3).map_err(storage)?;
        let content: String = file.get(4).map_err(storage)?;
        let budget = MAX_PASSAGE_BYTES
            .saturating_sub(path.len() + 1)
            .min(TARGET_PASSAGE_BYTES);
        for passage in passages(&content, budget).ok_or(SnapshotError::Invalid)? {
            if passage.text.trim().is_empty() {
                continue;
            }
            let start_byte = coordinate(passage.bytes.start)?;
            let location = serde_json::to_string(&Location {
                path: path.clone(),
                blob: blob.clone(),
                file_url: file_url.clone(),
                start_byte,
                end_byte: coordinate(passage.bytes.end)?,
                start_line: coordinate(passage.start_line)?,
                end_line: coordinate(passage.end_line)?,
            })
            .map_err(|_| SnapshotError::Storage)?;
            insert
                .execute(params![
                    stored.collection,
                    source,
                    stored.snapshot.revision,
                    stored.snapshot.tracked_ref,
                    path,
                    passage.text,
                    start_byte,
                    location,
                    stored.snapshot.id,
                ])
                .map_err(storage)?;
        }
    }
    Ok(())
}

fn replace_publication(tx: &Connection, stored: &Stored) -> Result<(), SnapshotError> {
    let identity = params![stored.collection, stored.snapshot.id];
    // Passage coordinates can shift. Reuse each profile's earliest matching input.
    tx.execute(
        "WITH retained AS (
             SELECT replacement.id AS document_id, v.profile, min(previous.id) AS previous_id
             FROM documents replacement
             JOIN documents previous
               ON previous.collection = replacement.collection AND previous.source = replacement.source
              AND previous.title = replacement.title AND previous.content = replacement.content
              AND previous.publication_id = ?3
             JOIN document_vectors v ON v.document_id = previous.id
             WHERE replacement.collection = ?1 AND replacement.publication_id = ?2
             GROUP BY replacement.id, v.profile
         )
         INSERT INTO document_vectors (document_id, profile, embedding)
         SELECT retained.document_id, v.profile, v.embedding
         FROM retained JOIN document_vectors v
           ON (v.document_id, v.profile) = (retained.previous_id, retained.profile)
         WHERE true
         ON CONFLICT(document_id, profile) DO NOTHING",
        params![stored.collection, stored.snapshot.id, stored.predecessor],
    ).map_err(storage)?;
    tx.execute(
        "DELETE FROM documents WHERE collection = ?1 AND publication_id <> ?2",
        identity,
    )
    .map_err(storage)?;
    tx.execute(
        "DELETE FROM source_snapshots WHERE id = ?1",
        [stored.predecessor],
    )
    .map_err(storage)?;
    tx.execute(
        "UPDATE source_snapshots SET phase = 'published',
            published_at_ms = CAST(unixepoch('subsec') * 1000 AS INTEGER) WHERE id = ?1",
        [stored.snapshot.id],
    )
    .map_err(storage)?;
    tx.execute(
        "UPDATE collections SET cursor = ?2 WHERE name = ?1",
        params![stored.collection, stored.snapshot.revision],
    )
    .map_err(storage)?;
    collect_unused_blobs(tx, &stored.collection)
}

pub(super) fn collect_unused_blobs(tx: &Connection, collection: &str) -> Result<(), SnapshotError> {
    tx.execute(
        "DELETE FROM source_blobs WHERE collection = ?1 AND NOT EXISTS (
            SELECT 1 FROM source_snapshot_files
            WHERE collection = source_blobs.collection AND blob = source_blobs.blob
         )",
        [collection],
    )
    .map_err(storage)?;
    Ok(())
}

fn coordinate(value: usize) -> Result<u32, SnapshotError> {
    u32::try_from(value).map_err(|_| SnapshotError::Invalid)
}
