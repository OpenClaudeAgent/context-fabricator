//! Stores confirmed setup operations and atomically fences their generation selection.
use ctxfab_inference::generation::SelectionStoreError;
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use uuid::Uuid;

use super::{
    SqliteStore,
    models::save_selection,
    retrieval::{index_state, set_index_enabled},
};
use crate::application::inference_setup::{
    Claim, JournalError, SetupIntent, SetupJournal, SetupLease, SetupPhase, SetupRun, SetupStatus,
};
use crate::application::retrieval::IndexState;

fn phase_name(phase: SetupPhase) -> &'static str {
    match phase {
        SetupPhase::Runtime | SetupPhase::WaitingForArchive => "runtime",
        SetupPhase::AnswerWeights => "answer_weights",
        SetupPhase::ActivatingAnswer => "activating_answer",
        SetupPhase::EmbeddingWeights => "embedding_weights",
        SetupPhase::Completed => "completed",
        SetupPhase::Failed => "failed",
        SetupPhase::Interrupted => "interrupted",
    }
}

fn status(row: &Row<'_>) -> rusqlite::Result<SetupStatus> {
    let operation_id: String = row.get(0)?;
    let phase: String = row.get(1)?;
    let waiting: bool = row.get(4)?;
    let phase = match phase.as_str() {
        "runtime" if waiting => SetupPhase::WaitingForArchive,
        "runtime" => SetupPhase::Runtime,
        "answer_weights" => SetupPhase::AnswerWeights,
        "activating_answer" => SetupPhase::ActivatingAnswer,
        "embedding_weights" => SetupPhase::EmbeddingWeights,
        "completed" => SetupPhase::Completed,
        "failed" => SetupPhase::Failed,
        "interrupted" => SetupPhase::Interrupted,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    Ok(SetupStatus {
        operation_id: Uuid::parse_str(&operation_id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        phase,
        answer_ready: row.get(2)?,
        error: row.get(3)?,
        selection: row
            .get::<_, Option<String>>(5)?
            .and_then(|intent| serde_json::from_str::<SetupIntent>(&intent).ok())
            .map(|intent| intent.selection),
    })
}

fn existing(
    db: &Connection,
    id: Uuid,
    fingerprint: &str,
) -> Result<Option<SetupStatus>, JournalError> {
    let row = db.query_row(
        "SELECT operation_id, phase, answer_ready, error, waiting_archive, intent, fingerprint FROM inference_setups WHERE operation_id = ?1",
        [id.to_string()],
        |row| Ok((status(row)?, row.get::<_, String>(6)?)),
    ).optional().map_err(|_| JournalError::Storage)?;
    match row {
        Some((status, stored)) if stored == fingerprint => Ok(Some(status)),
        Some(_) => Err(JournalError::Conflict),
        None => Ok(None),
    }
}

impl SetupJournal for SqliteStore {
    fn recover(&self) -> Result<Option<SetupRun>, JournalError> {
        let mut db = self.write().map_err(|_| JournalError::Storage)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| JournalError::Storage)?;
        tx.execute("UPDATE inference_setups SET phase = 'interrupted', waiting_archive = 0, lease_revision = lease_revision + 1 WHERE phase NOT IN ('completed', 'failed', 'interrupted') AND intent IS NULL", []).map_err(|_| JournalError::Storage)?;
        tx.execute("UPDATE inference_setups SET lease_revision = lease_revision + 1 WHERE phase NOT IN ('completed', 'failed', 'interrupted') AND intent IS NOT NULL", []).map_err(|_| JournalError::Storage)?;
        let row = tx.query_row("SELECT operation_id, phase, answer_ready, error, waiting_archive, intent, lease_revision FROM inference_setups WHERE phase NOT IN ('completed', 'failed', 'interrupted') AND intent IS NOT NULL", [], |row| {
            Ok((status(row)?, row.get::<_, i64>(6)?, row.get::<_, String>(5)?))
        }).optional().map_err(|_| JournalError::Storage)?;
        let run = if let Some((status, revision, json)) = row {
            match serde_json::from_str::<SetupIntent>(&json) {
                Ok(intent) if intent.version == SetupIntent::VERSION => Some(SetupRun {
                    lease: SetupLease {
                        id: status.operation_id,
                        revision,
                    },
                    status,
                    intent,
                }),
                _ => {
                    tx.execute("UPDATE inference_setups SET phase = 'interrupted', waiting_archive = 0, error = 'The saved setup cannot be resumed. Review a new plan.' WHERE operation_id = ?1", [status.operation_id.to_string()]).map_err(|_| JournalError::Storage)?;
                    None
                }
            }
        } else {
            None
        };
        tx.commit().map_err(|_| JournalError::Storage)?;
        Ok(run)
    }

    fn suspend(&self, lease: SetupLease) -> Result<(), JournalError> {
        let changed = self.write().map_err(|_| JournalError::Storage)?.execute(
            "UPDATE inference_setups SET lease_revision = lease_revision + 1 WHERE operation_id = ?1 AND lease_revision = ?2 AND phase NOT IN ('completed','failed','interrupted')",
            params![lease.id.to_string(), lease.revision],
        ).map_err(|_| JournalError::Storage)?;
        if changed != 1 {
            return Err(JournalError::Conflict);
        }
        Ok(())
    }

    fn current(&self) -> Result<Option<SetupStatus>, JournalError> {
        self.read().map_err(|_| JournalError::Storage)?.query_row(
            "SELECT operation_id, phase, answer_ready, error, waiting_archive, intent FROM inference_setups ORDER BY rowid DESC LIMIT 1",
            [], status
        ).optional().map_err(|_| JournalError::Storage)
    }

    fn lookup(&self, id: Uuid, fingerprint: &str) -> Result<Option<SetupStatus>, JournalError> {
        let db = self.read().map_err(|_| JournalError::Storage)?;
        existing(&db, id, fingerprint)
    }

    fn claim(
        &self,
        id: Uuid,
        fingerprint: &str,
        embedding_epoch: Option<i64>,
        intent: &SetupIntent,
    ) -> Result<Claim, JournalError> {
        let mut db = self.write().map_err(|_| JournalError::Storage)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| JournalError::Storage)?;
        if let Some(status) = existing(&tx, id, fingerprint)? {
            return Ok(Claim::Existing(status));
        }
        let intent = serde_json::to_string(intent).map_err(|_| JournalError::Storage)?;
        let added = tx.execute(
            "INSERT INTO inference_setups (operation_id, fingerprint, embedding_epoch, selection_revision, phase, intent)
             SELECT ?1, ?2, ?3, coalesce((SELECT revision FROM generation_selection WHERE singleton = 1), 0), 'runtime', ?4
             WHERE NOT EXISTS (SELECT 1 FROM inference_setups WHERE phase NOT IN ('completed', 'failed', 'interrupted'))",
            params![id.to_string(), fingerprint, embedding_epoch, intent],
        ).map_err(|_| JournalError::Storage)?;
        if added == 0 {
            return Err(JournalError::Conflict);
        }
        tx.commit().map_err(|_| JournalError::Storage)?;
        Ok(Claim::Started(SetupLease { id, revision: 0 }))
    }

    fn set_archive_wait(&self, lease: SetupLease, waiting: bool) -> Result<bool, JournalError> {
        let count = self.write().map_err(|_| JournalError::Storage)?.execute(
            "UPDATE inference_setups SET waiting_archive = ?3 WHERE operation_id = ?1 AND lease_revision = ?2 AND phase = 'runtime'",
            params![lease.id.to_string(), lease.revision, waiting],
        ).map_err(|_| JournalError::Storage)?;
        Ok(count == 1)
    }

    fn selection_unchanged(&self, lease: SetupLease) -> Result<bool, JournalError> {
        self.read().map_err(|_| JournalError::Storage)?.query_row(
            "SELECT EXISTS(SELECT 1 FROM inference_setups WHERE operation_id = ?1 AND lease_revision = ?2 AND phase NOT IN ('completed','failed','interrupted') AND selection_revision = coalesce((SELECT revision FROM generation_selection WHERE singleton = 1), 0))",
            params![lease.id.to_string(), lease.revision], |row| row.get(0),
        ).map_err(|_| JournalError::Storage)
    }

    fn embedding_unchanged(&self, lease: SetupLease, profile: &str) -> Result<bool, JournalError> {
        self.read().map_err(|_| JournalError::Storage)?.query_row(
            "SELECT EXISTS(SELECT 1 FROM inference_setups WHERE operation_id = ?1 AND lease_revision = ?2 AND phase NOT IN ('completed','failed','interrupted') AND embedding_epoch IS NOT NULL AND embedding_epoch = coalesce((SELECT epoch FROM embedding_index_state WHERE profile = ?3), 0))",
            params![lease.id.to_string(), lease.revision, profile], |row| row.get(0),
        ).map_err(|_| JournalError::Storage)
    }

    fn transition(
        &self,
        lease: SetupLease,
        from: SetupPhase,
        to: SetupPhase,
        error: Option<&str>,
    ) -> Result<bool, JournalError> {
        if matches!(to, SetupPhase::WaitingForArchive)
            || (matches!(from, SetupPhase::WaitingForArchive)
                && !matches!(to, SetupPhase::Failed | SetupPhase::Interrupted))
        {
            return Ok(false);
        }
        let waiting = matches!(from, SetupPhase::WaitingForArchive);
        let rows = self.write().map_err(|_| JournalError::Storage)?.execute(
            "UPDATE inference_setups SET phase = ?1, error = ?2, waiting_archive = 0
             WHERE operation_id = ?3 AND phase = ?4 AND lease_revision = ?5 AND waiting_archive = ?6 AND phase NOT IN ('completed', 'failed', 'interrupted')",
            params![phase_name(to), error, lease.id.to_string(), phase_name(from), lease.revision, waiting],
        ).map_err(|_| JournalError::Storage)?;
        Ok(rows == 1)
    }

    fn commit_activation(
        &self,
        lease: SetupLease,
        profile: &str,
        fingerprint: &str,
        next: SetupPhase,
    ) -> Result<(), SelectionStoreError> {
        if !matches!(next, SetupPhase::EmbeddingWeights | SetupPhase::Completed) {
            return Err(SelectionStoreError);
        }
        let mut db = self.write().map_err(|_| SelectionStoreError)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| SelectionStoreError)?;
        let changed = tx.execute(
            "UPDATE inference_setups SET answer_ready = 1, phase = ?2, error = NULL
             WHERE operation_id = ?1 AND lease_revision = ?3 AND phase = 'activating_answer'
             AND selection_revision = coalesce((SELECT revision FROM generation_selection WHERE singleton = 1), 0)",
            params![lease.id.to_string(), phase_name(next), lease.revision],
        ).map_err(|_| SelectionStoreError)?;
        if changed != 1 {
            return Err(SelectionStoreError);
        }
        save_selection(&tx, profile, fingerprint).map_err(|_| SelectionStoreError)?;
        tx.commit().map_err(|_| SelectionStoreError)
    }

    fn commit_embedding(
        &self,
        lease: SetupLease,
        previous_profile: &str,
        profile: &str,
    ) -> Result<Option<IndexState>, JournalError> {
        let mut db = self.write().map_err(|_| JournalError::Storage)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| JournalError::Storage)?;
        let epoch = tx.query_row(
            "SELECT embedding_epoch FROM inference_setups WHERE operation_id = ?1 AND lease_revision = ?2 AND phase = 'embedding_weights' AND embedding_epoch IS NOT NULL",
            params![lease.id.to_string(), lease.revision], |row| row.get::<_, i64>(0),
        ).optional().map_err(|_| JournalError::Storage)?;
        let Some(epoch) = epoch else {
            return Ok(None);
        };
        if index_state(&tx, previous_profile)
            .map_err(|_| JournalError::Storage)?
            .epoch
            != epoch
        {
            return Ok(None);
        }
        let Some(state) = switch_embedding_profile(&tx, previous_profile, profile, true)
            .map_err(|_| JournalError::Storage)?
        else {
            return Ok(None);
        };
        let changed = tx.execute(
            "UPDATE inference_setups SET phase = 'completed', error = NULL WHERE operation_id = ?1 AND phase = 'embedding_weights' AND embedding_epoch = ?2 AND lease_revision = ?3",
            params![lease.id.to_string(), epoch, lease.revision],
        ).map_err(|_| JournalError::Storage)?;
        if changed != 1 {
            return Err(JournalError::Storage);
        }
        tx.commit().map_err(|_| JournalError::Storage)?;
        Ok(Some(state))
    }

    fn select_embedding(
        &self,
        previous: &IndexState,
        profile: &str,
    ) -> Result<Option<IndexState>, JournalError> {
        let mut db = self.write().map_err(|_| JournalError::Storage)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| JournalError::Storage)?;
        let setup_active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM inference_setups WHERE phase NOT IN ('completed','failed','interrupted'))",
            [],
            |row| row.get(0),
        ).map_err(|_| JournalError::Storage)?;
        if setup_active
            || selected_embedding(&tx).map_err(|_| JournalError::Storage)? != previous.profile
            || index_state(&tx, &previous.profile)
                .map_err(|_| JournalError::Storage)?
                .epoch
                != previous.epoch
        {
            return Ok(None);
        }
        let state = switch_embedding_profile(&tx, &previous.profile, profile, previous.enabled)
            .map_err(|_| JournalError::Storage)?;
        tx.commit().map_err(|_| JournalError::Storage)?;
        Ok(state)
    }
}

