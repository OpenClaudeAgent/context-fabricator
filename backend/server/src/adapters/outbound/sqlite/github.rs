pub(super) mod storage;
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use super::SqliteStore;
use crate::application::github::{Binding, Store};
use crate::domain::github::current::{CollectionStatus, ManifestPhase, SourceProgress};
use crate::domain::github::{
    ACTIONS_TOKEN_URL, Access, BUCKET, ConfiguredRepository, CredentialState, Defaults, Error,
    Features, Repository, Snapshot, TOKEN_URL, TokenSetup,
};
use crate::domain::sync::{Phase, minute_spacing};

mod actions;
mod current;
pub(super) mod graph;
mod history;
mod trace;

impl Store for SqliteStore {
    fn stage_credential(&self, reference: &str) -> Result<(), Error> {
        let writer = self.write().map_err(|_| Error::Storage)?;
        let inserted = writer
            .execute(
                "INSERT INTO github_credential_cleanup (credential_ref)
             SELECT ?1 WHERE NOT EXISTS (
                 SELECT 1 FROM github_connection WHERE credential_ref = ?1
             )",
                [reference],
            )
            .map_err(|_| Error::Storage)?;
        if inserted == 0 {
            Err(Error::Storage)
        } else {
            Ok(())
        }
    }

    fn pending_credentials(&self) -> Result<Vec<String>, Error> {
        let reader = self.read().map_err(|_| Error::Storage)?;
        let mut query = reader.prepare(
            "SELECT credential_ref FROM github_credential_cleanup
             WHERE NOT EXISTS (SELECT 1 FROM github_connection
                               WHERE github_connection.credential_ref = github_credential_cleanup.credential_ref)",
        ).map_err(|_| Error::Storage)?;
        query
            .query_map([], |row| row.get(0))
            .and_then(Iterator::collect)
            .map_err(|_| Error::Storage)
    }

    fn forget_credential(&self, reference: &str) -> Result<(), Error> {
        self.write()
            .map_err(|_| Error::Storage)?
            .execute(
                "DELETE FROM github_credential_cleanup WHERE credential_ref = ?1
             AND NOT EXISTS (SELECT 1 FROM github_connection WHERE credential_ref = ?1)",
                [reference],
            )
            .map(|_| ())
            .map_err(|_| Error::Storage)
    }

    fn data_state(&self, id: i64) -> Result<crate::domain::github::DataState, Error> {
        self.github_data_state(id)
    }

    fn purge_data(&self, id: i64) -> Result<crate::domain::github::StorageBudget, Error> {
        self.purge_github_data(id)
    }
    fn resume_data(&self, id: i64, stream: &str) -> Result<(), Error> {
        self.resume_github_data(id, stream)
    }

    fn storage_budget(&self) -> Result<crate::domain::github::StorageBudget, Error> {
        self.github_storage()
    }
    fn set_storage_budget(
        &self,
        revision: u32,
        limit_gb: u16,
    ) -> Result<crate::domain::github::StorageBudget, Error> {
        self.update_github_storage(revision, limit_gb)
    }

