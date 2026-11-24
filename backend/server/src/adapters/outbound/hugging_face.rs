//! Installs catalog-pinned weights from Hugging Face into the local model directory.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddrV4;
use std::path::PathBuf;
use std::sync::Arc;

use hyper::Uri;

use super::http::HttpTransport;
use super::{artifact_storage::DesktopArtifactBudget, sqlite::SqliteStore};
use crate::application::model_management::{ArtifactResult, ArtifactStore, LocalArtifactState};
use crate::domain::model::{Artifact, DownloadError, InstallationStage};
use crate::runtime::outbound::OutboundGate;

mod catalog;
mod discovery;
mod install;
mod representation;
mod transfer;

pub const BUCKET: &str = "model-weights";

pub struct HuggingFace {
    directory: PathBuf,
    transport: HttpTransport,
    scope: Scope,
    urls: HashMap<&'static str, Uri>,
    managed: Option<ManagedWeights>,
    transfer_slot: Option<Arc<tokio::sync::Semaphore>>,
}

struct ManagedWeights {
    receipts: Arc<SqliteStore>,
    budget: DesktopArtifactBudget,
}

#[derive(Clone, Copy)]
enum Scope {
    HuggingFace,
    Loopback(SocketAddrV4),
}

impl Scope {
    fn origin(self) -> String {
        match self {
            Self::HuggingFace => "https://huggingface.co".into(),
            Self::Loopback(address) => format!("http://{address}"),
        }
    }

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

impl HuggingFace {
    /// Returns the pinned model catalog and its installation adapter.
    ///
    /// # Errors
    /// Returns an error if the pinned URLs or HTTP client cannot be configured.
    pub fn production(
        directory: PathBuf,
        gate: Arc<OutboundGate>,
    ) -> Result<(Vec<Artifact>, Self), DownloadError> {
        Self::with_catalog(directory, gate, Scope::HuggingFace)
    }

    #[must_use]
    pub fn with_transfer_slot(mut self, slot: Arc<tokio::sync::Semaphore>) -> Self {
        self.transfer_slot = Some(slot);
        self
    }

    #[must_use]
    pub fn with_managed_weights(
        mut self,
        receipts: Arc<SqliteStore>,
        budget: DesktopArtifactBudget,
    ) -> Self {
        self.managed = Some(ManagedWeights { receipts, budget });
        self
    }

    /// Installs and discovers models using the supervised app's loopback fixture.
    /// # Errors
    /// Returns a configuration error if HTTP setup fails.
    pub fn discovery_fixture(
        directory: PathBuf,
        gate: Arc<OutboundGate>,
        address: SocketAddrV4,
    ) -> Result<(Vec<Artifact>, Self), DownloadError> {
        Self::with_catalog(directory, gate, Scope::Loopback(address))
    }

    fn with_catalog(
        directory: PathBuf,
        gate: Arc<OutboundGate>,
        scope: Scope,
    ) -> Result<(Vec<Artifact>, Self), DownloadError> {
        let adapter = Self::new(directory, gate, scope, HashMap::new())?;
        Ok((catalog::ARTIFACTS.to_vec(), adapter))
    }

    fn artifact_url(&self, model: &Artifact) -> Result<Uri, DownloadError> {
        if let Some(url) = self.urls.get(model.profile.as_ref()) {
            return Ok(url.clone());
        }
        let source = model.source.as_ref().ok_or(DownloadError::UnknownProfile)?;
        let file = model.remote_file.as_ref().unwrap_or(&model.file);
        let url = format!(
            "{}/{}/resolve/{}/{file}?download=true",
            self.scope.origin(),
            source.repository,
            source.revision,
        )
        .parse()
        .map_err(|_| DownloadError::Unavailable)?;
        if !self.scope.permits(&url) {
            return Err(DownloadError::Unavailable);
        }
        Ok(url)
    }

    /// Configures the same adapter against a single loopback origin for qualification.
    ///
    /// # Errors
    /// Rejects origins outside the given loopback address and an unavailable HTTP client.
    pub fn fixture(
        directory: PathBuf,
        gate: Arc<OutboundGate>,
        address: SocketAddrV4,
        urls: HashMap<&'static str, Uri>,
    ) -> Result<Self, DownloadError> {
        Self::new(directory, gate, Scope::Loopback(address), urls)
    }

    fn new(
        directory: PathBuf,
        gate: Arc<OutboundGate>,
        scope: Scope,
        urls: HashMap<&'static str, Uri>,
    ) -> Result<Self, DownloadError> {
        if urls.values().any(|url| !scope.permits(url)) {
            return Err(DownloadError::Unavailable);
        }
        let transport = HttpTransport::new(gate).map_err(|_| DownloadError::Unavailable)?;
        Ok(Self {
            directory,
            transport,
            scope,
            urls,
            managed: None,
            transfer_slot: None,
        })
    }
}

impl ArtifactStore for HuggingFace {
    fn install<'a>(&'a self, artifact: &'a Artifact) -> ArtifactResult<'a, ()> {
        Box::pin(async move {
            let (_sender, pause) = tokio::sync::watch::channel(false);
            self.install_controlled(artifact, pause).await
        })
    }

    fn install_controlled<'a>(
        &'a self,
        artifact: &'a Artifact,
        pause: tokio::sync::watch::Receiver<bool>,
    ) -> ArtifactResult<'a, ()> {
        let (stage, _) = tokio::sync::watch::channel(InstallationStage::Preparing);
        self.install_tracked(artifact, pause, stage)
    }

    fn install_tracked<'a>(
        &'a self,
        artifact: &'a Artifact,
        pause: tokio::sync::watch::Receiver<bool>,
        stage: tokio::sync::watch::Sender<InstallationStage>,
    ) -> ArtifactResult<'a, ()> {
        Box::pin(self.install_verified(artifact, pause, stage))
    }
    fn local_state<'a>(&'a self, artifact: &'a Artifact) -> ArtifactResult<'a, LocalArtifactState> {
        Box::pin(self.installed_and_staged(artifact))
    }

    fn remove(&self, artifact: &Artifact) -> Result<(), DownloadError> {
        self.remove_installed(artifact)
    }
    fn requires_guarded_removal(&self) -> bool {
        self.managed.is_some()
    }
    fn removal_identity(
        &self,
        artifact: &Artifact,
    ) -> Result<Option<(u64, String)>, DownloadError> {
        self.installed_identity(artifact)
    }
    // App-owned operations are serialized by ModelManagement. A separate process
    // can still replace the path between the identity check and unlink.
    fn remove_if_identity(&self, artifact: &Artifact, identity: &str) -> Result<(), DownloadError> {
        if self
            .installed_identity(artifact)?
            .as_ref()
            .is_none_or(|(_, current)| current != identity)
        {
            return Err(DownloadError::StaleRemoval);
        }
        self.remove_installed(artifact)
    }
    fn discard_partial(&self, artifact: &Artifact) -> Result<(), DownloadError> {
        self.remove_partial(artifact)
    }
}

impl From<io::Error> for DownloadError {
    fn from(_: io::Error) -> Self {
        Self::Unavailable
    }
}
