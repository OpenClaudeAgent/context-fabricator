//! Logical retained GitHub payload accounting, evaluated inside write transactions.
use super::SqliteStore;
use crate::domain::github::{Error, StorageBudget};
use rusqlite::{Connection, params};

pub(in crate::adapters::outbound::sqlite) fn used(
    connection: &Connection,
) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT coalesce(sum(bytes), 0) FROM github_storage_ledger",
        [],
        |row| row.get(0),
    )
}

pub(in crate::adapters::outbound::sqlite) fn fits(
    connection: &Connection,
    before: i64,
) -> rusqlite::Result<bool> {
    let (after, limit): (i64, i64) = connection.query_row(
        "SELECT (SELECT coalesce(sum(bytes),0) FROM github_storage_ledger), limit_bytes FROM github_storage_budget WHERE singleton=1", [],
        |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(after <= limit || after <= before)
}

impl SqliteStore {
    pub(super) fn github_storage(&self) -> Result<StorageBudget, Error> {
        let connection = self.read().map_err(|_| Error::Storage)?;
        let (revision, limit_bytes, bytes): (u32, i64, i64) = connection
            .query_row(
                "SELECT revision, limit_bytes, (SELECT coalesce(sum(bytes),0) FROM github_storage_ledger) FROM github_storage_budget WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|_| Error::Storage)?;
        Ok(StorageBudget {
            revision,
            limit_gb: u16::try_from(limit_bytes / 1_000_000_000).map_err(|_| Error::Storage)?,
            bytes,
        })
    }

    pub(super) fn update_github_storage(
        &self,
        revision: u32,
        limit_gb: u16,
    ) -> Result<StorageBudget, Error> {
        if !(1..=1000).contains(&limit_gb) {
            return Err(Error::InvalidStorageLimit);
        }
        let connection = self.write().map_err(|_| Error::Storage)?;
        let changed = connection.execute("UPDATE github_storage_budget SET revision=revision+1, limit_bytes=?2 WHERE singleton=1 AND revision=?1", params![revision, i64::from(limit_gb) * 1_000_000_000]).map_err(|_| Error::Storage)?;
        if changed != 1 {
            return Err(Error::Conflict);
        }
        drop(connection);
        self.github_storage()
    }
}

impl SqliteStore {
    pub(super) fn purge_github_data(&self, id: i64) -> Result<StorageBudget, Error> {
        let mut connection = self.write().map_err(|_| Error::Storage)?;
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| Error::Storage)?;
        let known: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM github_repositories WHERE id=?1 UNION ALL SELECT 1 FROM github_code_scopes WHERE repository_id=?1 UNION ALL SELECT 1 FROM github_graph_scopes WHERE repository_id=?1 UNION ALL SELECT 1 FROM github_history_lineage WHERE repository_id=?1 UNION ALL SELECT 1 FROM github_history_evidence WHERE repository_id=?1 UNION ALL SELECT 1 FROM github_actions_snapshots WHERE repository_id=?1 UNION ALL SELECT 1 FROM github_graph_publications WHERE repository_id=?1)", [id], |r| r.get(0)).map_err(|_| Error::Storage)?;
        if !known {
            return Err(Error::Access);
        }
        tx.execute(
            "UPDATE answer_history_settings SET purge_sequence=purge_sequence+1 WHERE singleton=1",
            [],
        )
        .map_err(|_| Error::Storage)?;
        tx.execute("INSERT INTO answer_history_purges(repository_id, sequence) SELECT ?1, purge_sequence FROM answer_history_settings WHERE singleton=1 ON CONFLICT(repository_id) DO UPDATE SET sequence=excluded.sequence", [id]).map_err(|_| Error::Storage)?;
        tx.execute("DELETE FROM answer_history_entries WHERE id IN (SELECT entry_id FROM answer_history_owners WHERE repository_id=?1)", [id]).map_err(|_| Error::Storage)?;
        tx.execute(
            "INSERT INTO github_data_paused(repository_id) VALUES(?1) ON CONFLICT DO NOTHING",
            [id],
        )
        .map_err(|_| Error::Storage)?;
        tx.execute(
            "UPDATE github_history_generations SET generation=generation+1 WHERE repository_id=?1",
            [id],
        )
        .map_err(|_| Error::Storage)?;
        tx.execute(
            "UPDATE github_actions_generations SET generation=generation+1 WHERE repository_id=?1",
            [id],
        )
        .map_err(|_| Error::Storage)?;
        tx.execute("UPDATE collections SET generation=generation+1 WHERE name IN (SELECT collection FROM github_code_scopes WHERE repository_id=?1 UNION SELECT collection FROM github_graph_scopes WHERE repository_id=?1)", [id]).map_err(|_| Error::Storage)?;
        for table in [
            "github_history_schedule",
            "github_history_evidence",
            "github_history_lineage",
            "github_actions_snapshots",
            "github_actions_schedule",
            "github_graph_publications",
        ] {
            tx.execute(&format!("DELETE FROM {table} WHERE repository_id=?1"), [id])
                .map_err(|_| Error::Storage)?;
        }
        for table in ["documents", "source_snapshots", "source_blobs", "sync_jobs"] {
            tx.execute(&format!("DELETE FROM {table} WHERE collection IN (SELECT collection FROM github_code_scopes WHERE repository_id=?1 UNION SELECT collection FROM github_graph_scopes WHERE repository_id=?1)"), [id]).map_err(|_| Error::Storage)?;
        }
        tx.commit().map_err(|_| Error::Storage)?;
        drop(connection);
        self.github_storage()
    }

    pub(super) fn github_data_state(
        &self,
        id: i64,
    ) -> Result<crate::domain::github::DataState, Error> {
        let connection = self.read().map_err(|_| Error::Storage)?;
        let paused = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM github_data_paused WHERE repository_id=?1)",
                [id],
                |r| r.get(0),
            )
            .map_err(|_| Error::Storage)?;
        Ok(crate::domain::github::DataState { paused })
    }

    pub(super) fn resume_github_data(&self, id: i64, stream: &str) -> Result<(), Error> {
        let connection = self.write().map_err(|_| Error::Storage)?;
        resume_data_in(&connection, id, stream)
    }
}

pub(super) fn resume_data_in(
    connection: &rusqlite::Connection,
    id: i64,
    stream: &str,
) -> Result<(), Error> {
    let predicate = match stream {
        "current" => "(r.current_code=1 OR r.code_graph=1) AND r.contents_access=1",
        "history" => "r.history=1 AND r.contents_access=1",
        "actions" => "r.actions=1 AND r.contents_access=1 AND r.actions_access=1",
        _ => return Err(Error::InvalidSelection),
    };
    let sql = format!(
        "DELETE FROM github_data_paused WHERE repository_id=?1 AND EXISTS (SELECT 1 FROM github_repositories r CROSS JOIN github_connection c WHERE r.id=?1 AND r.verified=1 AND c.singleton=1 AND c.reconnect=0 AND c.credential_ref IS NOT NULL AND {predicate})"
    );
    connection.execute(&sql, [id]).map_err(|_| Error::Storage)?;
    Ok(())
}