    fn snapshot(&self) -> Result<Snapshot, Error> {
        let mut reader = self.read().map_err(|_| Error::Storage)?;
        let tx = reader.transaction().map_err(|_| Error::Storage)?;
        let (revision, login, reconnect) = tx
            .query_row(
                "SELECT revision, login, reconnect FROM github_connection WHERE singleton = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, bool>(2)?,
                    ))
                },
            )
            .map_err(|_| Error::Storage)?;
        let repositories = tx.prepare(
            "SELECT r.id, r.full_name, r.default_branch, r.private, r.branch, r.interval_minutes,
                    r.current_code, r.history, r.actions, r.code_graph, r.contents_access, r.actions_access, r.verified,
                    e.repository_id IS NOT NULL, j.phase, j.due_at,
                    s.last_checked_at_ms, p.published_at_ms, p.revision,
                    CASE WHEN j.phase IN ('retry', 'throttled', 'failed') THEN s.last_error END,
                    hg.latest_trace AS history_attempt, h.published_trace AS history_producer,
                    ag.latest_trace AS actions_attempt, a.published_trace AS actions_producer,
                    CASE WHEN p.id IS NOT NULL THEN
                        (SELECT COUNT(*) FROM source_snapshot_files f WHERE f.snapshot_id = p.id)
                    END AS published_files,
                    w.revision AS pending_commit, w.phase = 'sealed' AS manifest_complete,
                    CASE WHEN w.id IS NOT NULL THEN
                        (SELECT COUNT(*) FROM source_snapshot_files f WHERE f.snapshot_id = w.id)
                    END AS staged_files
             FROM github_repositories r
             LEFT JOIN (
                SELECT collection,repository_id,branch,last_checked_at_ms,last_error,1 AS general
                FROM github_code_scopes
                UNION ALL
                SELECT collection,repository_id,branch,last_checked_at_ms,last_error,0 AS general
                FROM github_graph_scopes
             ) s ON (s.repository_id, s.branch) = (r.id, r.branch)
                    AND s.general = (r.current_code OR NOT r.code_graph)
             LEFT JOIN github_current_eligible e ON e.collection = s.collection
             LEFT JOIN sync_jobs j ON (j.collection, j.generation) = (s.collection, e.generation)
             LEFT JOIN source_snapshots p ON p.collection = s.collection AND p.phase = 'published'
             LEFT JOIN source_snapshots w ON w.collection = e.collection
                 AND w.generation = e.generation AND w.tracked_ref = 'refs/heads/' || e.branch
                 AND w.phase <> 'published'
             LEFT JOIN github_history_generations hg ON hg.repository_id = r.id
             LEFT JOIN github_history_lineage h ON (h.repository_id, h.branch) = (r.id, r.branch)
             LEFT JOIN github_actions_generations ag ON ag.repository_id = r.id
             LEFT JOIN github_actions_snapshots a ON (a.repository_id, a.branch) = (r.id, r.branch)
             ORDER BY r.full_name COLLATE NOCASE, r.id",
        ).and_then(|mut query| query.query_map([], read_repository)?.collect::<rusqlite::Result<_>>())
            .map_err(|_| Error::Storage)?;
        let credential = match (login.is_some(), reconnect) {
            (false, _) => CredentialState::Unconfigured,
            (true, true) => CredentialState::ReconnectRequired,
            (true, false) => CredentialState::Connected,
        };
        Ok(Snapshot {
            revision,
            credential,
            login,
            authorization_url: TOKEN_URL,
            actions_authorization_url: ACTIONS_TOKEN_URL,
            defaults: Defaults::default(),
            repositories,
            traffic: None,
        })
    }

    fn binding(&self) -> Result<Binding, Error> {
        self.read().map_err(|_| Error::Storage)?.query_row(
            "SELECT revision, credential_ref, reconnect FROM github_connection WHERE singleton = 1",
            [],
            |row| Ok(Binding { revision: row.get(0)?, reference: row.get(1)?, reconnect: row.get(2)? }),
        ).map_err(|_| Error::Storage)
    }

    fn bind(&self, revision: u32, credential: Option<(&str, &str)>) -> Result<(), Error> {
        let (reference, login) = credential.unzip();
        let mut writer = self.write().map_err(|_| Error::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| Error::Storage)?;
        tx.execute(
            "INSERT OR IGNORE INTO github_credential_cleanup (credential_ref)
             SELECT credential_ref FROM github_connection
             WHERE singleton = 1 AND revision = ?1 AND credential_ref IS NOT NULL
               AND credential_ref IS NOT ?2",
            params![revision, reference],
        )
        .map_err(|_| Error::Storage)?;
        let changed = tx
            .execute(
                "UPDATE github_connection SET revision = revision + 1, credential_ref = ?2,
                    login = ?3, reconnect = 0
             WHERE singleton = 1 AND revision = ?1",
                params![revision, reference, login],
            )
            .map_err(|_| Error::Storage)?;
        if changed == 0 {
            return Err(Error::Conflict);
        }
        if let Some(reference) = reference {
            tx.execute(
                "DELETE FROM github_credential_cleanup WHERE credential_ref = ?1",
                [reference],
            )
            .map_err(|_| Error::Storage)?;
        }
        invalidate_repositories(&tx)?;
        tx.commit().map_err(|_| Error::Storage)
    }

    fn expire(&self, reference: &str) -> Result<(), Error> {
        let mut writer = self.write().map_err(|_| Error::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| Error::Storage)?;
        let changed = tx
            .execute(
                "UPDATE github_connection SET revision = revision + 1, reconnect = 1
             WHERE singleton = 1 AND credential_ref = ?1 AND reconnect = 0",
                [reference],
            )
            .map_err(|_| Error::Storage)?;
        if changed != 0 {
            invalidate_repositories(&tx)?;
        }
        tx.commit().map_err(|_| Error::Storage)
    }

    fn save(&self, revision: u32, config: &ConfiguredRepository) -> Result<(), Error> {
        let mut writer = self.write().map_err(|_| Error::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| Error::Storage)?;
        advance(&tx, revision, true)?;
        tx.execute(
            "INSERT INTO github_repositories (id, full_name, default_branch, private, branch,
                 interval_minutes, current_code, history, actions, code_graph, contents_access,
                 actions_access, verified) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET full_name = excluded.full_name,
                 default_branch = excluded.default_branch, private = excluded.private,
                 branch = excluded.branch, interval_minutes = excluded.interval_minutes,
                 current_code = excluded.current_code, history = excluded.history,
                 actions = excluded.actions, code_graph = excluded.code_graph,
                 contents_access = excluded.contents_access, actions_access = excluded.actions_access,
                 verified = excluded.verified",
            params![
                config.repository.id, config.repository.full_name, config.repository.default_branch,
                config.repository.private, config.branch, config.interval_minutes, config.features.current_code,
                config.features.history, config.features.actions, config.features.code_graph,
                access_value(config.contents), access_value(config.actions), config.verified,
            ],
        ).map_err(|_| Error::Storage)?;
        current::configured(&tx, config)?;
        tx.commit().map_err(|_| Error::Storage)
    }

    fn record_access(&self, revision: u32, config: &ConfiguredRepository) -> Result<bool, Error> {
        let mut writer = self.write().map_err(|_| Error::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| Error::Storage)?;
        let id = config.repository.id;
        let previously_allowed: bool = tx
            .query_row(
                "SELECT r.verified AND r.contents_access IS 1 FROM github_repositories r
                 CROSS JOIN github_connection c WHERE r.id = ?1 AND c.singleton = 1
                 AND c.revision = ?2 AND c.credential_ref IS NOT NULL AND c.reconnect = 0",
                params![id, revision],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| Error::Storage)?
            .ok_or(Error::Conflict)?;
        let changed = tx
            .execute(
                "UPDATE github_repositories SET contents_access = ?2, actions_access = ?3,
                     verified = ?4 WHERE id = ?1 AND (contents_access IS NOT ?2
                     OR actions_access IS NOT ?3 OR verified IS NOT ?4)",
                params![
                    id,
                    access_value(config.contents),
                    access_value(config.actions),
                    config.verified,
                ],
            )
            .map_err(|_| Error::Storage)?
            != 0;
        if changed {
            advance(&tx, revision, true)?;
            let has_contents = config.verified && config.contents == Access::Available;
            if previously_allowed && !has_contents {
                current::retire(&tx, Some(id), None)?;
                graph::retire(&tx, Some(id), None)?;
            } else if !previously_allowed && has_contents {
                current::configured(&tx, config)?;
            }
        }
        tx.commit().map_err(|_| Error::Storage)?;
        Ok(changed)
    }

    fn remove(&self, revision: u32, id: i64) -> Result<(), Error> {
        let mut writer = self.write().map_err(|_| Error::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| Error::Storage)?;
        advance(&tx, revision, false)?;
        current::retire(&tx, Some(id), None)?;
        graph::retire(&tx, Some(id), None)?;
        tx.execute("DELETE FROM github_repositories WHERE id = ?1", [id])
            .map_err(|_| Error::Storage)?;
        tx.commit().map_err(|_| Error::Storage)
    }

    fn set_rate(&self, revision: u32, requests_per_minute: u16) -> Result<(), Error> {
        let mut writer = self.write().map_err(|_| Error::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| Error::Storage)?;
        advance(&tx, revision, false)?;
        tx.execute(
            "INSERT INTO outbound_buckets (bucket, requests_per_minute) VALUES (?1, ?2)
             ON CONFLICT(bucket) DO UPDATE SET requests_per_minute = excluded.requests_per_minute,
               next_at = max(next_at, CASE WHEN attempts > 0 THEN last_handoff + ?3 ELSE 0 END)",
            params![
                BUCKET,
                requests_per_minute,
                minute_spacing(requests_per_minute)
            ],
        )
        .map_err(|_| Error::Storage)?;
        tx.commit().map_err(|_| Error::Storage)
    }
}

