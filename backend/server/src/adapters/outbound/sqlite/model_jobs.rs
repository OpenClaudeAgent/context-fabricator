//! Durable model download intent; paths and transport details are not persisted.
use super::SqliteStore;
use crate::application::inference_setup::{SetupIntent, SetupLease};
use crate::application::model_management::{DownloadJobStore, RemovalIntent, StoredDownload};
use crate::domain::model::DownloadError;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

fn authorize_setup_job(
    db: &Connection,
    job: &StoredDownload,
    lease: SetupLease,
    expected_previous_job: Option<&str>,
) -> Result<(), DownloadError> {
    let row = db.query_row(
        "SELECT phase, intent, selection_revision, embedding_epoch FROM inference_setups WHERE operation_id = ?1 AND lease_revision = ?2 AND phase IN ('answer_weights','embedding_weights')",
        params![lease.id.to_string(), lease.revision],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?, row.get::<_, Option<i64>>(3)?)),
    ).optional().map_err(|_| DownloadError::Unavailable)?.ok_or(DownloadError::InUse)?;
    let (phase, json, selection_revision, embedding_epoch) = row;
    let intent: SetupIntent =
        serde_json::from_str(&json).map_err(|_| DownloadError::Unavailable)?;
    if intent.version != SetupIntent::VERSION {
        return Err(DownloadError::InUse);
    }
    let asset = if phase == "answer_weights" {
        &intent.answer
    } else {
        intent.embedding.as_ref().ok_or(DownloadError::InUse)?
    };
    if asset.profile != job.profile
        || asset.sha256 != job.fingerprint
        || asset.previous_job.as_deref() != expected_previous_job
    {
        return Err(DownloadError::InUse);
    }
    let unchanged: bool = if phase == "answer_weights" {
        db.query_row("SELECT coalesce((SELECT revision FROM generation_selection WHERE singleton = 1), 0) = ?1", [selection_revision], |row| row.get(0))
    } else {
        db.query_row("SELECT ?1 IS NOT NULL AND ?1 = coalesce((SELECT epoch FROM embedding_index_state WHERE profile = ?2), 0)", params![embedding_epoch, super::inference_setup::selected_embedding(db).map_err(|_| DownloadError::Unavailable)?], |row| row.get(0))
    }.map_err(|_| DownloadError::Unavailable)?;
    if !unchanged {
        return Err(DownloadError::InUse);
    }
    Ok(())
}

