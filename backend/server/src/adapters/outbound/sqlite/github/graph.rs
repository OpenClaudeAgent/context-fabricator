//! Source ownership, publication fencing and local navigation queries for GitHub graphs.
use super::{SqliteStore, current};
use crate::application::github::graph::{Query, Snapshot, State, Store};
use crate::domain::{
    corpus::snapshot::SnapshotError,
    github::{Access, ConfiguredRepository, Error, current::Scope, graph::PROFILE},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

mod projection;

pub(super) fn configure(tx: &Connection, config: &ConfiguredRepository) -> Result<(), Error> {
    let id = config.repository.id;
    let enabled = config.verified
        && config.features.code_graph
        && !config.features.current_code
        && config.contents == Access::Available;
    let branch = enabled.then_some(config.branch.as_str());
    retire(tx, Some(id), branch)?;
    if let Some(branch) = branch {
        tx.execute(
            "INSERT INTO github_graph_scopes(collection,repository_id,branch)
            VALUES('github/graph/' || ?1 || '/' || ?2 || '#' || lower(hex(randomblob(16))),?1,?2)
            ON CONFLICT(repository_id,branch) DO NOTHING",
            params![id, branch],
        )
        .map_err(storage)?;
        let collection: String = tx
            .query_row(
                "SELECT collection FROM github_graph_scopes WHERE repository_id=?1 AND branch=?2",
                params![id, branch],
                |r| r.get(0),
            )
            .map_err(storage)?;
        tx.execute(
            "INSERT INTO collections(name) VALUES (?1) ON CONFLICT(name) DO UPDATE
            SET access_lost=0,generation=generation+1 WHERE access_lost=1",
            [&collection],
        )
        .map_err(storage)?;
        current::jobs::reschedule(tx, &collection, config.interval_minutes)?;
    }
    Ok(())
}

pub(super) fn retire(tx: &Connection, id: Option<i64>, keep: Option<&str>) -> Result<(), Error> {
    tx.execute(
        "UPDATE collections SET access_lost=1,generation=generation+1
        WHERE access_lost=0 AND name IN (SELECT collection FROM github_graph_scopes
        WHERE (?1 IS NULL OR repository_id=?1) AND (?2 IS NULL OR branch<>?2))",
        params![id, keep],
    )
    .map(|_| ())
    .map_err(storage)
}

pub(super) fn publish(store: &SqliteStore, scope: &Scope, stage: i64) -> Result<(), Error> {
    let (general, graph, files) = {
        let mut db = store.read().map_err(|_| Error::Storage)?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(storage)?;
        if !current::authorized(&tx, scope)? {
            return Err(Error::Conflict);
        }
        let (general, graph): (bool, bool) = tx
            .query_row(
                "SELECT current_code, code_graph FROM github_repositories WHERE id=?1",
                [scope.repository_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(storage)?;
        let files = if graph {
            projection::files(&tx, stage, scope)?
        } else {
            None
        };
        (general, graph, files)
    };
    let prepared = if graph {
        Some(projection::prepare(files.as_deref())?)
    } else {
        None
    };
    store
        .publish_with_projection(stage, general, |tx| {
            if !current::authorized(tx, scope).map_err(|_| SnapshotError::Storage)? {
                return Err(SnapshotError::Conflict);
            }
            if let Some(prepared) = &prepared {
                prepared.publish(tx, scope, stage)?;
            }
            Ok(())
        })
        .map_err(snapshot_error)
}

pub(super) fn refresh_profile(
    store: &SqliteStore,
    scope: &Scope,
    stage: i64,
) -> Result<bool, Error> {
    let prepared = {
        let mut reader = store.read().map_err(|_| Error::Storage)?;
        let tx = reader.transaction().map_err(storage)?;
        if !stale_profile(&tx, scope, stage)? {
            return Ok(false);
        }
        // Read all bytes from the same authoritative manifest before releasing the read snapshot.
        projection::files(&tx, stage, scope)?
    };
    let prepared = projection::prepare(prepared.as_deref())?;
    let mut writer = store.write().map_err(|_| Error::Storage)?;
    let tx = writer
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    if !stale_profile(&tx, scope, stage)? {
        return Ok(false);
    }
    let before = super::storage::used(&tx).map_err(storage)?;
    prepared
        .publish(&tx, scope, stage)
        .map_err(snapshot_error)?;
    if !super::storage::fits(&tx, before).map_err(storage)? {
        return Err(Error::StorageLimit);
    }
    tx.commit().map_err(storage)?;
    Ok(true)
}

fn stale_profile(tx: &Connection, scope: &Scope, stage: i64) -> Result<bool, Error> {
    if scope.predecessor != Some(stage) || !current::authorized(tx, scope)? {
        return Err(Error::Conflict);
    }
    tx.query_row(
        "SELECT r.code_graph AND NOT EXISTS (
            SELECT 1 FROM github_graph_publications p
            WHERE p.repository_id=r.id AND p.branch=?5 AND p.revision=s.revision AND p.profile=?6
         ) FROM source_snapshots s JOIN github_repositories r ON r.id=?4
         WHERE s.id=?1 AND s.collection=?2 AND s.generation=?3 AND s.phase='published'",
        params![
            stage,
            scope.collection,
            scope.generation,
            scope.repository_id,
            scope.branch,
            PROFILE
        ],
        |row| row.get(0),
    )
    .optional()
    .map_err(storage)?
    .ok_or(Error::Conflict)
}

fn snapshot_error(error: SnapshotError) -> Error {
    match error {
        SnapshotError::Conflict => Error::Conflict,
        SnapshotError::AccessLost => Error::Access,
        SnapshotError::Invalid => Error::CollectionLimit,
        SnapshotError::Storage => Error::Storage,
        SnapshotError::StorageLimit => Error::StorageLimit,
    }
}

impl Store for SqliteStore {
    fn graph(&self, id: i64, query: &Query) -> Result<State, Error> {
        let mut db = self.read().map_err(|_| Error::Storage)?;
        let tx = db.transaction().map_err(storage)?;
        let active: Option<bool> = tx
            .query_row(
                "SELECT r.code_graph AND EXISTS (
                SELECT 1 FROM github_current_eligible e WHERE e.repository_id=r.id AND e.branch=?2
             ) FROM github_repositories r WHERE r.id=?1",
                params![id, query.branch],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage)?;
        let mut snapshot = tx
            .query_row(
                "SELECT revision, profile, coverage, roots FROM github_graph_publications
             WHERE repository_id=?1 AND branch=?2 AND (profile=?3 OR NOT ?4)",
                params![id, query.branch, PROFILE, active == Some(true)],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(storage)?
            .map(|(revision, profile, coverage, json)| {
                let roots = serde_json::from_str(&json).map_err(|_| Error::Storage)?;
                Ok::<_, Error>(Snapshot {
                    revision,
                    profile,
                    coverage,
                    roots,
                    hits: Vec::new(),
                    truncated: false,
                })
            })
            .transpose()?;
        if active.is_none() && snapshot.is_none() {
            return Err(Error::Access);
        }
        if let (Some(snapshot), Some(root), Some(symbol)) =
            (&mut snapshot, &query.root, &query.symbol)
        {
            let mut stmt = tx.prepare(
                "SELECT record FROM github_graph_occurrences WHERE repository_id=?1 AND branch=?2
                 AND root=?3 AND (name=?4 OR qualified_name=?4) ORDER BY ordinal LIMIT 51",
            ).map_err(storage)?;
            let mut rows = stmt
                .query(params![id, query.branch, root, symbol])
                .map_err(storage)?;
            while let Some(row) = rows.next().map_err(storage)? {
                if snapshot.hits.len() == 50 {
                    snapshot.truncated = true;
                    break;
                }
                let record: String = row.get(0).map_err(storage)?;
                snapshot
                    .hits
                    .push(serde_json::from_str(&record).map_err(|_| Error::Storage)?);
            }
        }
        Ok(State {
            repository_id: id,
            branch: query.branch.clone(),
            access: if active == Some(true) {
                "available"
            } else if snapshot.is_some() {
                "retained"
            } else {
                "unavailable"
            },
            snapshot,
        })
    }
}

fn storage(_: rusqlite::Error) -> Error {
    Error::Storage
}
