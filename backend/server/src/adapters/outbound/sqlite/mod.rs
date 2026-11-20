use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior, named_params, params};

use crate::application::{
    AnswerLanguage, CodeScope, Diagnostic, Failure, SearchHit, SearchQuery, Settings, Status,
    ports::Repository,
};
use retrieval::{CANDIDATE_COLUMNS, hit};

mod corpus;
mod evidence;
mod github;
mod retrieval;
mod snapshot;
mod sync;

const CODE_COLLECTION: &str = "(SELECT collection FROM github_code_scopes
    WHERE repository_id = :repository_id AND branch = :branch)";

fn scope_parameters(scope: Option<&CodeScope>) -> (Option<i64>, Option<&str>) {
    scope.map_or((None, None), |scope| {
        (Some(scope.repository_id), Some(scope.branch.as_str()))
    })
}

const MIGRATIONS: [&str; 13] = [
    include_str!("migrations/0001_foundation.sql"),
    include_str!("migrations/0002_corpus.sql"),
    include_str!("migrations/0003_sync.sql"),
    include_str!("migrations/0004_retrieval.sql"),
    include_str!("migrations/0005_canonical_lexical.sql"),
    include_str!("migrations/0006_diagnostic_storage.sql"),
    include_str!("migrations/0007_sync_job_generation.sql"),
    include_str!("migrations/0008_github.sql"),
    include_str!("migrations/0009_provider_pacing.sql"),
    include_str!("migrations/0010_source_passages.sql"),
    include_str!("migrations/0011_source_snapshots.sql"),
    include_str!("migrations/0012_github_code_scopes.sql"),
    include_str!("migrations/0013_github_collection_jobs.sql"),
];
const MAX_DIAGNOSTICS: u16 = 500;

pub struct SqliteStore {
    writer: Mutex<Connection>,
    reader: Option<Mutex<Connection>>,
}

impl SqliteStore {
    /// Opens the application store and applies its migrations.
    ///
    /// # Errors
    /// Returns an error if the directory, database or migration is unavailable.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let mut database = Connection::open(path)?;
        database.busy_timeout(Duration::from_secs(1))?;
        database.pragma_update(None, "foreign_keys", true)?;
        let on_disk = path != Path::new(":memory:");
        if on_disk {
            database.pragma_update(None, "journal_mode", "WAL")?;
            database.pragma_update(None, "synchronous", "FULL")?;
        }
        let transaction = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let schema: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let schema = usize::try_from(schema).map_err(|_| rusqlite::Error::InvalidQuery)?;
        let pending = MIGRATIONS
            .get(schema..)
            .ok_or(rusqlite::Error::InvalidQuery)?;
        for migration in pending {
            transaction.execute_batch(migration)?;
        }
        if !pending.is_empty() {
            let version =
                i64::try_from(MIGRATIONS.len()).map_err(|_| rusqlite::Error::InvalidQuery)?;
            transaction.pragma_update(None, "user_version", version)?;
        }
        transaction.commit()?;
        let reader = if on_disk {
            let reader = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            reader.busy_timeout(Duration::from_secs(1))?;
            Some(Mutex::new(reader))
        } else {
            None
        };
        Ok(Self {
            writer: Mutex::new(database),
            reader,
        })
    }

    /// Appends a bounded application event.
    ///
    /// # Errors
    /// Returns a storage error if the event cannot be saved.
    pub fn record(&self, code: &'static str, message: &str) -> Result<(), Failure> {
        let mut database = self.write()?;
        let transaction = database.transaction().map_err(storage_failure)?;
        record(&transaction, code, message).map_err(storage_failure)?;
        transaction.commit().map_err(storage_failure)
    }

    fn write(&self) -> Result<MutexGuard<'_, Connection>, Failure> {
        self.writer.lock().map_err(|_| Failure::Storage)
    }

    fn read(&self) -> Result<MutexGuard<'_, Connection>, Failure> {
        self.reader
            .as_ref()
            .unwrap_or(&self.writer)
            .lock()
            .map_err(|_| Failure::Storage)
    }
}

