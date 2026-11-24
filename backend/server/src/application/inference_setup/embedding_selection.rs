//! Selects verified embedding weights independently of the answer model.

use std::sync::Arc;

use serde::Serialize;

use super::InferenceSetup;
use crate::application::{
    model_management::ModelManagement,
    retrieval::{Retrieval, RetrievalError},
};
use crate::domain::model::{DownloadError, SetupRole, State};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingModels {
    #[serde(skip_serializing_if = "Option::is_none")]
    active_profile: Option<String>,
    profiles: Vec<EmbeddingModel>,
}

#[derive(Serialize)]
struct EmbeddingModel {
    profile: String,
    label: String,
}

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingSelectionError {
    #[error("Install this embedding model before selecting it.")]
    NotFound,
    #[error("The embedding selection changed or another model operation is active. Try again.")]
    Conflict,
    #[error("The selected embedding model failed its readiness check.")]
    Readiness,
    #[error("The embedding model selection is unavailable.")]
    Unavailable,
}

impl From<DownloadError> for EmbeddingSelectionError {
    fn from(error: DownloadError) -> Self {
        match error {
            DownloadError::UnknownProfile => Self::NotFound,
            DownloadError::InUse => Self::Conflict,
            _ => Self::Unavailable,
        }
    }
}

/// # Errors
/// Returns an error if installed weights or the active index cannot be inspected.
pub async fn embedding_models(
    downloads: &ModelManagement,
    retrieval: &Arc<Retrieval>,
) -> Result<EmbeddingModels, EmbeddingSelectionError> {
    let profiles = downloads
        .snapshot()
        .await?
        .into_iter()
        .filter(|model| {
            model.state == State::Installed
                && matches!(model.setup_role, Some(SetupRole::OptionalEmbedding))
        })
        .map(|model| {
            let profile = model.profile.into_owned();
            let label = model
                .label
                .map_or_else(|| profile.clone(), std::borrow::Cow::into_owned);
            EmbeddingModel { profile, label }
        })
        .collect::<Vec<_>>();
    let retrieval = retrieval.clone();
    let state = tokio::task::spawn_blocking(move || retrieval.index_state())
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or(EmbeddingSelectionError::Unavailable)?;
    let active_profile = profiles
        .iter()
        .any(|model| model.profile == state.profile)
        .then_some(state.profile);
    Ok(EmbeddingModels {
        active_profile,
        profiles,
    })
}

impl InferenceSetup {
    /// # Errors
    /// Returns an error if the selected model is unavailable, unready, or a competing operation
    /// changes the installed model or index state.
    pub async fn select_embedding(
        self: &Arc<Self>,
        profile: String,
    ) -> Result<EmbeddingModels, EmbeddingSelectionError> {
        let setup = self.clone();
        // Retain admission until publication even if the HTTP client stops waiting.
        tokio::spawn(async move { setup.select_installed_embedding(profile).await })
            .await
            .map_err(|_| EmbeddingSelectionError::Unavailable)?
    }

    async fn select_installed_embedding(
        &self,
        profile: String,
    ) -> Result<EmbeddingModels, EmbeddingSelectionError> {
        let _admission = self
            .admission
            .try_lock()
            .map_err(|_| EmbeddingSelectionError::Conflict)?;
        if *self.closing.borrow() {
            return Err(EmbeddingSelectionError::Unavailable);
        }
        if self
            .status()
            .map_err(|_| EmbeddingSelectionError::Unavailable)?
            .is_some_and(|status| status.phase.is_active())
        {
            return Err(EmbeddingSelectionError::Conflict);
        }
        let reservation = self.downloads.reserve_embedding(&profile)?;
        if !reservation.is_installed().await? {
            return Err(EmbeddingSelectionError::NotFound);
        }

        let mut models = embedding_models(&self.downloads, &self.retrieval).await?;
        let retrieval = self.retrieval.clone();
        let journal = self.journal.clone();
        let target = profile.clone();
        tokio::task::spawn_blocking(move || {
            let _reservation = reservation;
            let previous = retrieval
                .index_state()
                .map_err(|_| EmbeddingSelectionError::Unavailable)?;
            let _prepared = retrieval
                .prepare_embedding(&target)
                .map_err(|error| match error {
                    RetrievalError::Busy => EmbeddingSelectionError::Conflict,
                    _ => EmbeddingSelectionError::Readiness,
                })?;
            let changed = retrieval
                .complete_setup_index(&target, |active, next| {
                    if previous.profile != active {
                        return Ok(None);
                    }
                    journal
                        .select_embedding(&previous, next)
                        .map_err(|_| RetrievalError::Storage)
                })
                .map_err(|_| EmbeddingSelectionError::Unavailable)?;
            changed
                .then_some(())
                .ok_or(EmbeddingSelectionError::Conflict)
        })
        .await
        .map_err(|_| EmbeddingSelectionError::Unavailable)??;
        models.active_profile = Some(profile);
        Ok(models)
    }
}
