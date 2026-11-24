//! Model catalog entries and installation state.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

pub mod discovery;

pub(crate) const fn staged_weight_peak(bytes: u64, staged_bytes: u64) -> u64 {
    bytes.saturating_sub(staged_bytes)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub profile: Cow<'static, str>,
    pub label: Option<Cow<'static, str>>,
    pub kind: Option<ArtifactKind>,
    pub source: Option<ModelSource>,
    pub file: Cow<'static, str>,
    pub remote_file: Option<Cow<'static, str>>,
    pub bytes: u64,
    pub sha256: Cow<'static, str>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ModelSource {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<Cow<'static, str>>,
    pub label: Cow<'static, str>,
    pub repository: Cow<'static, str>,
    pub revision: Cow<'static, str>,
    pub url: Cow<'static, str>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactKind {
    #[default]
    Generation,
    Embedding,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SetupRole {
    RecommendedAnswer,
    AlternativeAnswer,
    OptionalEmbedding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    NotInstalled,
    Installing,
    Pausing,
    Paused,
    Interrupted,
    Installed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallationStage {
    Preparing,
    Transferring,
    Verifying,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub profile: Cow<'static, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<Cow<'static, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ArtifactKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ModelSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_role: Option<SetupRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletion_block: Option<&'static str>,
    pub state: State,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation_stage: Option<InstallationStage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DownloadError {
    #[error("This model profile is not available for download.")]
    UnknownProfile,
    #[error("Model installation is unavailable.")]
    Unavailable,
    #[error("The model transfer failed. Try again to resume.")]
    Transfer,
    #[error("The downloaded model did not match its pinned size and SHA-256.")]
    Integrity,
    #[error("The inference storage limit or available disk space is too low.")]
    StorageLimit,
    #[error("The model is in use. Switch models or wait for the current operation to finish.")]
    InUse,
    #[error("The model file changed. Review the removal plan again.")]
    StaleRemoval,
    #[error("The download was paused.")]
    Paused,
}

impl DownloadError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnknownProfile => "unknown_model_profile",
            Self::Unavailable => "model_installation_unavailable",
            Self::Transfer => "model_transfer_failed",
            Self::Integrity => "model_integrity_failed",
            Self::StorageLimit => "model_storage_limit",
            Self::InUse => "model_in_use",
            Self::StaleRemoval => "stale_model_removal",
            Self::Paused => "model_download_paused",
        }
    }
}