impl Repository for SqliteStore {
    fn status(&self) -> Result<Status, Failure> {
        let database = self.read()?;
        let indexed_documents = database.query_row(
                "SELECT count(*) FROM (SELECT DISTINCT source, revision, kind, tracked_ref FROM documents)",
                [],
                |row| row.get(0),
            )
            .map_err(storage_failure)?;
        let (outbound_attempts, outbound_pending, outbound_uncertain) = database
            .query_row(
                "SELECT coalesce(sum(attempts), 0), coalesce(sum(pending), 0),
                        coalesce(sum(uncertain), 0) FROM outbound_buckets",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(storage_failure)?;
        Ok(Status {
            status: "ready",
            api_version: 1,
            connected_sources: u64::try_from(
                database
                    .query_row(
                        "SELECT count(*) FROM github_repositories WHERE verified = 1",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(storage_failure)?,
            )
            .map_err(|_| Failure::Storage)?,
            collected_documents: indexed_documents,
            indexed_documents,
            outbound_attempts,
            outbound_pending,
            outbound_uncertain,
            inference: "notConfigured",
            retrieval: None,
        })
    }

    fn settings(&self) -> Result<Settings, Failure> {
        read_settings(&*self.read()?).map_err(storage_failure)
    }

    fn save_settings(&self, settings: Settings) -> Result<Settings, Failure> {
        settings.validate()?;
        let mut database = self.write()?;
        let transaction = database.transaction().map_err(storage_failure)?;
        let language = match settings.answer_language {
            AnswerLanguage::Auto => "auto",
            AnswerLanguage::En => "en",
            AnswerLanguage::Fr => "fr",
        };
        let changed = transaction
            .execute(
                "UPDATE settings SET revision = revision + 1, answer_language = ?1,
                  diagnostic_retention_hours = ?2,
                  diagnostic_storage_limit_mb = coalesce(?3, diagnostic_storage_limit_mb)
                 WHERE revision = ?4",
                params![
                    language,
                    settings.diagnostic_retention_hours,
                    settings.diagnostic_storage_limit_mb,
                    settings.revision
                ],
            )
            .map_err(storage_failure)?;
        if changed == 0 {
            return Err(Failure::Conflict);
        }
        record(
            &transaction,
            "settings_saved",
            "Application settings saved.",
        )
        .map_err(storage_failure)?;
        let effective = read_settings(&transaction).map_err(storage_failure)?;
        transaction.commit().map_err(storage_failure)?;
        Ok(effective)
    }

    fn diagnostics(&self) -> Result<Vec<Diagnostic>, Failure> {
        let database = self.read()?;
        let mut statement = database
            .prepare(
                "SELECT id, timestamp, code, message FROM diagnostics
                 WHERE timestamp >= unixepoch() - (SELECT diagnostic_retention_hours FROM settings) * 3600
                 ORDER BY id DESC LIMIT 50",
            )
            .map_err(storage_failure)?;
        let rows = statement
            .query_map([], |row| {
                Ok(Diagnostic {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    code: row.get(2)?,
                    message: row.get(3)?,
                })
            })
            .map_err(storage_failure)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_failure)
    }

    fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>, Failure> {
        let expression = query
            .terms()
            .iter()
            .map(|term| format!("\"{term}\""))
            .collect::<Vec<_>>()
            .join(" OR ");
        let database = self.read()?;
        let (repository_id, branch) = scope_parameters(query.scope());
        // Each FTS row represents one immutable passage. Return the requested
        // owner's projection even if a different owner supplied that FTS row.
        let sql = format!(
                "SELECT {CANDIDATE_COLUMNS}
                 FROM documents_fts f
                 JOIN documents canonical ON canonical.id = f.rowid
                 JOIN documents d ON d.id = CASE WHEN :repository_id IS NULL THEN canonical.id
                     ELSE (SELECT owned.id FROM documents owned
                           WHERE owned.collection = {CODE_COLLECTION}
                             AND owned.publication_id IS NOT NULL
                             AND (owned.source, owned.kind, owned.tracked_ref, owned.revision, owned.passage_start)
                               = (canonical.source, canonical.kind, canonical.tracked_ref, canonical.revision, canonical.passage_start))
                     END
                 JOIN collections c ON c.name = d.collection
                 WHERE documents_fts MATCH :expression
                 ORDER BY bm25(documents_fts), d.source, d.revision, d.kind, d.tracked_ref, d.passage_start LIMIT 20"
        );
        let mut statement = database.prepare(&sql).map_err(storage_failure)?;
        let rows = statement
            .query_map(named_params! {":expression": expression, ":repository_id": repository_id, ":branch": branch}, hit)
            .map_err(storage_failure)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_failure)
    }
}

fn read_settings(database: &Connection) -> rusqlite::Result<Settings> {
    database.query_row(
        "SELECT revision, answer_language, diagnostic_retention_hours, diagnostic_storage_limit_mb
         FROM settings",
        [],
        |row| {
            let answer_language = match row.get::<_, String>(1)?.as_str() {
                "auto" => AnswerLanguage::Auto,
                "en" => AnswerLanguage::En,
                "fr" => AnswerLanguage::Fr,
                _ => return Err(rusqlite::Error::InvalidQuery),
            };
            Ok(Settings {
                revision: row.get(0)?,
                answer_language,
                diagnostic_retention_hours: row.get(2)?,
                diagnostic_storage_limit_mb: row.get(3)?,
            })
        },
    )
}

fn record(database: &Connection, code: &str, message: &str) -> rusqlite::Result<()> {
    database.execute(
        "INSERT INTO diagnostics (timestamp, code, message) VALUES (unixepoch(), ?1, ?2)",
        [code, message],
    )?;
    database.execute(
        "DELETE FROM diagnostics
         WHERE timestamp < unixepoch() - (SELECT diagnostic_retention_hours FROM settings) * 3600
         OR id <= (SELECT max(id) FROM diagnostics) - ?1",
        [MAX_DIAGNOSTICS],
    )?;
    Ok(())
}

fn storage_failure(_error: rusqlite::Error) -> Failure {
    tracing::warn!(
        code = "storage_unavailable",
        "A local database operation failed"
    );
    Failure::Storage
}
