use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::{SqliteStore, invalidate_repositories};
use crate::application::github::current::Store;
use crate::domain::github::{Access, ConfiguredRepository, Error, current::Scope};

pub(super) mod jobs;

pub(super) fn configured(tx: &Connection, config: &ConfiguredRepository) -> Result<(), Error> {
    super::graph::configure(tx, config)?;
    let id = config.repository.id;
    let eligible =
        config.verified && config.features.current_code && config.contents == Access::Available;
    let branch = eligible.then_some(config.branch.as_str());
    retire(tx, Some(id), branch)?;
    if let Some(branch) = branch {
        let proposed = format!("github/current/{id}/{branch}");
        let collection: String = tx.query_row(
            "SELECT coalesce(
                (SELECT collection FROM github_code_scopes WHERE (repository_id, branch) = (?1, ?2)),
                CASE WHEN EXISTS(SELECT 1 FROM collections WHERE name = ?3)
                     THEN ?3 || '#' || lower(hex(randomblob(16))) ELSE ?3 END
            )",
            params![id, branch, proposed], |row| row.get(0),
        ).map_err(storage)?;
        tx.execute(
            "INSERT INTO collections (name) VALUES (?1)
             ON CONFLICT(name) DO UPDATE SET access_lost = 0, generation = generation + 1
             WHERE access_lost = 1",
            [&collection],
        )
        .map_err(storage)?;
        tx.execute(
            "INSERT INTO github_code_scopes (collection, repository_id, branch)
             VALUES (?1, ?2, ?3) ON CONFLICT(collection) DO NOTHING",
            params![collection, id, branch],
        )
        .map_err(storage)?;
        jobs::reschedule(tx, &collection, config.interval_minutes)?;
    }
    Ok(())
}

pub(super) fn retire(
    tx: &Connection,
    id: Option<i64>,
    keep_branch: Option<&str>,
) -> Result<(), Error> {
    // Graph-only owners are fenced together with the credential or repository binding.
    if keep_branch.is_none() && id.is_none() {
        super::graph::retire(tx, None, None)?;
    }
    tx.execute(
        "UPDATE collections SET generation = generation + 1, access_lost = 1
         WHERE access_lost = 0 AND name IN (
             SELECT collection FROM github_code_scopes
             WHERE (?1 IS NULL OR repository_id = ?1) AND (?2 IS NULL OR branch <> ?2)
         )",
        params![id, keep_branch],
    )
    .map_err(storage)?;
    Ok(())
}

impl Store for SqliteStore {
    fn publish_current(&self, scope: &Scope, stage: i64) -> Result<(), Error> {
        super::graph::publish(self, scope, stage)
    }

    fn refresh_graph_profile(&self, scope: &Scope, publication: i64) -> Result<bool, Error> {
        super::graph::refresh_profile(self, scope, publication)
    }

    fn current_scope(&self, id: i64) -> Result<Scope, Error> {
        let reader = self.read().map_err(|_| Error::Storage)?;
        scope_in(&reader, id)
    }

    fn check_scope(&self, scope: &Scope) -> Result<(), Error> {
        let reader = self.read().map_err(|_| Error::Storage)?;
        if authorized(&reader, scope)? {
            Ok(())
        } else {
            Err(Error::Conflict)
        }
    }

    fn expire_scope_credential(&self, scope: &Scope) -> Result<(), Error> {
        let mut writer = self.write().map_err(|_| Error::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        if !authorized(&tx, scope)? {
            return Ok(());
        }
        tx.execute(
            "UPDATE github_connection SET revision = revision + 1, reconnect = 1
             WHERE singleton = 1 AND credential_ref = ?1 AND reconnect = 0",
            [&scope.credential],
        )
        .map_err(storage)?;
        invalidate_repositories(&tx)?;
        tx.commit().map_err(storage)
    }

    fn retire_scope(&self, scope: &Scope) -> Result<(), Error> {
        let mut writer = self.write().map_err(|_| Error::Storage)?;
        let tx = writer
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        if !authorized(&tx, scope)? {
            return Ok(());
        }
        tx.execute(
            "UPDATE github_repositories SET contents_access = 0 WHERE id = ?1",
            [scope.repository_id],
        )
        .map_err(storage)?;
        tx.execute(
            "UPDATE github_connection SET revision = revision + 1 WHERE singleton = 1",
            [],
        )
        .map_err(storage)?;
        retire(&tx, Some(scope.repository_id), None)?;
        super::graph::retire(&tx, Some(scope.repository_id), None)?;
        tx.commit().map_err(storage)
    }
}

fn scope_in(database: &Connection, id: i64) -> Result<Scope, Error> {
    database
        .query_row(
            "SELECT e.repository_id, e.full_name, e.branch, e.collection, e.generation,
                    e.credential_ref, p.id
             FROM github_current_eligible e
             LEFT JOIN source_snapshots p ON p.collection = e.collection AND p.phase = 'published'
             WHERE e.repository_id = ?1",
            [id],
            |row| {
                Ok(Scope {
                    repository_id: row.get(0)?,
                    full_name: row.get(1)?,
                    branch: row.get(2)?,
                    collection: row.get(3)?,
                    generation: row.get(4)?,
                    credential: row.get(5)?,
                    predecessor: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(storage)?
        .ok_or(Error::Access)
}

pub(super) fn authorized(tx: &Connection, scope: &Scope) -> Result<bool, Error> {
    tx.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM github_current_eligible
             WHERE collection = ?1 AND generation = ?2 AND credential_ref = ?3
               AND repository_id = ?4 AND branch = ?5
         )",
        params![
            scope.collection,
            scope.generation,
            scope.credential,
            scope.repository_id,
            scope.branch
        ],
        |row| row.get(0),
    )
    .map_err(storage)
}

fn storage(_: rusqlite::Error) -> Error {
    Error::Storage
}