fn advance(tx: &Connection, revision: u32, authorized: bool) -> Result<(), Error> {
    let changed = tx
        .execute(
            "UPDATE github_connection SET revision = revision + 1
         WHERE singleton = 1 AND revision = ?1
           AND (NOT ?2 OR (credential_ref IS NOT NULL AND reconnect = 0))",
            params![revision, authorized],
        )
        .map_err(|_| Error::Storage)?;
    if changed == 0 {
        Err(Error::Conflict)
    } else {
        Ok(())
    }
}

fn invalidate_repositories(tx: &Connection) -> Result<(), Error> {
    current::retire(tx, None, None)?;
    tx.execute(
        "UPDATE github_repositories SET verified = 0, contents_access = NULL, actions_access = NULL",
        [],
    ).map(|_| ()).map_err(|_| Error::Storage)
}

const fn access(value: Option<bool>) -> Access {
    match value {
        None => Access::Unchecked,
        Some(true) => Access::Available,
        Some(false) => Access::Unavailable,
    }
}

const fn access_value(value: Access) -> Option<bool> {
    match value {
        Access::Unchecked => None,
        Access::Available => Some(true),
        Access::Unavailable => Some(false),
    }
}

fn read_repository(row: &Row<'_>) -> rusqlite::Result<ConfiguredRepository> {
    let full_name: String = row.get(1)?;
    let features = Features {
        current_code: row.get(6)?,
        history: row.get(7)?,
        actions: row.get(8)?,
        code_graph: row.get(9)?,
    };
    let code_collection = Some(collection_status(
        row,
        features.current_code || features.code_graph,
    )?);
    Ok(ConfiguredRepository {
        token_setup: TokenSetup::for_repository(&full_name),
        repository: Repository {
            id: row.get(0)?,
            full_name,
            default_branch: row.get(2)?,
            private: row.get(3)?,
        },
        branch: row.get(4)?,
        interval_minutes: row.get(5)?,
        features,
        contents: access(row.get(10)?),
        actions: access(row.get(11)?),
        verified: row.get(12)?,
        code_collection,
        history_activity: trace::activity(row, "history_attempt", "history_producer")?,
        actions_activity: trace::activity(row, "actions_attempt", "actions_producer")?,
    })
}

