use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::super::{storage::resume_data_in, trace};
use super::{SqliteStore, authorized, scope_in, storage};
use crate::adapters::outbound::sqlite::sync::{persist_job, read_job};
use crate::application::{
    github::current::{Claim, ClaimRequest, ScheduleStore},
    telemetry::TraceLink,
};
use crate::domain::{
    github::{
        Error,
        current::{Due, Scope},
    },
    sync::{JobState, Phase},
};

pub(in crate::adapters::outbound::sqlite) fn reschedule(
    database: &Connection,
    collection: &str,
    interval: u16,
) -> Result<(), Error> {
    database
        .execute(
            "UPDATE sync_jobs SET due_at = (
            SELECT last_checked_at_ms + ?2 * 60000 FROM (SELECT collection,last_checked_at_ms FROM github_code_scopes UNION ALL SELECT collection,last_checked_at_ms FROM github_graph_scopes) WHERE collection = ?1
         ) WHERE collection = ?1 AND phase = 'succeeded' AND generation = (
            SELECT generation FROM collections WHERE name = ?1
         )",
            params![collection, interval],
        )
        .map(|_| ())
        .map_err(storage)
}

impl ScheduleStore for SqliteStore {
    fn next(&self) -> Result<Option<Due>, Error> {
        self.read()
            .map_err(|_| Error::Storage)?
            .query_row(
                "SELECT e.repository_id, e.collection, e.generation,
                    CASE WHEN j.collection IS NULL THEN 0 ELSE j.due_at END AS next_at
             FROM github_current_eligible e
             LEFT JOIN sync_jobs j ON (j.collection, j.generation) = (e.collection, e.generation)
             WHERE j.collection IS NULL OR j.due_at IS NOT NULL
             ORDER BY next_at, e.repository_id LIMIT 1",
                [],
                |row| {
                    Ok(Due {
                        repository_id: row.get(0)?,
                        collection: row.get(1)?,
                        generation: row.get(2)?,
                        at_ms: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(storage)
    }

    fn enqueue(
        &self,
        repository_id: i64,
        now: i64,
        trigger: Option<TraceLink>,
    ) -> Result<Option<TraceLink>, Error> {
        let mut writer = self.write().map_err(|_| Error::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        resume_data_in(&tx, repository_id, "current")?;
        let scope = scope_in(&tx, repository_id)?;
        let state = job(&tx, &scope)?;
        if matches!(state.phase, Phase::Succeeded | Phase::Failed)
            || (state.phase == Phase::Queued && state.due_at == Some(0))
        {
            persist_job(
                &tx,
                &scope.collection,
                scope.generation,
                JobState::due(Phase::Queued, now),
            )
            .map_err(storage)?;
        }
        let existing = origin(&tx, &scope)?;
        let effective = existing.or(trigger);
        if existing.is_none() && effective.is_some() {
            save_origin(&tx, &scope, effective)?;
        }
        tx.commit().map_err(storage)?;
        Ok(effective)
    }

    fn resume_scope(&self, repository_id: i64) -> Result<Scope, Error> {
        let mut writer = self.write().map_err(|_| Error::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        resume_data_in(&tx, repository_id, "current")?;
        let scope = scope_in(&tx, repository_id)?;
        tx.commit().map_err(storage)?;
        Ok(scope)
    }

    fn claim(&self, scope: &Scope, request: ClaimRequest<'_>, now: i64) -> Result<Claim, Error> {
        let mut writer = self.write().map_err(|_| Error::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        if !authorized(&tx, scope)? {
            return Err(Error::Conflict);
        }
        let state = job(&tx, scope)?;
        let requested = state
            .request(request.manual, now)
            .or_else(|| state.request(false, now));
        if requested.is_none() {
            return Err(match state.phase {
                Phase::Retry => Error::Upstream,
                Phase::Throttled => Error::Throttled,
                _ => Error::Conflict,
            });
        }
        persist_job(
            &tx,
            &scope.collection,
            scope.generation,
            JobState {
                phase: Phase::Running,
                due_at: Some(now),
                failures: state.failures,
            },
        )
        .map_err(storage)?;
        let existing = origin(&tx, scope)?;
        let effective = existing.or(request.trigger).or_else(request.new_origin);
        if existing.is_none() && effective.is_some() {
            save_origin(&tx, scope, effective)?;
        }
        tx.commit().map_err(storage)?;
        Ok(Claim {
            failures: state.failures,
            origin: effective,
        })
    }

    fn finish(
        &self,
        scope: &Scope,
        state: JobState,
        error: Option<Error>,
        now: i64,
    ) -> Result<bool, Error> {
        let mut writer = self.write().map_err(|_| Error::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        if !authorized(&tx, scope)? || job(&tx, scope)?.phase != Phase::Running {
            return Ok(false);
        }
        let checked = state.phase == Phase::Succeeded;
        let state =
            if checked {
                let interval: u16 = tx.query_row(
                "SELECT interval_minutes FROM github_current_eligible WHERE collection = ?1",
                [&scope.collection], |row| row.get(0),
            ).map_err(storage)?;
                JobState::due(
                    Phase::Succeeded,
                    now.saturating_add(i64::from(interval) * 60_000),
                )
            } else {
                state
            };
        persist_job(&tx, &scope.collection, scope.generation, state).map_err(storage)?;
        if matches!(state.phase, Phase::Succeeded | Phase::Failed) {
            save_origin(&tx, scope, None)?;
        }
        tx.execute(
            "UPDATE github_code_scopes SET last_error = ?2,
                    last_checked_at_ms = CASE WHEN ?3 THEN ?4 ELSE last_checked_at_ms END
             WHERE collection = ?1",
            params![
                scope.collection,
                error.map(|error| error.to_string()),
                checked,
                now
            ],
        )
        .map_err(storage)?;
        tx.execute(
            "UPDATE github_graph_scopes SET last_error = ?2, last_checked_at_ms = CASE WHEN ?3 THEN ?4 ELSE last_checked_at_ms END WHERE collection = ?1",
            params![scope.collection, error.map(|error| error.to_string()), checked, now],
        ).map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(true)
    }
}

fn origin(database: &Connection, scope: &Scope) -> Result<Option<TraceLink>, Error> {
    let bytes: Option<[u8; 24]> = database
        .query_row(
            "SELECT github_current_origin FROM sync_jobs WHERE (collection, generation) = (?1, ?2)",
            params![scope.collection, scope.generation],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?
        .flatten();
    Ok(bytes.map(trace::decode))
}

fn save_origin(
    database: &Connection,
    scope: &Scope,
    origin: Option<TraceLink>,
) -> Result<(), Error> {
    let bytes = origin.map(trace::encode);
    database.execute(
        "UPDATE sync_jobs SET github_current_origin = ?3 WHERE (collection, generation) = (?1, ?2)",
        params![scope.collection, scope.generation, bytes],
    ).map(|_| ()).map_err(storage)
}

fn job(database: &Connection, scope: &Scope) -> Result<JobState, Error> {
    database.query_row(
        "SELECT phase, due_at, failures, generation FROM sync_jobs WHERE (collection, generation) = (?1, ?2)",
        params![scope.collection, scope.generation], read_job,
    ).optional().map_err(storage)
        .map(|state| state.map_or(JobState::due(Phase::Queued, 0), |job| job.state))
}