impl SqliteStore {
    /// # Errors
    /// Returns an error if the stored embedding selection cannot be read.
    pub fn selected_embedding(&self) -> Result<String, JournalError> {
        let database = self.read().map_err(|_| JournalError::Storage)?;
        selected_embedding(&database).map_err(|_| JournalError::Storage)
    }
}

fn switch_embedding_profile(
    db: &rusqlite::Connection,
    previous_profile: &str,
    target: &str,
    enabled: bool,
) -> rusqlite::Result<Option<IndexState>> {
    if previous_profile != target {
        if selected_embedding(db)? != previous_profile {
            return Ok(None);
        }
        set_index_enabled(db, previous_profile, false)?;
        db.execute(
            "INSERT INTO embedding_selection (singleton, profile) VALUES (1, ?1)
             ON CONFLICT(singleton) DO UPDATE SET profile = excluded.profile",
            [target],
        )?;
    }
    set_index_enabled(db, target, enabled).map(Some)
}

pub(super) fn selected_embedding(db: &rusqlite::Connection) -> rusqlite::Result<String> {
    db.query_row(
        "SELECT coalesce((SELECT profile FROM embedding_selection WHERE singleton = 1), ?1)",
        [ctxfab_inference::PROFILE],
        |row| row.get(0),
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::domain::model::DownloadError;
    use ctxfab_inference::generation::SelectionStore;
    use uuid::Uuid;

    fn id(value: u128) -> Uuid {
        Uuid::from_u128(value)
    }

    fn started(claim: &Claim) -> Option<SetupLease> {
        match claim {
            Claim::Started(lease) => Some(*lease),
            Claim::Existing(_) => None,
        }
    }

    fn lease(value: u128) -> SetupLease {
        SetupLease {
            id: id(value),
            revision: 0,
        }
    }

    fn intent() -> SetupIntent {
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "selection": {"answerProfile": "model", "embedding": true, "runtimeSource": "hosted"},
            "runtime": {"archiveSha256":"archive", "archiveBytes":1, "expandedBytes":2, "bundleBytes":2, "bundleManifestSha256":"bundle", "appleTeamId":"team", "hostedRelease":null},
            "runtimeRequired":true,
            "answer": {"profile":"model", "sha256":"a".repeat(64), "bytes":3, "wasInstalled":false, "additionalBytes":3, "previousJob":null},
            "embedding": null
        })).unwrap()
    }

    fn prepare(store: &SqliteStore, operation: Uuid) {
        prepare_at(store, operation, 12);
    }

    fn prepare_at(store: &SqliteStore, operation: Uuid, epoch: i64) {
        assert!(matches!(
            store.claim(operation, "plan", Some(epoch), &intent()),
            Ok(Claim::Started(_))
        ));
        assert!(
            store
                .transition(
                    SetupLease {
                        id: operation,
                        revision: 0
                    },
                    SetupPhase::Runtime,
                    SetupPhase::AnswerWeights,
                    None
                )
                .unwrap()
        );
        assert!(
            store
                .transition(
                    SetupLease {
                        id: operation,
                        revision: 0
                    },
                    SetupPhase::AnswerWeights,
                    SetupPhase::ActivatingAnswer,
                    None
                )
                .unwrap()
        );
    }

    #[test]
    fn duplicate_claims_and_recovery_retain_operation_receipts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("setup.sqlite");
        let store = SqliteStore::open(&path).unwrap();
        prepare(&store, id(1));
        assert!(matches!(
            store.claim(id(1), "plan", None, &intent()),
            Ok(Claim::Existing(_))
        ));
        assert!(matches!(
            store.lookup(id(1), "different"),
            Err(JournalError::Conflict)
        ));
        assert!(matches!(
            store.claim(id(2), "plan", None, &intent()),
            Err(JournalError::Conflict)
        ));
        assert!(
            !store
                .transition(lease(1), SetupPhase::Runtime, SetupPhase::Failed, None)
                .unwrap()
        );
        // A failed selection write must roll the setup checkpoint back with it.
        assert!(
            store
                .commit_activation(lease(1), "model", "invalid", SetupPhase::EmbeddingWeights)
                .is_err()
        );
        let pending = store.current().unwrap().unwrap();
        assert_eq!(pending.phase, SetupPhase::ActivatingAnswer);
        assert!(!pending.answer_ready);
        assert!(store.load().unwrap().is_none());
        store
            .commit_activation(
                lease(1),
                "model",
                &"a".repeat(64),
                SetupPhase::EmbeddingWeights,
            )
            .unwrap();
        assert_eq!(
            store.load().unwrap(),
            Some(("model".into(), "a".repeat(64)))
        );
        drop(store);
        let store = SqliteStore::open(&path).unwrap();
        let recovered = store.recover().unwrap().unwrap();
        assert_eq!(recovered.lease.revision, 1);
        assert!(recovered.status.answer_ready);
        assert_eq!(recovered.status.phase, SetupPhase::EmbeddingWeights);
        assert!(
            !store
                .transition(
                    lease(1),
                    SetupPhase::EmbeddingWeights,
                    SetupPhase::Failed,
                    None
                )
                .unwrap()
        );
        assert!(
            store
                .transition(
                    recovered.lease,
                    SetupPhase::EmbeddingWeights,
                    SetupPhase::Interrupted,
                    None
                )
                .unwrap()
        );
        assert!(matches!(
            store.claim(id(2), "plan", None, &intent()),
            Ok(Claim::Started(_))
        ));
        assert!(matches!(
            store.claim(id(1), "plan", None, &intent()),
            Ok(Claim::Existing(_))
        ));
        assert_eq!(store.current().unwrap().unwrap().phase, SetupPhase::Runtime);
    }

    #[test]
    fn manual_selection_aba_fences_old_activation() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(&dir.path().join("setup.sqlite")).unwrap();
        let hash = "a".repeat(64);
        store.save("A", &hash).unwrap();
        prepare(&store, id(1));
        store.save("B", &hash).unwrap();
        store.save("A", &hash).unwrap();
        assert!(
            store
                .commit_activation(lease(1), "candidate", &hash, SetupPhase::Completed)
                .is_err()
        );
        assert_eq!(store.load().unwrap(), Some(("A".into(), hash)));
        let status = store.current().unwrap().unwrap();
        assert_eq!(status.phase, SetupPhase::ActivatingAnswer);
        assert!(!status.answer_ready);
    }
    #[test]
    fn embedding_commit_atomically_enables_policy_and_completes_receipt() {
        use crate::application::retrieval::VectorIndex;
        const PROFILE: &str = "fixture-embedding";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("setup.sqlite");
        let store = SqliteStore::open(&path).unwrap();
        let disabled = store.set_enabled(PROFILE, false).unwrap();
        prepare_at(&store, id(7), disabled.epoch);
        store
            .commit_activation(
                lease(7),
                "model",
                &"a".repeat(64),
                SetupPhase::EmbeddingWeights,
            )
            .unwrap();
        let committed = store
            .commit_embedding(lease(7), PROFILE, PROFILE)
            .unwrap()
            .unwrap();
        assert!(committed.enabled);
        assert_eq!(committed.epoch, disabled.epoch + 1);
        let receipt = store.current().unwrap().unwrap();
        assert_eq!(receipt.phase, SetupPhase::Completed);
        assert!(receipt.answer_ready);
        drop(store);
        let reopened = SqliteStore::open(&path).unwrap();
        assert!(reopened.state(PROFILE).unwrap().enabled);
        assert_eq!(
            reopened.current().unwrap().unwrap().phase,
            SetupPhase::Completed
        );
        assert!(
            reopened
                .commit_embedding(lease(7), PROFILE, PROFILE)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn superseded_or_interrupted_embedding_cannot_commit() {
        use crate::application::retrieval::VectorIndex;
        const PROFILE: &str = "fixture-embedding";
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(&dir.path().join("setup.sqlite")).unwrap();
        let initial = store.state(PROFILE).unwrap();
        prepare_at(&store, id(8), initial.epoch);
        store
            .commit_activation(
                lease(8),
                "model",
                &"a".repeat(64),
                SetupPhase::EmbeddingWeights,
            )
            .unwrap();
        let disabled = store.set_enabled(PROFILE, false).unwrap();
        assert!(
            store
                .commit_embedding(lease(8), PROFILE, PROFILE)
                .unwrap()
                .is_none()
        );
        assert_eq!(store.state(PROFILE).unwrap().epoch, disabled.epoch);
        assert!(!store.state(PROFILE).unwrap().enabled);
        assert_eq!(
            store.current().unwrap().unwrap().phase,
            SetupPhase::EmbeddingWeights
        );
        let recovered = store.recover().unwrap().unwrap();
        assert!(
            store
                .commit_embedding(lease(8), PROFILE, PROFILE)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .transition(
                    recovered.lease,
                    SetupPhase::EmbeddingWeights,
                    SetupPhase::Interrupted,
                    None
                )
                .unwrap()
        );
        assert!(
            store
                .commit_embedding(recovered.lease, PROFILE, PROFILE)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store.current().unwrap().unwrap().phase,
            SetupPhase::Interrupted
        );
    }
    #[test]
    fn embedding_completion_failure_rolls_back_policy_change() {
        use crate::application::retrieval::VectorIndex;
        const PROFILE: &str = "fixture-embedding";
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("setup.sqlite");
        let store = SqliteStore::open(&path).unwrap();
        let disabled = store.set_enabled(PROFILE, false).unwrap();
        prepare_at(&store, id(9), disabled.epoch);
        store
            .commit_activation(
                lease(9),
                "model",
                &"a".repeat(64),
                SetupPhase::EmbeddingWeights,
            )
            .unwrap();
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TRIGGER reject_completion BEFORE UPDATE OF phase ON inference_setups WHEN NEW.phase = 'completed' BEGIN SELECT RAISE(ABORT, 'fixture failure'); END;").unwrap();
        assert!(store.commit_embedding(lease(9), PROFILE, PROFILE).is_err());
        let state = store.state(PROFILE).unwrap();
        assert!(!state.enabled);
        assert_eq!(state.epoch, disabled.epoch);
        let receipt = store.current().unwrap().unwrap();
        assert_eq!(receipt.phase, SetupPhase::EmbeddingWeights);
        assert!(receipt.answer_ready);
    }
    #[test]
    fn legacy_active_receipt_is_interrupted_instead_of_resumed() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(&dir.path().join("setup.sqlite")).unwrap();
        store.write().unwrap().execute("INSERT INTO inference_setups (operation_id, fingerprint, selection_revision, phase) VALUES (?1, 'legacy', 0, 'runtime')", [id(101).to_string()]).unwrap();
        assert!(store.recover().unwrap().is_none());
        assert_eq!(
            store.current().unwrap().unwrap().phase,
            SetupPhase::Interrupted
        );
    }
    #[test]
    fn waiting_archive_and_lease_survive_restart_and_fence_old_runner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("setup.sqlite");
        let store = SqliteStore::open(&path).unwrap();
        let original = started(&store.claim(id(50), "plan", Some(0), &intent()).unwrap()).unwrap();
        assert!(store.set_archive_wait(original, true).unwrap());
        assert_eq!(
            store.current().unwrap().unwrap().phase,
            SetupPhase::WaitingForArchive
        );
        store.suspend(original).unwrap();
        drop(store);
        let reopened = SqliteStore::open(&path).unwrap();
        let recovered = reopened.recover().unwrap().unwrap();
        assert_eq!(recovered.lease.revision, original.revision + 2);
        assert_eq!(recovered.status.phase, SetupPhase::WaitingForArchive);
        assert_eq!(
            recovered.intent.selection.answer_profile.as_deref(),
            Some("model")
        );
        assert!(!reopened.selection_unchanged(original).unwrap());
        assert!(
            !reopened
                .embedding_unchanged(original, "fixture-embedding")
                .unwrap()
        );
        assert!(!reopened.set_archive_wait(original, false).unwrap());
        assert!(reopened.selection_unchanged(recovered.lease).unwrap());
        assert!(
            reopened
                .embedding_unchanged(recovered.lease, "fixture-embedding")
                .unwrap()
        );
        assert!(reopened.set_archive_wait(recovered.lease, false).unwrap());
        assert!(
            reopened
                .transition(
                    recovered.lease,
                    SetupPhase::Runtime,
                    SetupPhase::AnswerWeights,
                    None
                )
                .unwrap()
        );
        assert!(
            !reopened
                .transition(
                    original,
                    SetupPhase::AnswerWeights,
                    SetupPhase::ActivatingAnswer,
                    None
                )
                .unwrap()
        );
    }

    #[test]
    fn setup_model_ownership_refuses_discard_replacement_and_stale_lease() {
        use crate::application::model_management::{DownloadJobStore, StoredDownload};
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(&dir.path().join("setup.sqlite")).unwrap();
        let current = started(&store.claim(id(51), "plan", None, &intent()).unwrap()).unwrap();
        assert!(
            store
                .transition(
                    current,
                    SetupPhase::Runtime,
                    SetupPhase::AnswerWeights,
                    None
                )
                .unwrap()
        );
        let job = StoredDownload {
            profile: "model".into(),
            fingerprint: "a".repeat(64),
            operation_id: current.id.to_string(),
            phase: "running".into(),
            error: None,
        };
        assert!(store.save_setup_job(&job, current, None).is_ok());
        assert!(store.save_setup_job(&job, current, None).is_ok());
        store.remove_download_job("model").unwrap();
        assert_eq!(
            store.save_setup_job(&job, current, None),
            Err(DownloadError::InUse)
        );
        let replacement = StoredDownload {
            operation_id: id(99).to_string(),
            phase: "interrupted".into(),
            ..job
        };
        store.save_download_job(&replacement).unwrap();
        let old = StoredDownload {
            operation_id: current.id.to_string(),
            phase: "running".into(),
            ..replacement
        };
        assert_eq!(
            store.save_setup_job(&old, current, None),
            Err(DownloadError::InUse)
        );
        store.suspend(current).unwrap();
        assert_eq!(
            store.save_setup_job(&old, current, None),
            Err(DownloadError::InUse)
        );
    }
    #[test]
    fn first_setup_claim_requires_the_reviewed_previous_job() {
        use crate::application::model_management::{DownloadJobStore, StoredDownload};
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(&dir.path().join("setup.sqlite")).unwrap();
        let mut reviewed = intent();
        reviewed.answer.previous_job = Some(id(90).to_string());
        let current = started(&store.claim(id(52), "plan", None, &reviewed).unwrap()).unwrap();
        assert!(
            store
                .transition(
                    current,
                    SetupPhase::Runtime,
                    SetupPhase::AnswerWeights,
                    None
                )
                .unwrap()
        );
        let prior = StoredDownload {
            profile: "model".into(),
            fingerprint: "a".repeat(64),
            operation_id: id(90).to_string(),
            phase: "interrupted".into(),
            error: None,
        };
        store.save_download_job(&prior).unwrap();
        let setup = StoredDownload {
            operation_id: current.id.to_string(),
            phase: "running".into(),
            ..prior
        };
        assert_eq!(
            store.save_setup_job(&setup, current, None),
            Err(DownloadError::InUse)
        );
        assert_eq!(
            store.save_setup_job(&setup, current, Some("different")),
            Err(DownloadError::InUse)
        );
        assert!(
            store
                .save_setup_job(&setup, current, Some(&id(90).to_string()))
                .is_ok()
        );
        let paused = StoredDownload {
            phase: "paused".into(),
            ..setup
        };
        store.save_download_job(&paused).unwrap();
        let resumed = StoredDownload {
            phase: "running".into(),
            ..paused
        };
        assert_eq!(
            store.save_setup_job(&resumed, current, None),
            Err(DownloadError::InUse)
        );
    }
    #[test]
    fn waiting_archive_can_be_interrupted_only_by_current_lease() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(&dir.path().join("setup.sqlite")).unwrap();
        let current = started(&store.claim(id(60), "plan", None, &intent()).unwrap()).unwrap();
        assert!(store.set_archive_wait(current, true).unwrap());
        let stale = SetupLease {
            revision: current.revision + 1,
            ..current
        };
        assert!(
            !store
                .transition(
                    stale,
                    SetupPhase::WaitingForArchive,
                    SetupPhase::Interrupted,
                    None
                )
                .unwrap()
        );
        assert!(
            !store
                .transition(
                    current,
                    SetupPhase::WaitingForArchive,
                    SetupPhase::AnswerWeights,
                    None
                )
                .unwrap()
        );
        assert!(
            store
                .transition(
                    current,
                    SetupPhase::WaitingForArchive,
                    SetupPhase::Interrupted,
                    Some("cancelled")
                )
                .unwrap()
        );
        let receipt = store.current().unwrap().unwrap();
        assert_eq!(receipt.phase, SetupPhase::Interrupted);
        assert_eq!(receipt.error.as_deref(), Some("cancelled"));
    }

    #[test]
    fn invalid_saved_intent_is_interrupted_without_blocking_startup() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(&dir.path().join("setup.sqlite")).unwrap();
        store.write().unwrap().execute("INSERT INTO inference_setups (operation_id, fingerprint, selection_revision, phase, intent) VALUES (?1, 'plan', 0, 'runtime', '{broken')", [id(61).to_string()]).unwrap();
        assert!(store.recover().unwrap().is_none());
        let receipt = store.current().unwrap().unwrap();
        assert_eq!(receipt.phase, SetupPhase::Interrupted);
        assert_eq!(
            receipt.error.as_deref(),
            Some("The saved setup cannot be resumed. Review a new plan.")
        );
    }
    #[test]
    fn dispatch_rejects_changed_selection_or_index_epoch_before_job_creation() {
        use crate::application::model_management::{DownloadJobStore, StoredDownload};
        use crate::application::retrieval::VectorIndex;
        const PROFILE: &str = ctxfab_inference::PROFILE;
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(&dir.path().join("selection.sqlite")).unwrap();
        let current = started(&store.claim(id(70), "plan", None, &intent()).unwrap()).unwrap();
        assert!(
            store
                .transition(
                    current,
                    SetupPhase::Runtime,
                    SetupPhase::AnswerWeights,
                    None
                )
                .unwrap()
        );
        store.save("other", &"b".repeat(64)).unwrap();
        let job = StoredDownload {
            profile: "model".into(),
            fingerprint: "a".repeat(64),
            operation_id: current.id.to_string(),
            phase: "running".into(),
            error: None,
        };
        assert_eq!(
            store.save_setup_job(&job, current, None),
            Err(DownloadError::InUse)
        );
        assert!(store.load_download_jobs().unwrap().is_empty());
        let store = SqliteStore::open(&dir.path().join("embedding.sqlite")).unwrap();
        let mut approved = intent();
        let mut embedding = approved.answer.clone();
        embedding.profile = PROFILE.into();
        approved.embedding = Some(embedding);
        let current = started(&store.claim(id(71), "plan", Some(0), &approved).unwrap()).unwrap();
        assert!(
            store
                .transition(
                    current,
                    SetupPhase::Runtime,
                    SetupPhase::AnswerWeights,
                    None
                )
                .unwrap()
        );
        assert!(
            store
                .transition(
                    current,
                    SetupPhase::AnswerWeights,
                    SetupPhase::ActivatingAnswer,
                    None
                )
                .unwrap()
        );
        store
            .commit_activation(
                current,
                "model",
                &"a".repeat(64),
                SetupPhase::EmbeddingWeights,
            )
            .unwrap();
        store.set_enabled(PROFILE, false).unwrap();
        let job = StoredDownload {
            profile: PROFILE.into(),
            operation_id: current.id.to_string(),
            ..job
        };
        assert_eq!(
            store.save_setup_job(&job, current, None),
            Err(DownloadError::InUse)
        );
        assert!(store.load_download_jobs().unwrap().is_empty());
    }
}
