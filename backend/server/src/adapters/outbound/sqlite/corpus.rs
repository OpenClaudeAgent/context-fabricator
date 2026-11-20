use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{SqliteStore, storage_failure};
use crate::application::{Failure, ports::CorpusStore};
use crate::domain::corpus::{Batch, Checkpoint, CollectionAccess, Commit};

impl CorpusStore for SqliteStore {
    fn checkpoint(&self, collection: &str) -> Result<Option<Checkpoint>, Failure> {
        read_checkpoint(&*self.read()?, collection).map_err(storage_failure)
    }

    fn commit_batch(&self, batch: &Batch<'_>) -> Result<Commit, Failure> {
        let mut database = self.write()?;
        let transaction = database
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_failure)?;
        transaction
            .execute(
                "INSERT INTO collections(name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
                [batch.collection],
            )
            .map_err(storage_failure)?;
        let checkpoint = read_checkpoint(&transaction, batch.collection)
            .map_err(storage_failure)?
            .ok_or(Failure::Storage)?;
        if transaction
            .query_row(
                "SELECT snapshot_mode FROM collections WHERE name = ?1",
                [batch.collection],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_failure)?
        {
            return Ok(Commit::InvalidProvenance);
        }
        if checkpoint.generation != batch.generation {
            return Ok(Commit::StaleCheckpoint);
        }
        if checkpoint.access == CollectionAccess::Lost {
            return Ok(Commit::AccessLost);
        }
        if checkpoint.cursor.as_deref() == Some(batch.next_cursor) {
            return Ok(Commit::AlreadyIndexed);
        }
        if checkpoint.cursor.as_deref() != batch.previous_cursor {
            return Ok(Commit::StaleCheckpoint);
        }
        for document in batch.documents {
            let (kind, tracked_ref) = document.kind.parts();
            if document.source.is_empty()
                || document.revision.is_empty()
                || (kind == "current" && tracked_ref.is_empty())
            {
                return Ok(Commit::InvalidProvenance);
            }
            if kind == "current" {
                transaction
                    .execute(
                        "DELETE FROM documents
                         WHERE collection = ?1 AND source = ?2 AND kind = 'current'
                           AND tracked_ref = ?3 AND revision <> ?4",
                        params![
                            batch.collection,
                            document.source,
                            tracked_ref,
                            document.revision
                        ],
                    )
                    .map_err(storage_failure)?;
            }
            transaction
                .execute(
                    "INSERT INTO documents
                        (collection, source, kind, tracked_ref, revision, title, content)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(collection, source, kind, tracked_ref, revision, passage_start) DO UPDATE
                     SET title = excluded.title, content = excluded.content
                     WHERE title IS NOT excluded.title OR content IS NOT excluded.content",
                    params![
                        batch.collection,
                        document.source,
                        kind,
                        tracked_ref,
                        document.revision,
                        document.title,
                        document.content
                    ],
                )
                .map_err(storage_failure)?;
        }
        transaction
            .execute(
                "UPDATE collections SET cursor = ?1 WHERE name = ?2",
                params![batch.next_cursor, batch.collection],
            )
            .map_err(storage_failure)?;
        transaction.commit().map_err(storage_failure)?;
        Ok(Commit::Indexed)
    }

    fn set_collection_access(
        &self,
        collection: &str,
        generation: u32,
        access: CollectionAccess,
    ) -> Result<bool, Failure> {
        let access_lost = access == CollectionAccess::Lost;
        self.write()?
            .execute(
                "UPDATE collections SET access_lost = ?3, generation = generation + 1
                 WHERE name = ?1 AND generation = ?2 AND access_lost <> ?3",
                params![collection, generation, access_lost],
            )
            .map(|count| count == 1)
            .map_err(storage_failure)
    }

    fn delete_collection(&self, collection: &str) -> Result<(), Failure> {
        let mut database = self.write()?;
        let transaction = database
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_failure)?;
        transaction
            .execute(
                "INSERT INTO collections(name, generation) VALUES (?1, 1)
                 ON CONFLICT(name) DO UPDATE SET generation = generation + 1, cursor = NULL",
                [collection],
            )
            .map_err(storage_failure)?;
        transaction
            .execute("DELETE FROM documents WHERE collection = ?1", [collection])
            .map_err(storage_failure)?;
        transaction
            .execute(
                "DELETE FROM source_snapshots WHERE collection = ?1",
                [collection],
            )
            .map_err(storage_failure)?;
        transaction
            .execute(
                "DELETE FROM source_blobs WHERE collection = ?1",
                [collection],
            )
            .map_err(storage_failure)?;
        transaction.commit().map_err(storage_failure)
    }
}

fn read_checkpoint(
    database: &Connection,
    collection: &str,
) -> rusqlite::Result<Option<Checkpoint>> {
    database
        .query_row(
            "SELECT generation, cursor, access_lost FROM collections WHERE name = ?1",
            [collection],
            |row| {
                Ok(Checkpoint {
                    generation: row.get(0)?,
                    cursor: row.get(1)?,
                    access: if row.get(2)? {
                        CollectionAccess::Lost
                    } else {
                        CollectionAccess::Available
                    },
                })
            },
        )
        .optional()
}
