//! Persisted download intent and its storage boundary.

use crate::application::inference_setup::SetupLease;
use crate::domain::model::DownloadError;

pub struct StoredDownload {
    pub profile: String,
    pub fingerprint: String,
    pub operation_id: String,
    pub phase: String,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct RemovalIntent {
    pub profile: String,
    pub fingerprint: String,
    pub identity: String,
}

pub trait DownloadJobStore: Send + Sync {
    /// # Errors
    /// Returns an error if saved download jobs cannot be read.
    fn load_download_jobs(&self) -> Result<Vec<StoredDownload>, DownloadError>;
    /// # Errors
    /// Returns an error if download intent cannot be saved.
    fn save_download_job(&self, job: &StoredDownload) -> Result<(), DownloadError>;
    /// Atomically claims a setup-owned child under the current setup lease.
    /// # Errors
    /// Returns an error when ownership or the approved previous job changed.
    fn save_setup_job(
        &self,
        _job: &StoredDownload,
        _lease: SetupLease,
        _expected_previous_job: Option<&str>,
    ) -> Result<(), DownloadError> {
        Err(DownloadError::Unavailable)
    }
    /// # Errors
    /// Returns an error if download intent cannot be cleared.
    fn remove_download_job(&self, profile: &str) -> Result<(), DownloadError>;
    /// # Errors
    /// Returns an error if intents cannot be read.
    fn load_removals(&self) -> Result<Vec<RemovalIntent>, DownloadError>;
    /// # Errors
    /// Returns an error if the intent cannot be saved.
    fn save_removal(&self, intent: &RemovalIntent) -> Result<(), DownloadError>;
    /// # Errors
    /// Returns an error if the intent cannot be cleared.
    fn clear_removal(&self, profile: &str) -> Result<(), DownloadError>;
}
