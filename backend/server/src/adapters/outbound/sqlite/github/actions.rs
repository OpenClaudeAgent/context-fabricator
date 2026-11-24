//! Durable Actions snapshot with independent authority fencing.
use super::{SqliteStore, trace};
use crate::application::github::actions::{Store, Trigger};
use crate::application::telemetry::TraceLink;
use crate::domain::github::{
    Error,
    actions::{Due, Schedule, Scope, Snapshot, State},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

impl Store for SqliteStore {
    fn actions_scope(&self, id: i64) -> Result<Scope, Error> {
        let connection = self.read().map_err(|_| Error::Storage)?;
        read_scope(&connection, id)
    }
    fn actions_state(&self, id: i64) -> Result<State, Error> {
        let connection = self.read().map_err(|_| Error::Storage)?;
        let (document, availability, schedule): (Option<String>, _, _) = connection
            .query_row(
                "SELECT s.snapshot,
                    CASE WHEN r.actions = 0 THEN 'disabled'
                         WHEN c.credential_ref IS NULL OR c.reconnect = 1 THEN 'reconnectRequired'
                         WHEN r.verified = 0 OR r.actions_access = 0 OR r.contents_access = 0
                           THEN 'accessRequired'
                         ELSE 'ready' END,
                    j.next_at_ms, j.last_checked_at_ms, j.last_error
                 FROM github_repositories r
                 LEFT JOIN github_actions_snapshots s ON (s.repository_id, s.branch) = (r.id, r.branch)
                 LEFT JOIN github_actions_generations g ON g.repository_id = r.id
                 LEFT JOIN github_actions_schedule j
                     ON (j.repository_id, j.generation) = (r.id, g.generation)
                 CROSS JOIN github_connection c
                 WHERE r.id = ?1 AND c.singleton = 1",
                [id],
                |row| {
                    let schedule = match row.get::<_, Option<i64>>(2)? {
                        Some(next_at_ms) => Some(Schedule {
                            next_at_ms,
                            last_checked_at_ms: row.get(3)?,
                            last_error: row.get(4)?,
                        }),
                        None => None,
                    };
                    Ok((row.get(0)?, row.get(1)?, schedule))
                },
            )
            .optional()
            .map_err(storage)?
            .ok_or(Error::Access)?;
        Ok(State {
            availability,
            schedule,
            snapshot: document
                .map(|value| serde_json::from_str(&value))
                .transpose()
                .map_err(|_| Error::Storage)?,
        })
    }
    fn actions_due(&self, id: Option<i64>) -> Result<Option<Due>, Error> {
        let connection = self.read().map_err(|_| Error::Storage)?;
        read_due(&connection, id)
    }
    fn claim_actions(&self, due: &Due, trace: Option<TraceLink>) -> Result<(), Error> {
        let mut connection = self.write().map_err(|_| Error::Storage)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        if read_due(&transaction, Some(due.repository_id))? != Some(*due) {
            return Err(Error::Conflict);
        }
        let changed = transaction
            .execute(
                "UPDATE github_actions_generations SET latest_trace = ?3
             WHERE repository_id = ?1 AND generation = ?2",
                params![due.repository_id, due.generation, trace.map(trace::encode)],
            )
            .map_err(storage)?;
        if changed != 1 {
            return Err(Error::Conflict);
        }
        transaction.commit().map_err(storage)
    }
    fn actions_failed(
        &self,
        due: &Due,
        now_ms: i64,
        error: Error,
        trigger: Trigger,
    ) -> Result<(), Error> {
        let mut connection = self.write().map_err(|_| Error::Storage)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let next_at_ms = match trigger {
            Trigger::Manual => due.at_ms,
            Trigger::Scheduled => now_ms.saturating_add(60_000).max(0),
        };
        record(
            &transaction,
            due,
            now_ms,
            next_at_ms,
            Some(error.schedule_code()),
        )?;
        transaction.commit().map_err(storage)
    }
    fn publish_actions(
        &self,
        scope: &Scope,
        due: &Due,
        snapshot: &Snapshot,
        checked_at_ms: i64,
        producer: Option<TraceLink>,
    ) -> Result<(), Error> {
        if scope.repository_id != due.repository_id || scope.generation != due.generation {
            return Err(Error::Conflict);
        }
        let encoded = serde_json::to_string(snapshot).map_err(|_| Error::Storage)?;
        let mut connection = self.write().map_err(|_| Error::Storage)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        match read_scope(&transaction, scope.repository_id) {
            Ok(current) if current == *scope => {}
            Ok(_) | Err(Error::Access) => return Err(Error::Conflict),
            Err(error) => return Err(error),
        }
        let before = super::storage::used(&transaction).map_err(storage)?;
        transaction.execute(
            "INSERT INTO github_actions_snapshots (repository_id, branch, snapshot, published_trace)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (repository_id, branch) DO UPDATE SET
                snapshot = excluded.snapshot, published_trace = excluded.published_trace
             WHERE snapshot <> excluded.snapshot OR published_trace IS NOT excluded.published_trace",
            params![scope.repository_id, scope.branch, encoded, producer.map(trace::encode)],
        ).map_err(storage)?;
        if !super::storage::fits(&transaction, before).map_err(storage)? {
            return Err(Error::StorageLimit);
        }
        let interval_ms = due.interval_minutes.saturating_mul(60_000);
        let next_at_ms = checked_at_ms.saturating_add(interval_ms).max(0);
        record(&transaction, due, checked_at_ms, next_at_ms, None)?;
        transaction.commit().map_err(storage)
    }
}

fn read_due(connection: &Connection, id: Option<i64>) -> Result<Option<Due>, Error> {
    connection
        .query_row(
            "SELECT r.id, g.generation, COALESCE(s.next_at_ms, 0), r.interval_minutes
             FROM github_repositories r
             JOIN github_actions_generations g ON g.repository_id = r.id
             LEFT JOIN github_actions_schedule s
                 ON (s.repository_id, s.generation) = (r.id, g.generation)
             CROSS JOIN github_connection c
             WHERE (?1 IS NULL OR r.id = ?1)
                 AND r.actions = 1 AND r.verified = 1 AND r.actions_access = 1
                 AND r.contents_access = 1 AND c.singleton = 1 AND c.reconnect = 0
                 AND c.credential_ref IS NOT NULL
                 AND NOT EXISTS (SELECT 1 FROM github_data_paused p WHERE p.repository_id = r.id)
             ORDER BY 3, r.id LIMIT 1",
            [id],
            |row| {
                Ok(Due {
                    repository_id: row.get(0)?,
                    generation: row.get(1)?,
                    at_ms: row.get(2)?,
                    interval_minutes: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(storage)
}

fn record(
    connection: &Connection,
    due: &Due,
    checked_at_ms: i64,
    next_at_ms: i64,
    error: Option<&str>,
) -> Result<(), Error> {
    if read_due(connection, Some(due.repository_id))? != Some(*due) {
        return Err(Error::Conflict);
    }
    connection
        .execute(
            "INSERT INTO github_actions_schedule
                 (repository_id, generation, next_at_ms, last_checked_at_ms, last_error)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(repository_id) DO UPDATE SET generation = excluded.generation,
                 next_at_ms = excluded.next_at_ms,
                 last_checked_at_ms = excluded.last_checked_at_ms, last_error = excluded.last_error",
            params![due.repository_id, due.generation, next_at_ms, checked_at_ms, error],
        )
        .map_err(storage)?;
    Ok(())
}

fn read_scope(connection: &Connection, id: i64) -> Result<Scope, Error> {
    connection.query_row("SELECT r.full_name, r.branch, g.generation, c.credential_ref FROM github_repositories r JOIN github_actions_generations g ON g.repository_id = r.id CROSS JOIN github_connection c WHERE r.id = ?1 AND r.actions = 1 AND r.verified = 1 AND r.actions_access = 1 AND r.contents_access = 1 AND c.singleton = 1 AND c.reconnect = 0 AND c.credential_ref IS NOT NULL AND NOT EXISTS (SELECT 1 FROM github_data_paused p WHERE p.repository_id=r.id)", [id], |row| Ok(Scope { repository_id:id, full_name:row.get(0)?, branch:row.get(1)?, generation:row.get(2)?, credential:row.get(3)? })).optional().map_err(storage)?.ok_or(Error::Access)
}
fn storage(_: rusqlite::Error) -> Error {
    Error::Storage
}
