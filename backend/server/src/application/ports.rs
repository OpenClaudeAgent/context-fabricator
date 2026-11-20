use super::{Diagnostic, Failure, SearchHit, SearchQuery, Settings, Status};
use crate::domain::corpus::snapshot::{File, Plan, Snapshot, SnapshotError};
use crate::domain::corpus::{Batch, Checkpoint, CollectionAccess, Commit};

pub trait Repository: Send + Sync {
    /// # Errors
    /// Returns a storage error when the status cannot be read.
    fn status(&self) -> Result<Status, Failure>;
    /// # Errors
    /// Returns a storage error when settings cannot be read.
    fn settings(&self) -> Result<Settings, Failure>;
    /// # Errors
    /// Returns a conflict for a stale revision or a storage error.
    fn save_settings(&self, settings: Settings) -> Result<Settings, Failure>;
    /// # Errors
    /// Returns a storage error when diagnostics cannot be read.
    fn diagnostics(&self) -> Result<Vec<Diagnostic>, Failure>;
    /// # Errors
    /// Returns a storage error when the lexical index cannot be read.
    fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>, Failure>;
}

/// Durable ingestion operations used by collection workers.
pub trait CorpusStore: Send + Sync {
    /// Returns the durable cursor, generation, and access state for an existing collection.
    ///
    /// # Errors
    /// Returns a storage error when the checkpoint cannot be read.
    fn checkpoint(&self, collection: &str) -> Result<Option<Checkpoint>, Failure>;

    /// Commits normalized documents, the required index and the next cursor together.
    ///
    /// # Errors
    /// Returns a storage error when the batch cannot be committed.
    fn commit_batch(&self, batch: &Batch<'_>) -> Result<Commit, Failure>;

    /// Changes access if the expected generation still owns this collection. A change
    /// invalidates earlier work while preserving the local copy and cursor.
    ///
    /// # Errors
    /// Returns a storage error when the state cannot be saved.
    fn set_collection_access(
        &self,
        collection: &str,
        generation: u32,
        access: CollectionAccess,
    ) -> Result<bool, Failure>;

    /// Removes this scope's documents from retrieval and invalidates outstanding work.
    ///
    /// # Errors
    /// Returns a storage error when deletion cannot be committed.
    fn delete_collection(&self, collection: &str) -> Result<(), Failure>;
}

/// Publication boundary for a complete immutable repository snapshot.
pub trait SnapshotStore: Send + Sync {
    /// # Errors
    /// Returns a conflict when the generation or publication predecessor changed.
    fn begin_snapshot(&self, plan: &Plan<'_>) -> Result<Snapshot, SnapshotError>;

    /// # Errors
    /// Returns a storage error when the unfinished snapshot cannot be read.
    fn pending_snapshot(&self, collection: &str) -> Result<Option<Snapshot>, SnapshotError>;

    /// # Errors
    /// Returns a storage error when the current publication cannot be read.
    fn active_snapshot(&self, collection: &str) -> Result<Option<Snapshot>, SnapshotError>;

    /// # Errors
    /// Returns a storage error when an owned source blob cannot be checked.
    fn has_source_blob(&self, collection: &str, blob: &str) -> Result<bool, SnapshotError>;

    /// # Errors
    /// Rejects stale pages and invalid entries. A page with no next cursor seals the manifest.
    fn append_snapshot(
        &self,
        id: i64,
        sequence: u32,
        next_cursor: Option<&str>,
        files: &[File<'_>],
    ) -> Result<(), SnapshotError>;

    /// # Errors
    /// Returns a conflict when the stage is incomplete, stale, or no longer authorized.
    fn publish_snapshot(&self, id: i64) -> Result<(), SnapshotError>;

    /// # Errors
    /// Rejects stale or published IDs. A failed stage can be discarded without altering current search.
    fn discard_snapshot(&self, id: i64) -> Result<(), SnapshotError>;
}
