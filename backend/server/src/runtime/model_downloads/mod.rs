//! App-session installation of pinned model weights through the shared outbound gate.

use std::io;
use std::net::SocketAddrV4;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ctxfab_inference::artifacts::{ARTIFACTS, ArtifactKind};
use hyper::Uri;
use serde::Serialize;
use tokio::fs;
use tokio::task::JoinHandle;

use super::outbound::{OutboundGate, OutboundPool, QuotaPolicy};
use crate::adapters::outbound::http::HttpTransport;

mod install;
mod transfer;

pub const BUCKET: &str = "model-weights";

pub struct Artifact {
    pub profile: &'static str,
    pub file: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
    pub url: Uri,
}

#[derive(Clone, Copy)]
pub enum Scope {
    HuggingFace,
    Loopback(SocketAddrV4),
}

impl Scope {
    fn permits(self, url: &Uri) -> bool {
        let Some(authority) = url.authority() else {
            return false;
        };
        if authority.as_str().contains('@') {
            return false;
        }
        let host = authority.host().to_ascii_lowercase();
        match self {
            Self::HuggingFace => {
                url.scheme_str() == Some("https")
                    && authority.port().is_none_or(|port| port.as_u16() == 443)
                    && (host == "huggingface.co" || host.ends_with(".hf.co"))
            }
            Self::Loopback(address) => {
                address.ip().is_loopback()
                    && url.scheme_str() == Some("http")
                    && host == address.ip().to_string()
                    && authority.port_u16() == Some(address.port())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    NotInstalled,
    Installing,
    Installed,
    Failed,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub profile: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ArtifactKind>,
    pub state: State,
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
}

impl DownloadError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::UnknownProfile => "unknown_model_profile",
            Self::Unavailable => "model_installation_unavailable",
            Self::Transfer => "model_transfer_failed",
            Self::Integrity => "model_integrity_failed",
        }
    }
}

impl From<io::Error> for DownloadError {
    fn from(_: io::Error) -> Self {
        Self::Unavailable
    }
}

struct Job {
    task: Option<JoinHandle<()>>,
    failure: Option<DownloadError>,
}

struct Jobs {
    accepting: bool,
    entries: Vec<Job>,
}

pub struct ModelDownloads {
    directory: PathBuf,
    transport: HttpTransport,
    scope: Scope,
    models: Vec<Artifact>,
    jobs: Mutex<Jobs>,
}

impl ModelDownloads {
    /// Constructs the production catalog from the pinned inference profiles.
    ///
    /// # Errors
    /// Returns an error if the pinned URLs or HTTP client cannot be configured.
    pub fn configured(directory: &Path, outbound: &OutboundPool) -> Result<Self, DownloadError> {
        let gate = outbound
            .gate(
                BUCKET,
                QuotaPolicy {
                    concurrency: NonZeroU16::MIN,
                    spacing: Duration::from_secs(1),
                },
            )
            .map_err(|_| DownloadError::Unavailable)?;
        let models = ARTIFACTS
            .iter()
            .map(|model| {
                let url = format!(
                    "https://huggingface.co/{}/resolve/{}/{}?download=true",
                    model.repository, model.revision, model.file,
                )
                .parse()
                .map_err(|_| DownloadError::Unavailable)?;
                Ok(Artifact {
                    profile: model.profile,
                    file: model.file,
                    bytes: model.bytes,
                    sha256: model.sha256,
                    url,
                })
            })
            .collect::<Result<Vec<_>, DownloadError>>()?;
        Self::new(directory.to_path_buf(), gate, Scope::HuggingFace, models)
    }

    /// Configures a catalog and its fixed source boundary.
    ///
    /// # Errors
    /// Returns an error if a source is outside the configured scope or the client cannot start.
    pub fn new(
        directory: PathBuf,
        gate: Arc<OutboundGate>,
        scope: Scope,
        models: Vec<Artifact>,
    ) -> Result<Self, DownloadError> {
        if models.iter().any(|model| !scope.permits(&model.url)) {
            return Err(DownloadError::Unavailable);
        }
        let transport = HttpTransport::new(gate).map_err(|_| DownloadError::Unavailable)?;
        let entries = models
            .iter()
            .map(|_| Job {
                task: None,
                failure: None,
            })
            .collect();
        Ok(Self {
            directory,
            transport,
            scope,
            models,
            jobs: Mutex::new(Jobs {
                accepting: true,
                entries,
            }),
        })
    }

    /// Starts one supervised installation for a catalog profile.
    ///
    /// # Errors
    /// Returns an error for an unknown profile or unavailable session.
    pub fn request(self: &Arc<Self>, profile: &str) -> Result<(), DownloadError> {
        let index = self
            .models
            .iter()
            .position(|model| model.profile == profile)
            .ok_or(DownloadError::UnknownProfile)?;
        let mut jobs = self.jobs.lock().map_err(|_| DownloadError::Unavailable)?;
        if !jobs.accepting {
            return Err(DownloadError::Unavailable);
        }
        let job = &mut jobs.entries[index];
        if job.task.is_none() {
            job.failure = None;
            let downloads = self.clone();
            job.task = Some(tokio::spawn(async move {
                let outcome = downloads.install(&downloads.models[index]).await;
                tracing::info!(
                    code = "model_installation",
                    profile = downloads.models[index].profile,
                    outcome = outcome.map_or_else(DownloadError::code, |()| "installed"),
                    "Model installation finished"
                );
                if let Ok(mut jobs) = downloads.jobs.lock() {
                    let job = &mut jobs.entries[index];
                    job.failure = outcome.err();
                    job.task = None;
                }
            }));
        }
        Ok(())
    }

    /// Returns the state and current bytes for all profiles.
    ///
    /// # Errors
    /// Returns an error if the session state or model directory is unreadable.
    pub async fn snapshot(&self) -> Result<Vec<Snapshot>, DownloadError> {
        let states = self
            .jobs
            .lock()
            .map_err(|_| DownloadError::Unavailable)?
            .entries
            .iter()
            .map(|job| (job.task.is_some(), job.failure))
            .collect::<Vec<_>>();
        let mut catalog = Vec::with_capacity(self.models.len());
        for (model, (running, failure)) in self.models.iter().zip(states) {
            let final_size = size(&self.directory.join(model.file)).await?;
            let staged = size(&self.partial_path(model))
                .await?
                .unwrap_or(0)
                .min(model.bytes);
            let state = if running {
                State::Installing
            } else if failure.is_some() {
                State::Failed
            } else if final_size == Some(model.bytes) {
                State::Installed
            } else {
                State::NotInstalled
            };
            let pinned = ARTIFACTS
                .iter()
                .find(|entry| entry.profile == model.profile);
            catalog.push(Snapshot {
                profile: model.profile,
                label: pinned.map(|entry| entry.label),
                kind: pinned.map(|entry| entry.kind),
                state,
                downloaded_bytes: if state == State::Installed {
                    model.bytes
                } else {
                    staged
                },
                total_bytes: model.bytes,
                error: failure.map(DownloadError::code),
            });
        }
        Ok(catalog)
    }

    pub async fn stop(&self) {
        let tasks = if let Ok(mut jobs) = self.jobs.lock() {
            jobs.accepting = false;
            jobs.entries
                .iter_mut()
                .filter_map(|job| job.task.take())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
    }

    fn partial_path(&self, model: &Artifact) -> PathBuf {
        self.directory.join(format!(".{}.part", model.file))
    }
}

async fn size(path: &Path) -> io::Result<Option<u64>> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(metadata.is_file().then_some(metadata.len())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
