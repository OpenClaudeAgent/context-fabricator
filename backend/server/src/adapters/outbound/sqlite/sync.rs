use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{SqliteStore, record, storage_failure};
use crate::application::{
    Failure,
    sync::{CollectionJobs, TrafficLedger},
};
use crate::domain::sync::{
    Admission, JobState, JobTransition, Outcome, Phase, Traffic, UNCERTAIN_COOLDOWN_MS,
    VersionedJob, minute_spacing,
};

impl SqliteStore {
    /// Marks transport reservations left by a prior process as uncertain.
    ///
    /// # Errors
    /// Returns a storage error if the reconciliation fails.
    pub fn recover_traffic(&self, now: i64) -> Result<(), Failure> {
        self.write()?
            .execute(
                "UPDATE outbound_buckets SET uncertain = uncertain + pending, pending = 0,
                        cooldown_until = max(cooldown_until, ?1) WHERE pending > 0",
                [now.saturating_add(UNCERTAIN_COOLDOWN_MS)],
            )
            .map_err(storage_failure)?;
        Ok(())
    }
}

impl TrafficLedger for SqliteStore {
    fn reserve(&self, bucket: &str, now: i64, spacing: i64) -> Result<Admission, Failure> {
        let mut writer = self.write()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_failure)?;
        tx.execute(
            "INSERT INTO outbound_buckets (bucket) VALUES (?1) ON CONFLICT DO NOTHING",
            [bucket],
        )
        .map_err(storage_failure)?;
        let (eligible, requested_rate): (i64, Option<u16>) = tx
            .query_row(
                "SELECT max(next_at, cooldown_until), requests_per_minute
                 FROM outbound_buckets WHERE bucket = ?1",
                [bucket],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(storage_failure)?;
        if eligible > now {
            return Ok(Admission::WaitUntil(eligible));
        }
        let spacing = requested_rate.map_or(spacing, |rate| spacing.max(minute_spacing(rate)));
        let window = now - now.rem_euclid(1_000);
        tx.execute(
            "UPDATE outbound_buckets SET next_at = ?2, attempts = attempts + 1,
                 pending = pending + 1,
                 window_attempts = CASE WHEN window_start = ?3 THEN window_attempts + 1 ELSE 1 END,
                 window_start = ?3, last_handoff = ?4
             WHERE bucket = ?1",
            params![bucket, now.saturating_add(spacing), window, now],
        )
        .map_err(storage_failure)?;
        tx.commit().map_err(storage_failure)?;
        Ok(Admission::Granted)
    }

    fn complete(&self, bucket: &str, outcome: Outcome, cooldown_until: i64) -> Result<(), Failure> {
        let changed = self
            .write()?
            .execute(
                "UPDATE outbound_buckets SET pending = pending - 1,
                 failures = failures + ?2, uncertain = uncertain + ?3,
                 cooldown_until = max(cooldown_until, ?4)
             WHERE bucket = ?1 AND pending > 0",
                params![
                    bucket,
                    outcome == Outcome::Failed,
                    outcome == Outcome::Uncertain,
                    cooldown_until
                ],
            )
            .map_err(storage_failure)?;
        if changed != 1 {
            return Err(Failure::Storage);
        }
        Ok(())
    }

    fn defer(&self, bucket: &str, cooldown_until: i64) -> Result<(), Failure> {
        self.write()?.execute(
            "UPDATE outbound_buckets SET cooldown_until = max(cooldown_until, ?2) WHERE bucket = ?1",
            params![bucket, cooldown_until],
        ).map_err(storage_failure)?;
        Ok(())
    }

    fn traffic(&self, bucket: &str) -> Result<Traffic, Failure> {
        self.read()?
            .query_row(
                "SELECT attempts, pending, uncertain, failures, window_start, window_attempts,
                    cooldown_until, requests_per_minute
             FROM outbound_buckets WHERE bucket = ?1",
                [bucket],
                |row| {
                    Ok(Traffic {
                        attempts: row.get(0)?,
                        pending: row.get(1)?,
                        uncertain: row.get(2)?,
                        failures: row.get(3)?,
                        window_start: row.get(4)?,
                        window_attempts: row.get(5)?,
                        cooldown_until: row.get(6)?,
                        requests_per_minute: row.get(7)?,
                    })
                },
            )
            .optional()
            .map(Option::unwrap_or_default)
            .map_err(storage_failure)
    }
}

impl CollectionJobs for SqliteStore {
    fn load_job(&self, collection: &str) -> Result<Option<VersionedJob>, Failure> {
        let database = self.read()?;
        database
            .query_row(
                "SELECT phase, due_at, failures, generation FROM sync_jobs
             WHERE collection = ?1 AND generation = coalesce(
                 (SELECT generation FROM collections WHERE name = ?1), 0)",
                [collection],
                read_job,
            )
            .optional()
            .map_err(storage_failure)
    }

    fn transition_job(
        &self,
        collection: &str,
        generation: u32,
        transition: JobTransition,
    ) -> Result<Option<u32>, Failure> {
        let mut writer = self.write()?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_failure)?;
        let (current_generation, lost) = tx
            .query_row(
                "SELECT generation, access_lost FROM collections WHERE name = ?1",
                [collection],
                |row| Ok((row.get::<_, u32>(0)?, row.get::<_, bool>(1)?)),
            )
            .optional()
            .map_err(storage_failure)?
            .unwrap_or_default();
        if current_generation != generation || lost {
            return Ok(None);
        }
        let state = match transition {
            JobTransition::Queue(due_at) => JobState::due(Phase::Queued, due_at),
            JobTransition::Start { now, failures } => JobState {
                phase: Phase::Running,
                due_at: Some(now),
                failures,
            },
            JobTransition::Finish(state) => {
                let running = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sync_jobs WHERE collection = ?1
                         AND generation = ?2 AND phase = 'running')",
                        params![collection, generation],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(storage_failure)?;
                if !running {
                    return Ok(None);
                }
                state
            }
        };
        let generation = if state.phase == Phase::Credentials {
            let next = generation.checked_add(1).ok_or(Failure::Storage)?;
            tx.execute(
                "INSERT INTO collections(name, generation, access_lost) VALUES (?1, ?2, 1)
                 ON CONFLICT(name) DO UPDATE SET generation = excluded.generation, access_lost = 1",
                params![collection, next],
            )
            .map_err(storage_failure)?;
            next
        } else {
            generation
        };
        persist_job(&tx, collection, generation, state).map_err(storage_failure)?;
        if let Some((code, message)) = state.phase.diagnostic() {
            record(&tx, code, message).map_err(storage_failure)?;
        }
        tx.commit().map_err(storage_failure)?;
        Ok(Some(generation))
    }
}

pub(super) fn read_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<VersionedJob> {
    let code: String = row.get(0)?;
    let phase = Phase::from_code(&code).ok_or(rusqlite::Error::InvalidQuery)?;
    Ok(VersionedJob {
        generation: row.get(3)?,
        state: JobState {
            phase,
            due_at: row.get(1)?,
            failures: row.get(2)?,
        },
    })
}

pub(super) fn persist_job(
    database: &Connection,
    collection: &str,
    generation: u32,
    state: JobState,
) -> rusqlite::Result<()> {
    database
        .execute(
            "INSERT INTO sync_jobs (collection, phase, due_at, failures, generation)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(collection) DO UPDATE SET phase = excluded.phase,
                 due_at = excluded.due_at, failures = excluded.failures,
                 generation = excluded.generation",
            params![
                collection,
                state.phase.code(),
                state.due_at,
                state.failures,
                generation
            ],
        )
        .map(|_| ())
}
