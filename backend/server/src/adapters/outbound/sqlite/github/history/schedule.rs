use super::{Error, Scope, SqliteStore, storage, trace};
use crate::application::telemetry::TraceLink;
use crate::{
    application::github::history::{Attempts, schedule::ScheduleStore},
    domain::github::{Schedule, ScheduleDue},
};
use rusqlite::{Connection, OptionalExtension, params};

pub(super) fn status(db: &Connection, id: i64) -> Result<Option<Schedule>, Error> {
    db.query_row(
        "SELECT next_at_ms,last_checked_at_ms,last_error FROM github_history_eligible
         WHERE repository_id=?1 AND last_checked_at_ms IS NOT NULL",
        [id],
        |row| {
            Ok(Schedule {
                next_at_ms: row.get(0)?,
                last_checked_at_ms: row.get(1)?,
                last_error: row.get(2)?,
            })
        },
    )
    .optional()
    .map_err(storage)
}

pub(super) fn record(
    db: &Connection,
    scope: &Scope,
    expected_due_ms: i64,
    checked: i64,
    error: Option<&str>,
) -> Result<(), Error> {
    let updated = db.execute(
        "INSERT INTO github_history_schedule(repository_id,generation,last_checked_at_ms,last_error)
         SELECT repository_id,generation,?4,?5 FROM github_history_eligible
         WHERE repository_id=?1 AND generation=?2 AND credential_ref=?3 AND next_at_ms=?6
         ON CONFLICT(repository_id) DO UPDATE SET generation=excluded.generation,
            last_checked_at_ms=excluded.last_checked_at_ms,last_error=excluded.last_error,
            preparation=CASE WHEN github_history_schedule.generation=excluded.generation
                             THEN github_history_schedule.preparation ELSE '{}' END",
        params![
            scope.repository_id,
            scope.generation,
            scope.credential,
            checked,
            error,
            expected_due_ms,
        ],
    ).map_err(storage)?;
    if updated == 0 {
        return Err(Error::Conflict);
    }
    Ok(())
}

impl ScheduleStore for SqliteStore {
    fn preparation_attempts(&self, scope: &Scope) -> Result<Attempts, Error> {
        let db = self.read().map_err(|_| Error::Storage)?;
        let document: String = db.query_row("SELECT preparation FROM github_history_schedule WHERE repository_id=?1 AND generation=?2",
            params![scope.repository_id,scope.generation], |row| row.get(0)).optional().map_err(storage)?.ok_or(Error::Conflict)?;
        serde_json::from_str(&document).map_err(|_| Error::Storage)
    }

    fn save_preparation_attempts(
        &self,
        scope: &Scope,
        tip: &str,
        attempts: &Attempts,
    ) -> Result<(), Error> {
        let document = serde_json::to_string(attempts).map_err(|_| Error::Storage)?;
        let db = self.write().map_err(|_| Error::Storage)?;
        let rows = db.execute("UPDATE github_history_schedule SET preparation=?1 WHERE repository_id=?2 AND generation=?3
            AND EXISTS(SELECT 1 FROM github_history_eligible WHERE repository_id=?2 AND generation=?3 AND branch=?4)
            AND EXISTS(SELECT 1 FROM github_history_lineage WHERE repository_id=?2 AND branch=?4 AND json_extract(snapshot,'$.tip')=?5)",
            params![document,scope.repository_id,scope.generation,scope.branch,tip]).map_err(storage)?;
        if rows == 1 {
            Ok(())
        } else {
            Err(Error::Conflict)
        }
    }

    fn history_due(&self, id: Option<i64>) -> Result<Option<ScheduleDue>, Error> {
        let reader = self.read().map_err(|_| Error::Storage)?;
        reader
            .query_row(
                "SELECT repository_id, generation, next_at_ms, interval_minutes
             FROM github_history_eligible WHERE ?1 IS NULL OR repository_id = ?1
             ORDER BY next_at_ms, repository_id LIMIT 1",
                [id],
                |row| {
                    Ok(ScheduleDue {
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
    fn claim_history(
        &self,
        scope: &Scope,
        due: &ScheduleDue,
        trace: Option<TraceLink>,
    ) -> Result<(), Error> {
        if due.repository_id != scope.repository_id || due.generation != scope.generation {
            return Err(Error::Conflict);
        }
        let db = self.write().map_err(|_| Error::Storage)?;
        let changed = db
            .execute(
                "UPDATE github_history_generations SET latest_trace = ?6
             WHERE repository_id = ?1 AND generation = ?2
             AND EXISTS(SELECT 1 FROM github_history_eligible WHERE repository_id = ?1
                        AND generation = ?2 AND credential_ref = ?3
                        AND next_at_ms = ?4 AND interval_minutes = ?5)",
                params![
                    scope.repository_id,
                    scope.generation,
                    scope.credential,
                    due.at_ms,
                    due.interval_minutes,
                    trace.map(trace::encode),
                ],
            )
            .map_err(storage)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(Error::Conflict)
        }
    }

    fn history_failed(
        &self,
        scope: &Scope,
        expected_due_ms: i64,
        checked: i64,
        error: Error,
    ) -> Result<(), Error> {
        let db = self.write().map_err(|_| Error::Storage)?;
        record(
            &db,
            scope,
            expected_due_ms,
            checked,
            Some(error.schedule_code()),
        )
    }
}