impl DownloadJobStore for SqliteStore {
    fn load_download_jobs(&self) -> Result<Vec<StoredDownload>, DownloadError> {
        let connection = self.read().map_err(|_| DownloadError::Unavailable)?;
        let mut statement = connection
            .prepare(
                "SELECT profile, fingerprint, operation_id, phase, error FROM model_download_jobs",
            )
            .map_err(|_| DownloadError::Unavailable)?;
        let rows = statement
            .query_map([], |row| {
                Ok(StoredDownload {
                    profile: row.get(0)?,
                    fingerprint: row.get(1)?,
                    operation_id: row.get(2)?,
                    phase: row.get(3)?,
                    error: row.get(4)?,
                })
            })
            .map_err(|_| DownloadError::Unavailable)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| DownloadError::Unavailable)
    }
    fn save_download_job(&self, job: &StoredDownload) -> Result<(), DownloadError> {
        self.write().map_err(|_| DownloadError::Unavailable)?.execute(
            "INSERT INTO model_download_jobs (profile, fingerprint, operation_id, phase, error) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(profile) DO UPDATE SET fingerprint=excluded.fingerprint, operation_id=excluded.operation_id, phase=excluded.phase, error=excluded.error",
            params![job.profile, job.fingerprint, job.operation_id, job.phase, job.error],
        ).map_err(|_| DownloadError::Unavailable)?;
        Ok(())
    }
    fn save_setup_job(
        &self,
        job: &StoredDownload,
        lease: SetupLease,
        expected_previous_job: Option<&str>,
    ) -> Result<(), DownloadError> {
        if job.operation_id != lease.id.to_string() || job.phase != "running" {
            return Err(DownloadError::InUse);
        }
        let mut db = self.write().map_err(|_| DownloadError::Unavailable)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| DownloadError::Unavailable)?;
        authorize_setup_job(&tx, job, lease, expected_previous_job)?;
        let owned: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM inference_setup_model_receipts WHERE operation_id = ?1 AND profile = ?2)", params![lease.id.to_string(), job.profile], |row| row.get(0)).map_err(|_| DownloadError::Unavailable)?;
        let existing = tx
            .query_row(
                "SELECT operation_id, phase, fingerprint FROM model_download_jobs WHERE profile = ?1",
                [&job.profile],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
            )
            .optional()
            .map_err(|_| DownloadError::Unavailable)?;
        if owned {
            if !existing.as_ref().is_some_and(|(id, phase, fingerprint)| {
                id == &job.operation_id
                    && fingerprint == &job.fingerprint
                    && matches!(phase.as_str(), "running" | "interrupted")
            }) {
                return Err(DownloadError::InUse);
            }
        } else if existing.as_ref().map(|(id, _, _)| id.as_str()) != expected_previous_job
            || existing
                .as_ref()
                .is_some_and(|(_, phase, _)| !matches!(phase.as_str(), "interrupted" | "failed"))
        {
            return Err(DownloadError::InUse);
        }
        tx.execute("INSERT INTO model_download_jobs (profile, fingerprint, operation_id, phase, error) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(profile) DO UPDATE SET fingerprint=excluded.fingerprint, operation_id=excluded.operation_id, phase=excluded.phase, error=excluded.error", params![job.profile, job.fingerprint, job.operation_id, job.phase, job.error]).map_err(|_| DownloadError::Unavailable)?;
        tx.execute("INSERT OR IGNORE INTO inference_setup_model_receipts (operation_id, profile) VALUES (?1, ?2)", params![lease.id.to_string(), job.profile]).map_err(|_| DownloadError::Unavailable)?;
        tx.commit().map_err(|_| DownloadError::Unavailable)
    }
    fn remove_download_job(&self, profile: &str) -> Result<(), DownloadError> {
        self.write()
            .map_err(|_| DownloadError::Unavailable)?
            .execute(
                "DELETE FROM model_download_jobs WHERE profile = ?1",
                [profile],
            )
            .map_err(|_| DownloadError::Unavailable)?;
        Ok(())
    }
    fn load_removals(&self) -> Result<Vec<RemovalIntent>, DownloadError> {
        let connection = self.read().map_err(|_| DownloadError::Unavailable)?;
        let mut statement = connection
            .prepare("SELECT profile, fingerprint, identity FROM model_removals")
            .map_err(|_| DownloadError::Unavailable)?;
        let rows = statement
            .query_map([], |row| {
                Ok(RemovalIntent {
                    profile: row.get(0)?,
                    fingerprint: row.get(1)?,
                    identity: row.get(2)?,
                })
            })
            .map_err(|_| DownloadError::Unavailable)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| DownloadError::Unavailable)
    }
    fn save_removal(&self, intent: &RemovalIntent) -> Result<(), DownloadError> {
        self.write().map_err(|_| DownloadError::Unavailable)?.execute("INSERT INTO model_removals (profile, fingerprint, identity) VALUES (?1, ?2, ?3) ON CONFLICT(profile) DO UPDATE SET fingerprint=excluded.fingerprint, identity=excluded.identity", params![intent.profile, intent.fingerprint, intent.identity]).map_err(|_| DownloadError::Unavailable)?;
        Ok(())
    }
    fn clear_removal(&self, profile: &str) -> Result<(), DownloadError> {
        self.write()
            .map_err(|_| DownloadError::Unavailable)?
            .execute("DELETE FROM model_removals WHERE profile = ?1", [profile])
            .map_err(|_| DownloadError::Unavailable)?;
        Ok(())
    }
}
