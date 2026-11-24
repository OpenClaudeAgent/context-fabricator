//! Durable lineage metadata, separate from both corpus and vector queues.

mod schedule;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{SqliteStore, trace};
use crate::application::github::history::{Reopen, Reopened, Store};
use crate::application::telemetry::TraceLink;
use crate::domain::github::{
    Error,
    history::{Scope, Snapshot, State},
};

type ReopenedRows = (String, Option<Vec<u8>>, Option<Vec<u8>>, bool);

impl Store for SqliteStore {
    fn history_scope(&self, id: i64) -> Result<Scope, Error> {
        let connection = self.read().map_err(|_| Error::Storage)?;
        read_scope(&connection, id)
    }

    fn history_state(&self, id: i64) -> Result<State, Error> {
        let mut reader = self.read().map_err(|_| Error::Storage)?;
        let connection = reader.transaction().map_err(storage)?;
        let (document, published_trace): (Option<String>, Option<[u8; 24]>) = connection
            .query_row(
                "SELECT h.snapshot, h.published_trace
                 FROM github_repositories r
                 LEFT JOIN github_history_lineage h ON (h.repository_id, h.branch) = (r.id, r.branch)
                 WHERE r.id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(storage)?
            .ok_or(Error::Access)?;
        Ok(State {
            schedule: schedule::status(&connection, id)?,
            published_trace: published_trace.map(trace::decode),
            snapshot: document
                .map(|value| serde_json::from_str(&value))
                .transpose()
                .map_err(|_| Error::Storage)?,
        })
    }

    fn check_comparison_scope(&self, scope: &Scope, snapshot: &str) -> Result<(), Error> {
        let connection = self.read().map_err(|_| Error::Storage)?;
        let matched: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM github_repositories r
             JOIN github_history_generations g ON g.repository_id = r.id
             JOIN github_history_lineage h ON (h.repository_id, h.branch) = (r.id, r.branch)
             CROSS JOIN github_connection c
             WHERE r.id = ?1 AND r.full_name = ?2 AND r.branch = ?3 AND g.generation = ?4
               AND r.history = 1 AND r.verified = 1 AND r.contents_access = 1
               AND c.singleton = 1 AND c.reconnect = 0 AND c.credential_ref = ?5 AND h.snapshot = ?6)",
            params![scope.repository_id, scope.full_name, scope.branch, scope.generation, scope.credential, snapshot],
            |row| row.get(0),
        ).map_err(storage)?;
        if matched {
            Ok(())
        } else {
            Err(Error::Conflict)
        }
    }

    fn publish_comparison(
        &self,
        scope: &Scope,
        snapshot: &str,
        child: &str,
        parent: Option<&str>,
        document: &crate::domain::github::history::HistoryEvidence,
        blobs: &[(String, Vec<u8>)],
    ) -> Result<(), Error> {
        let encoded = serde_json::to_string(document).map_err(|_| Error::Storage)?;
        let mut connection = self.write().map_err(|_| Error::Storage)?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        match read_scope(&tx, scope.repository_id) {
            Ok(current) if current == *scope => {}
            Ok(_) | Err(Error::Access) => return Err(Error::Conflict),
            Err(e) => return Err(e),
        }
        let before = super::storage::used(&tx).map_err(storage)?;
        let current: Option<String> = tx.query_row("SELECT snapshot FROM github_history_lineage WHERE repository_id = ?1 AND branch = ?2", params![scope.repository_id, scope.branch], |r| r.get(0)).optional().map_err(storage)?;
        if current.as_deref() != Some(snapshot) {
            return Err(Error::Conflict);
        }
        let data: Snapshot = serde_json::from_str(snapshot).map_err(|_| Error::Storage)?;
        let commit = data
            .commits
            .iter()
            .find(|c| c.id == child)
            .ok_or(Error::Conflict)?;
        if commit.parents.first().map(String::as_str) != parent {
            return Err(Error::Conflict);
        }
        let existing: Option<String> = tx.query_row("SELECT document FROM github_history_evidence WHERE repository_id = ?1 AND branch = ?2 AND child = ?3 AND parent = ?4 AND projection_version = 1", params![scope.repository_id, scope.branch, child, parent.unwrap_or("")], |r| r.get(0)).optional().map_err(storage)?;
        if let Some(existing) = existing {
            if existing != encoded {
                return Err(Error::Conflict);
            }
        } else {
            tx.execute("INSERT INTO github_history_evidence (repository_id, branch, child, parent, projection_version, document) VALUES (?1, ?2, ?3, ?4, 1, ?5)", params![scope.repository_id, scope.branch, child, parent.unwrap_or(""), encoded]).map_err(storage)?;
        }
        for (blob, bytes) in blobs {
            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT bytes FROM github_history_blobs WHERE blob = ?1",
                    [blob],
                    |r| r.get(0),
                )
                .optional()
                .map_err(storage)?;
            if let Some(existing) = existing {
                if existing != *bytes {
                    return Err(Error::Conflict);
                }
            } else {
                tx.execute(
                    "INSERT INTO github_history_blobs (blob, bytes) VALUES (?1, ?2)",
                    params![blob, bytes],
                )
                .map_err(storage)?;
            }
            tx.execute("INSERT OR IGNORE INTO github_history_evidence_blobs (repository_id, branch, child, parent, projection_version, blob) VALUES (?1, ?2, ?3, ?4, 1, ?5)", params![scope.repository_id, scope.branch, child, parent.unwrap_or(""), blob]).map_err(storage)?;
        }
        if !super::storage::fits(&tx, before).map_err(storage)? {
            return Err(Error::StorageLimit);
        }
        tx.commit().map_err(storage)
    }

    fn read_comparison_evidence(
        &self,
        id: i64,
        child: &str,
    ) -> Result<Option<crate::domain::github::history::HistoryEvidence>, Error> {
        let connection = self.read().map_err(|_| Error::Storage)?;
        let row: Option<(String, Option<String>)> = connection.query_row(
            "SELECT h.snapshot, e.document FROM github_repositories r
             JOIN github_history_generations g ON g.repository_id = r.id
             JOIN github_history_lineage h ON (h.repository_id, h.branch) = (r.id, r.branch)
             CROSS JOIN github_connection c
             JOIN json_each(h.snapshot, '$.commits') item ON json_extract(item.value, '$.id') = ?2
             LEFT JOIN github_history_evidence e ON e.repository_id = r.id AND e.branch = r.branch AND e.child = ?2 AND e.parent = COALESCE(json_extract(item.value, '$.parents[0]'), '') AND e.projection_version = 1
             WHERE r.id = ?1 AND r.history = 1 AND r.verified = 1 AND r.contents_access = 1
               AND c.singleton = 1 AND c.reconnect = 0 AND c.credential_ref IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM github_data_paused p WHERE p.repository_id = r.id)",
            params![id, child], |r| Ok((r.get(0)?, r.get(1)?))).optional().map_err(storage)?;
        let Some((snapshot, document)) = row else {
            return Ok(None);
        };
        let snapshot: Snapshot = serde_json::from_str(&snapshot).map_err(|_| Error::Storage)?;
        let Some(commit) = snapshot.commits.iter().find(|c| c.id == child) else {
            return Ok(None);
        };
        let Some(document) = document else {
            return Ok(None);
        };
        let value: crate::domain::github::history::HistoryEvidence =
            serde_json::from_str(&document).map_err(|_| Error::Storage)?;
        if value.parent.as_deref() != commit.parents.first().map(String::as_str) {
            return Ok(None);
        }
        Ok(Some(value))
    }

    fn reopen_hunk(&self, selection: &Reopen<'_>) -> Result<Option<Reopened>, Error> {
        use crate::domain::github::history::{HistoryEvidence, hunks};
        let Reopen {
            repository_id: id,
            branch,
            child,
            parent,
            change,
            hunk,
        } = *selection;
        let connection = self.read().map_err(|_| Error::Storage)?;
        let before = format!("$.changes[{change}].projection.hunks[{hunk}].before.blob");
        let after = format!("$.changes[{change}].projection.hunks[{hunk}].after.blob");
        let row: Option<ReopenedRows> = connection.query_row(
            "SELECT e.document, b.bytes, a.bytes,
                    NOT coalesce(r.history AND r.verified AND r.contents_access
                                 AND NOT c.reconnect AND c.credential_ref IS NOT NULL, 0)
             FROM github_history_lineage h
             LEFT JOIN github_repositories r ON (r.id, r.branch) = (h.repository_id, h.branch)
             CROSS JOIN github_connection c
             JOIN json_each(h.snapshot, '$.commits') item ON json_extract(item.value, '$.id') = ?2
             JOIN github_history_evidence e ON e.repository_id = h.repository_id AND e.branch = h.branch AND e.child = ?2
               AND e.parent = COALESCE(json_extract(item.value, '$.parents[0]'), '') AND e.projection_version = 1
             LEFT JOIN github_history_evidence_blobs rb ON (rb.repository_id,rb.branch,rb.child,rb.parent,rb.projection_version) = (e.repository_id,e.branch,e.child,e.parent,e.projection_version) AND rb.blob = json_extract(e.document, ?3)
             LEFT JOIN github_history_blobs b ON b.blob = rb.blob
             LEFT JOIN github_history_evidence_blobs ra ON (ra.repository_id,ra.branch,ra.child,ra.parent,ra.projection_version) = (e.repository_id,e.branch,e.child,e.parent,e.projection_version) AND ra.blob = json_extract(e.document, ?4)
             LEFT JOIN github_history_blobs a ON a.blob = ra.blob
             WHERE h.repository_id = ?1 AND h.branch = ?5 AND e.parent = ?6 AND c.singleton = 1
               AND NOT EXISTS (SELECT 1 FROM github_data_paused p WHERE p.repository_id = h.repository_id)",
            params![id, child, before, after, branch, parent.unwrap_or("")],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional().map_err(storage)?;
        let Some((document, old, new, retained)) = row else {
            return Ok(None);
        };
        let evidence: HistoryEvidence =
            serde_json::from_str(&document).map_err(|_| Error::Storage)?;
        let Some(change) = evidence.changes.get(change) else {
            return Ok(None);
        };
        if change.projection.coverage != "complete" {
            return Ok(None);
        }
        let Some(hunk) = change.projection.hunks.get(hunk) else {
            return Ok(None);
        };
        for (anchor, bytes, side) in [
            (&hunk.before, old.as_deref(), &change.change.before),
            (&hunk.after, new.as_deref(), &change.change.after),
        ] {
            match (anchor, bytes, side) {
                (Some(anchor), Some(bytes), Some(side)) => hunks::reopen(anchor, bytes, side)?,
                (None, None, _) => {}
                _ => return Err(Error::Storage),
            }
        }
        let adjacent = old
            .as_deref()
            .zip(new.as_deref())
            .map_or_else(Vec::new, |(old, new)| {
                hunks::adjacent(&change.projection, hunk, old, new)
            });
        Ok(Some(Reopened {
            hunk: hunk.clone(),
            retained,
            adjacent,
        }))
    }

    fn publish_history(
        &self,
        scope: &Scope,
        snapshot: &Snapshot,
        expected_due_ms: i64,
        checked: i64,
        producer: Option<TraceLink>,
    ) -> Result<(), Error> {
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
        transaction
            .execute(
                "INSERT INTO github_history_lineage (repository_id, branch, snapshot, published_trace)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (repository_id, branch) DO UPDATE SET
                    snapshot = excluded.snapshot, published_trace = excluded.published_trace
                 WHERE snapshot <> excluded.snapshot OR published_trace IS NOT excluded.published_trace",
                params![scope.repository_id, scope.branch, encoded, producer.map(trace::encode)],
            )
            .map_err(storage)?;
        if !super::storage::fits(&transaction, before).map_err(storage)? {
            return Err(Error::StorageLimit);
        }
        schedule::record(&transaction, scope, expected_due_ms, checked, None)?;
        transaction.commit().map_err(storage)
    }
}

fn read_scope(connection: &Connection, id: i64) -> Result<Scope, Error> {
    connection
        .query_row(
            "SELECT full_name, branch, generation, credential_ref
             FROM github_history_eligible WHERE repository_id=?1",
            [id],
            |row| {
                Ok(Scope {
                    repository_id: id,
                    full_name: row.get(0)?,
                    branch: row.get(1)?,
                    generation: row.get(2)?,
                    credential: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(storage)?
        .ok_or(Error::Access)
}

fn storage(_: rusqlite::Error) -> Error {
    Error::Storage
}
