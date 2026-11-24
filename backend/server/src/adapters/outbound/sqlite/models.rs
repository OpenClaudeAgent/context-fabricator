use ctxfab_inference::generation::{SelectionStore, SelectionStoreError};
use rusqlite::{OptionalExtension, params};

use super::SqliteStore;
use crate::application::model_management::{ModelInventory, RetainedModel};
use crate::domain::model::{
    Artifact, ArtifactKind, DownloadError,
    discovery::{
        approved_imported_embedding, hex, managed_profile, repository_valid, variant_quantization,
    },
};

impl ModelInventory for SqliteStore {
    fn load_models(&self) -> Result<Vec<RetainedModel>, DownloadError> {
        let connection = self.read().map_err(|_| DownloadError::Unavailable)?;
        let mut query = connection
            .prepare("SELECT artifact, verified FROM model_inventory ORDER BY rowid")
            .map_err(|_| DownloadError::Unavailable)?;
        let rows = query
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get(1)?)))
            .map_err(|_| DownloadError::Unavailable)?;
        rows.map(|row| {
            let (artifact, verified) = row.map_err(|_| DownloadError::Unavailable)?;
            let artifact =
                serde_json::from_str(&artifact).map_err(|_| DownloadError::Unavailable)?;
            validate(&artifact)?;
            Ok(RetainedModel { artifact, verified })
        })
        .collect()
    }

    fn retain_model(&self, model: &Artifact) -> Result<(), DownloadError> {
        validate(model)?;
        let data = serde_json::to_string(model).map_err(|_| DownloadError::Unavailable)?;
        self.write()
            .map_err(|_| DownloadError::Unavailable)?
            .execute(
                "INSERT INTO model_inventory (profile, artifact) VALUES (?, ?)
                 ON CONFLICT (profile) DO NOTHING",
                params![model.profile.as_ref(), data],
            )
            .map_err(|_| DownloadError::Unavailable)?;
        Ok(())
    }

    fn mark_verified(&self, profile: &str, verified: bool) -> Result<(), DownloadError> {
        let changed = self
            .write()
            .map_err(|_| DownloadError::Unavailable)?
            .execute(
                "UPDATE model_inventory SET verified = ? WHERE profile = ?",
                params![verified, profile],
            )
            .map_err(|_| DownloadError::Unavailable)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(DownloadError::Unavailable)
        }
    }
}

fn validate(model: &Artifact) -> Result<(), DownloadError> {
    let source = model.source.as_ref().ok_or(DownloadError::Unavailable)?;
    let file = model
        .remote_file
        .as_deref()
        .ok_or(DownloadError::Unavailable)?;
    if !repository_valid(&source.repository)
        || !hex(&source.revision, 40)
        || variant_quantization(file).is_none()
        || !hex(&model.sha256, 64)
        || !(1..=5_000_000_000).contains(&model.bytes)
    {
        return Err(DownloadError::Unavailable);
    }
    let supported = match model.kind {
        Some(ArtifactKind::Generation) => {
            model.profile == managed_profile(&source.repository, &source.revision, file)
                && model.file == format!("{}.gguf", model.profile)
        }
        Some(ArtifactKind::Embedding) => approved_imported_embedding(model),
        None => false,
    };
    if !supported
        || source.url
            != format!(
                "https://huggingface.co/{}/tree/{}",
                source.repository, source.revision
            )
    {
        return Err(DownloadError::Unavailable);
    }
    Ok(())
}

pub(super) fn save_selection(
    connection: &rusqlite::Connection,
    profile: &str,
    fingerprint: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO generation_selection (singleton, profile, fingerprint) VALUES (1, ?1, ?2)
         ON CONFLICT(singleton) DO UPDATE SET profile = excluded.profile,
             fingerprint = excluded.fingerprint, revision = generation_selection.revision + 1",
        params![profile, fingerprint],
    )?;
    Ok(())
}

impl SelectionStore for SqliteStore {
    fn load(&self) -> Result<Option<(String, String)>, SelectionStoreError> {
        let connection = self.read().map_err(|_| SelectionStoreError)?;
        connection
            .query_row(
                "SELECT profile, fingerprint FROM generation_selection WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|_| SelectionStoreError)
    }
    fn save(&self, profile: &str, fingerprint: &str) -> Result<(), SelectionStoreError> {
        let connection = self.write().map_err(|_| SelectionStoreError)?;
        save_selection(&connection, profile, fingerprint).map_err(|_| SelectionStoreError)
    }
}
