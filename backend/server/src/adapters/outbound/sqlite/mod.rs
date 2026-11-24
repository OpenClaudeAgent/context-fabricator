use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior, params};

use crate::application::{
    AnswerLanguage, CodeScope, Diagnostic, Failure, SearchHit, SearchQuery, Settings, Status,
    ports::Repository,
};

mod action_evidence;
mod answer_history;
mod corpus;
mod evidence;
mod github;
mod history_search;
mod inference_setup;
mod model_jobs;
mod models;
mod retrieval;
mod search;
mod snapshot;
mod sync;
pub mod verified_weights;

const CODE_COLLECTION: &str = "(SELECT collection FROM github_code_scopes
    WHERE repository_id = :repository_id AND branch = :branch)";

fn scope_parameters(scope: Option<&CodeScope>) -> (Option<i64>, Option<&str>) {
    scope.map_or((None, None), |scope| {
        (Some(scope.repository_id), Some(scope.branch.as_str()))
    })
}

const MIGRATIONS: &[&str] = &[
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
    include_str!("migrations/0014_model_inventory.sql"),
    include_str!("migrations/0015_github_history_lineage.sql"),
    include_str!("migrations/0016_github_history_evidence.sql"),
    include_str!("migrations/0017_github_actions.sql"),
    include_str!("migrations/0018_github_history_search.sql"),
    include_str!("migrations/0019_github_graph.sql"),
    include_str!("migrations/0020_github_storage.sql"),
    include_str!("migrations/0021_generation_selection.sql"),
    include_str!("migrations/0022_github_history_schedule.sql"),
    include_str!("migrations/0023_model_download_jobs.sql"),
    include_str!("migrations/0024_inference_artifact_limit.sql"),
    include_str!("migrations/0025_verified_weights.sql"),
    include_str!("migrations/0026_embedding_index_state.sql"),
    include_str!("migrations/0027_model_removals.sql"),
    include_str!("migrations/0028_inference_setup.sql"),
    include_str!("migrations/0029_inference_setup_resume.sql"),
    include_str!("migrations/0030_history_preparation.sql"),
    include_str!("migrations/0031_github_credential_cleanup.sql"),
    include_str!("migrations/0032_answer_history.sql"),
    include_str!("migrations/0033_answer_history_trace.sql"),
    include_str!("migrations/0034_github_job_traces.sql"),
    include_str!("migrations/0035_github_metadata_traces.sql"),
    include_str!("migrations/0036_answer_locale.sql"),
    include_str!("migrations/0037_document_vector_profiles.sql"),
    include_str!("migrations/0038_embedding_selection.sql"),
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
        let locale = settings.answer_locale;
        let language = locale.unwrap_or(settings.answer_language).legacy().code();
        let changed = transaction
            .execute(
                "UPDATE settings SET revision = revision + 1, answer_language = ?1,
                  diagnostic_retention_hours = ?2,
                  diagnostic_storage_limit_mb = coalesce(?3, diagnostic_storage_limit_mb),
                  inference_artifact_limit_gb = coalesce(?4, inference_artifact_limit_gb),
                  answer_locale = CASE
                      WHEN ?5 IS NOT NULL THEN ?5
                      WHEN answer_language != ?1 THEN NULL
                      ELSE answer_locale
                  END
                 WHERE revision = ?6",
                params![
                    language,
                    settings.diagnostic_retention_hours,
                    settings.diagnostic_storage_limit_mb,
                    settings.inference_artifact_limit_gb,
                    locale.map(AnswerLanguage::code),
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
        search::rank(self, query)
    }
}

fn read_settings(database: &Connection) -> rusqlite::Result<Settings> {
    database.query_row(
        "SELECT revision, answer_language, diagnostic_retention_hours, diagnostic_storage_limit_mb, inference_artifact_limit_gb, answer_locale
         FROM settings",
        [],
        |row| {
            let decode = |code: &str| {
                AnswerLanguage::from_code(code).ok_or(rusqlite::Error::InvalidQuery)
            };
            let answer_language = decode(&row.get::<_, String>(1)?)?;
            Ok(Settings {
                revision: row.get(0)?,
                answer_language,
                answer_locale: row.get::<_, Option<String>>(5)?.as_deref().map(decode).transpose()?,
                answer_language_options: None,
                diagnostic_retention_hours: row.get(2)?,
                diagnostic_storage_limit_mb: row.get(3)?,
                inference_artifact_limit_gb: row.get(4)?,
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
