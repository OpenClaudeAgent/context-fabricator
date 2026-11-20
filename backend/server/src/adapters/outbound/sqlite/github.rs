use rusqlite::{Connection, Row, TransactionBehavior, params};

use super::SqliteStore;
use crate::application::github::{Binding, Store};
use crate::domain::github::current::CollectionStatus;
use crate::domain::github::{
    ACTIONS_TOKEN_URL, Access, BUCKET, ConfiguredRepository, CredentialState, Defaults, Error,
    Features, Repository, Snapshot, TOKEN_URL,
};
use crate::domain::sync::{Phase, minute_spacing};

mod current;

impl Store for SqliteStore {
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
                    CASE WHEN j.phase IN ('retry', 'throttled', 'failed') THEN s.last_error END
             FROM github_repositories r
             LEFT JOIN github_code_scopes s ON (s.repository_id, s.branch) = (r.id, r.branch)
             LEFT JOIN github_current_eligible e ON e.collection = s.collection
             LEFT JOIN sync_jobs j ON (j.collection, j.generation) = (s.collection, e.generation)
             LEFT JOIN source_snapshots p ON p.collection = s.collection AND p.phase = 'published'
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
                 actions_access, verified) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1)
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
                access_value(config.contents), access_value(config.actions),
            ],
        ).map_err(|_| Error::Storage)?;
        current::configured(&tx, config)?;
        tx.commit().map_err(|_| Error::Storage)
    }

    fn remove(&self, revision: u32, id: i64) -> Result<(), Error> {
        let mut writer = self.write().map_err(|_| Error::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| Error::Storage)?;
        advance(&tx, revision, false)?;
        current::retire(&tx, Some(id), None)?;
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
    let current_code: bool = row.get(6)?;
    Ok(ConfiguredRepository {
        repository: Repository {
            id: row.get(0)?,
            full_name: row.get(1)?,
            default_branch: row.get(2)?,
            private: row.get(3)?,
        },
        branch: row.get(4)?,
        interval_minutes: row.get(5)?,
        features: Features {
            current_code,
            history: row.get(7)?,
            actions: row.get(8)?,
            code_graph: row.get(9)?,
        },
        contents: access(row.get(10)?),
        actions: access(row.get(11)?),
        verified: row.get(12)?,
        code_collection: Some(collection_status(row, current_code)?),
    })
}

fn collection_status(row: &Row<'_>, current_code: bool) -> rusqlite::Result<CollectionStatus> {
    let eligible: bool = row.get(13)?;
    let job = row.get::<_, Option<String>>(14)?;
    let state = job.as_deref().and_then(Phase::from_code);
    let phase = match (current_code, eligible, state) {
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
        error: row.get(19)?,
    })
}