fn collection_status(row: &Row<'_>, enabled: bool) -> rusqlite::Result<CollectionStatus> {
    let eligible: bool = row.get(13)?;
    let job = row.get::<_, Option<String>>(14)?;
    let state = job.as_deref().and_then(Phase::from_code);
    let phase = match (enabled, eligible, state) {
        (false, _, _) => "disabled",
        (true, false, _) => "credentials",
        (true, true, None) => "queued",
        (true, true, Some(phase)) => phase.code(),
    };
    Ok(CollectionStatus {
        phase,
        can_refresh: eligible && matches!(state, Some(Phase::Succeeded | Phase::Failed)),
        next_at_ms: if eligible && state != Some(Phase::Running) {
            row.get(15)?
        } else {
            None
        },
        last_checked_at_ms: row.get(16)?,
        published_at_ms: row.get(17)?,
        commit: row.get(18)?,
        published_files: row.get("published_files")?,
        pending: pending_progress(row)?,
        error: row.get(19)?,
    })
}

fn pending_progress(row: &Row<'_>) -> rusqlite::Result<Option<SourceProgress>> {
    let Some(commit) = row.get("pending_commit")? else {
        return Ok(None);
    };
    let phase = if row.get("manifest_complete")? {
        ManifestPhase::ManifestComplete
    } else {
        ManifestPhase::Collecting
    };
    Ok(Some(SourceProgress {
        commit,
        staged_files: row.get("staged_files")?,
        phase,
    }))
}
