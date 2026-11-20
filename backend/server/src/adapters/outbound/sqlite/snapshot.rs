//! Durable source manifests. Only publication writes to the searchable corpus.

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use super::SqliteStore;
use crate::application::ports::SnapshotStore;
use crate::domain::corpus::snapshot::{File, Phase, Plan, Snapshot, SnapshotError};

mod projection;

const MAX_PAGE_FILES: usize = 64;
const MAX_PAGES: u32 = 8_192;
const MAX_FILE_BYTES: usize = 256 * 1024;
const MAX_SNAPSHOT_FILES: i64 = 4_096;
const MAX_SNAPSHOT_BYTES: i64 = 32 * 1024 * 1024;
const SNAPSHOT_COLUMNS: &str =
    "id, revision, sequence, cursor, phase, collection, generation, predecessor, tracked_ref";

struct Stored {
    snapshot: Snapshot,
    collection: String,
    predecessor: Option<i64>,
}

impl SnapshotStore for SqliteStore {
    fn begin_snapshot(&self, plan: &Plan<'_>) -> Result<Snapshot, SnapshotError> {
        if plan.collection.is_empty() || plan.revision.is_empty() || plan.tracked_ref.is_empty() {
            return Err(SnapshotError::Invalid);
        }
        let mut writer = self.write().map_err(|_| SnapshotError::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        tx.execute(
            "INSERT INTO collections(name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
            [plan.collection],
        )
        .map_err(storage)?;
        guard_generation(&tx, plan.collection, plan.generation)?;
        let active = by_collection(&tx, plan.collection, true)?;
        if active.as_ref().map(|active| active.snapshot.id) != plan.predecessor {
            return Err(SnapshotError::Conflict);
        }
        if let Some(pending) = by_collection(&tx, plan.collection, false)? {
            return if pending.snapshot.revision == plan.revision
                && pending.snapshot.tracked_ref == plan.tracked_ref
            {
                Ok(pending.snapshot)
            } else {
                Err(SnapshotError::Conflict)
            };
        }
        if let Some(active) = active
            && active.snapshot.revision == plan.revision
            && active.snapshot.tracked_ref == plan.tracked_ref
            && active.snapshot.generation == plan.generation
        {
            return Ok(active.snapshot);
        }
        let owned = tx
            .execute(
                "UPDATE collections SET snapshot_mode = 1 WHERE name = ?1
             AND (snapshot_mode = 1 OR (cursor IS NULL AND NOT EXISTS
                 (SELECT 1 FROM documents WHERE collection = ?1)))",
                [plan.collection],
            )
            .map_err(storage)?;
        if owned != 1 {
            return Err(SnapshotError::Conflict);
        }
        let id: i64 = tx.query_row(
            "INSERT INTO source_snapshots (collection, generation, predecessor, revision, tracked_ref, phase)
             VALUES (?1, ?2, ?3, ?4, ?5, 'staging') RETURNING id",
            params![plan.collection, plan.generation, plan.predecessor, plan.revision, plan.tracked_ref],
            |row| row.get(0),
        ).map_err(storage)?;
        let created = by_id(&tx, id)?.snapshot;
        tx.commit().map_err(storage)?;
        Ok(created)
    }

    fn pending_snapshot(&self, collection: &str) -> Result<Option<Snapshot>, SnapshotError> {
        let database = self.read().map_err(|_| SnapshotError::Storage)?;
        Ok(by_collection(&database, collection, false)?.map(|stored| stored.snapshot))
    }

    fn active_snapshot(&self, collection: &str) -> Result<Option<Snapshot>, SnapshotError> {
        let database = self.read().map_err(|_| SnapshotError::Storage)?;
        Ok(by_collection(&database, collection, true)?.map(|stored| stored.snapshot))
    }

    fn has_source_blob(&self, collection: &str, blob: &str) -> Result<bool, SnapshotError> {
        self.read()
            .map_err(|_| SnapshotError::Storage)?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM source_blobs WHERE collection = ?1 AND blob = ?2)",
                params![collection, blob],
                |row| row.get(0),
            )
            .map_err(storage)
    }

    fn append_snapshot(
        &self,
        id: i64,
        sequence: u32,
        next_cursor: Option<&str>,
        files: &[File<'_>],
    ) -> Result<(), SnapshotError> {
        if files.len() > MAX_PAGE_FILES
            || sequence >= MAX_PAGES
            || (sequence == MAX_PAGES - 1 && next_cursor.is_some())
            || next_cursor.is_some_and(|cursor| cursor.is_empty() || cursor.len() > 4_096)
        {
            return Err(SnapshotError::Invalid);
        }
        let mut writer = self.write().map_err(|_| SnapshotError::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let stored = by_id(&tx, id)?;
        guard_generation(&tx, &stored.collection, stored.snapshot.generation)?;
        if stored.snapshot.phase != Phase::Staging || stored.snapshot.sequence != sequence {
            return Err(SnapshotError::Conflict);
        }
        for file in files {
            stage_file(&tx, id, &stored.collection, file)?;
        }
        let (count, bytes): (i64, i64) = tx
            .query_row(
                "SELECT count(*), coalesce(sum(length(CAST(b.content AS BLOB))), 0)
             FROM source_snapshot_files f JOIN source_blobs b USING (collection, blob)
             WHERE f.snapshot_id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(storage)?;
        if count > MAX_SNAPSHOT_FILES || bytes > MAX_SNAPSHOT_BYTES {
            return Err(SnapshotError::Invalid);
        }
        tx.execute(
            "UPDATE source_snapshots SET sequence = sequence + 1, cursor = ?2,
                 phase = CASE WHEN ?2 IS NULL THEN 'sealed' ELSE 'staging' END WHERE id = ?1",
            params![id, next_cursor],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)
    }

    fn publish_snapshot(&self, id: i64) -> Result<(), SnapshotError> {
        let mut writer = self.write().map_err(|_| SnapshotError::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let stored = by_id(&tx, id)?;
        guard_generation(&tx, &stored.collection, stored.snapshot.generation)?;
        if stored.snapshot.phase == Phase::Published {
            return Ok(());
        }
        let active = by_collection(&tx, &stored.collection, true)?;
        if stored.snapshot.phase != Phase::Sealed
            || active.map(|active| active.snapshot.id) != stored.predecessor
        {
            return Err(SnapshotError::Conflict);
        }

        projection::publish(&tx, &stored)?;
        tx.commit().map_err(storage)
    }

    fn discard_snapshot(&self, id: i64) -> Result<(), SnapshotError> {
        let mut writer = self.write().map_err(|_| SnapshotError::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let stored = by_id(&tx, id)?;
        guard_generation(&tx, &stored.collection, stored.snapshot.generation)?;
        if stored.snapshot.phase == Phase::Published {
            return Err(SnapshotError::Conflict);
        }
        tx.execute("DELETE FROM source_snapshots WHERE id = ?1", [id])
            .map_err(storage)?;
        projection::collect_unused_blobs(&tx, &stored.collection)?;
        tx.commit().map_err(storage)
    }
}

fn stage_file(
    tx: &Connection,
    id: i64,
    collection: &str,
    file: &File<'_>,
) -> Result<(), SnapshotError> {
    if file.source.is_empty()
        || file.source.len() > 2_048
        || file.path.len() > 1_024
        || file
            .path
            .split('/')
            .any(|part| matches!(part, "" | "." | ".."))
        || file.path.chars().any(char::is_control)
        || file.blob.is_empty()
        || file.blob.len() > 128
        || !file.file_url.starts_with("https://")
        || file.file_url.len() > 4_096
        || file.file_url.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(SnapshotError::Invalid);
    }
    if let Some(text) = file.text {
        if text.len() > MAX_FILE_BYTES || text.contains('\0') {
            return Err(SnapshotError::Invalid);
        }
        tx.execute(
            "INSERT INTO source_blobs (collection, blob, content) VALUES (?1, ?2, ?3)
             ON CONFLICT(collection, blob) DO NOTHING",
            params![collection, file.blob, text],
        )
        .map_err(storage)?;
    }
    let inserted = tx.execute(
        "INSERT INTO source_snapshot_files (snapshot_id, collection, path, source, blob, file_url)
         SELECT ?1, collection, ?3, ?4, blob, ?6 FROM source_blobs
         WHERE collection = ?2 AND blob = ?5 AND (?7 IS NULL OR content = ?7)
         ON CONFLICT DO NOTHING",
        params![id, collection, file.path, file.source, file.blob, file.file_url, file.text],
    ).map_err(storage)?;
    if inserted == 1 {
        Ok(())
    } else {
        Err(SnapshotError::Invalid)
    }
}

fn guard_generation(tx: &Connection, collection: &str, expected: u32) -> Result<(), SnapshotError> {
    let (generation, lost): (u32, bool) = tx
        .query_row(
            "SELECT generation, access_lost FROM collections WHERE name = ?1",
            [collection],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(storage)?;
    if generation != expected {
        return Err(SnapshotError::Conflict);
    }
    if lost {
        Err(SnapshotError::AccessLost)
    } else {
        Ok(())
    }
}

fn by_collection(
    tx: &Connection,
    collection: &str,
    published: bool,
) -> Result<Option<Stored>, SnapshotError> {
    let sql = format!(
        "SELECT {SNAPSHOT_COLUMNS} FROM source_snapshots WHERE collection = ?1 AND (phase = 'published') = ?2"
    );
    tx.query_row(&sql, params![collection, published], read_snapshot)
        .optional()
        .map_err(storage)
}

fn by_id(tx: &Connection, id: i64) -> Result<Stored, SnapshotError> {
    let sql = format!("SELECT {SNAPSHOT_COLUMNS} FROM source_snapshots WHERE id = ?1");
    tx.query_row(&sql, [id], read_snapshot)
        .optional()
        .map_err(storage)?
        .ok_or(SnapshotError::Conflict)
}

fn read_snapshot(row: &Row<'_>) -> rusqlite::Result<Stored> {
    let phase = match row.get_ref(4)?.as_str()? {
        "staging" => Phase::Staging,
        "sealed" => Phase::Sealed,
        "published" => Phase::Published,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(Stored {
        snapshot: Snapshot {
            id: row.get(0)?,
            revision: row.get(1)?,
            tracked_ref: row.get(8)?,
            generation: row.get(6)?,
            sequence: row.get(2)?,
            cursor: row.get(3)?,
            phase,
        },
        collection: row.get(5)?,
        predecessor: row.get(7)?,
    })
}

fn storage(_: rusqlite::Error) -> SnapshotError {
    SnapshotError::Storage
}
