//! Session admission for the release-pinned runtime installer.

use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
};

use ctxfab_inference::supervisor::Runtime;
use serde::Serialize;
use tokio::sync::watch;

pub type PackageDownload<'a> = Pin<Box<dyn Future<Output = Result<PathBuf, String>> + Send + 'a>>;
pub const STORAGE_LIMIT: &str = "runtime_storage_limit";

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageIdentity {
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub expanded_bytes: u64,
    pub bundle_bytes: u64,
    pub bundle_manifest_sha256: String,
    pub apple_team_id: String,
    pub hosted_release: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(untagged)]
pub enum RuntimeIdentity {
    Archive(PackageIdentity),
    Bundled {
        #[serde(rename = "bundledManifestSha256")]
        manifest_sha256: String,
    },
}

pub trait Package: Send + Sync {
    fn identity(&self) -> Option<PackageIdentity> {
        None
    }
    fn archive_bytes(&self) -> u64;
    fn storage_peaks(&self) -> Option<(u64, Option<u64>)> {
        None
    }
    fn can_download(&self) -> bool {
        false
    }
    fn download(&self) -> PackageDownload<'_> {
        Box::pin(async { Err("No runtime download is configured".into()) })
    }
    /// # Errors
    /// Rejects a package that cannot be verified and staged.
    fn install(&self, archive: &Path) -> Result<PathBuf, String>;
}

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimePhase {
    #[default]
    Unavailable,
    AvailableForInstall,
    Installing,
    Ready,
    Failed,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub state: RuntimePhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<RuntimeEngine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub can_download: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'static str>,
}

#[derive(Clone, Serialize)]
pub struct RuntimeEngine {
    pub name: &'static str,
    pub build: String,
}

impl RuntimeStatus {
    #[must_use]
    pub const fn unavailable() -> Self {
        Self {
            state: RuntimePhase::Unavailable,
            engine: None,
            archive_bytes: None,
            can_download: None,
            error: None,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Installation {
    phase: RuntimePhase,
    error: Option<&'static str>,
}

pub struct RuntimeSetup {
    source: Source,
    engine: Option<RuntimeEngine>,
    runtime: Arc<Runtime>,
    installation: Mutex<Installation>,
    published: Box<dyn Fn() + Send + Sync>,
    closing: watch::Sender<bool>,
    ready: watch::Sender<bool>,
}

enum Source {
    Archive(Box<dyn Package>),
    Bundled(String),
}

impl RuntimeSetup {
    #[must_use]
    pub fn new(
        package: Box<dyn Package>,
        runtime: Arc<Runtime>,
        published: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::with_source(Source::Archive(package), runtime, published)
    }

    #[must_use]
    pub fn bundled(runtime: Arc<Runtime>, manifest_sha256: String) -> Self {
        Self::with_source(Source::Bundled(manifest_sha256), runtime, || {})
    }

    #[must_use]
    pub fn with_engine(mut self, engine: Option<RuntimeEngine>) -> Self {
        self.engine = engine;
        self
    }

    fn with_source(
        source: Source,
        runtime: Arc<Runtime>,
        published: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let phase = if runtime.is_available() {
            RuntimePhase::Ready
        } else if matches!(&source, Source::Bundled(_)) {
            RuntimePhase::Unavailable
        } else {
            RuntimePhase::AvailableForInstall
        };
        Self {
            source,
            engine: None,
            runtime,
            installation: Mutex::new(Installation { phase, error: None }),
            published: Box::new(published),
            closing: watch::channel(false).0,
            ready: watch::channel(matches!(phase, RuntimePhase::Ready)).0,
        }
    }

    #[must_use]
    pub fn status(&self) -> RuntimeStatus {
        self.snapshot(
            self.installation
                .lock()
                .map_or_else(|_| Installation::default(), |state| *state),
        )
    }

    #[must_use]
    pub fn identity(&self) -> Option<RuntimeIdentity> {
        match &self.source {
            Source::Archive(package) => package.identity().map(RuntimeIdentity::Archive),
            Source::Bundled(manifest_sha256) => Some(RuntimeIdentity::Bundled {
                manifest_sha256: manifest_sha256.clone(),
            }),
        }
    }

    fn package(&self) -> Option<&dyn Package> {
        match &self.source {
            Source::Archive(package) => Some(package.as_ref()),
            Source::Bundled(_) => None,
        }
    }

    #[must_use]
    pub fn storage_peaks(&self) -> Option<(u64, Option<u64>)> {
        if self.runtime.is_available() {
            None
        } else {
            self.package().and_then(Package::storage_peaks)
        }
    }

    #[must_use]
    pub fn install(&self, archive: &Path) -> RuntimeStatus {
        let Some(package) = self.package() else {
            return self.status();
        };
        if let Err(state) = self.begin() {
            return self.snapshot(state);
        }
        self.finish(
            package.install(archive),
            "The archive could not be installed. Choose the exact runtime package for this app release.",
        )
    }

    /// Schedules a session-owned transfer on the running backend task runtime.
    #[must_use]
    pub fn request_download(self: &Arc<Self>) -> RuntimeStatus {
        if !self.package().is_some_and(Package::can_download) {
            return self.status();
        }
        let installation = match self.begin() {
            Ok(installation) => installation,
            Err(state) => return self.snapshot(state),
        };
        let setup = self.clone();
        let mut closing = self.closing.subscribe();
        tokio::spawn(async move {
            let Some(package) = setup.package() else {
                return;
            };
            tokio::select! {
                biased;
                _ = closing.wait_for(|closing| *closing) => {}
                package = package.download() => {
                    setup.finish(
                        package,
                        "The runtime could not be downloaded or verified. Try again or choose the release ZIP.",
                    );
                }
            }
        });
        self.snapshot(installation)
    }

    pub(super) fn subscribe_ready(&self) -> watch::Receiver<bool> {
        self.ready.subscribe()
    }

    pub fn stop(&self) {
        self.closing.send_replace(true);
    }

    fn begin(&self) -> Result<Installation, Installation> {
        let mut state = self
            .installation
            .lock()
            .map_err(|_| Installation::default())?;
        if !matches!(
            state.phase,
            RuntimePhase::AvailableForInstall | RuntimePhase::Failed
        ) {
            return Err(*state);
        }
        *state = Installation {
            phase: RuntimePhase::Installing,
            error: None,
        };
        Ok(*state)
    }

    fn finish(&self, result: Result<PathBuf, String>, error: &'static str) -> RuntimeStatus {
        let error = if result.as_ref().is_err_and(|reason| reason == STORAGE_LIMIT) {
            "The inference storage limit or available disk space is too low."
        } else {
            error
        };
        let installed = result.is_ok_and(|binary| self.runtime.publish(binary).is_ok());
        let state = if installed {
            tracing::info!(
                code = "runtime_installed",
                "The verified inference runtime is available"
            );
            (self.published)();
            Installation {
                phase: RuntimePhase::Ready,
                error: None,
            }
        } else {
            tracing::warn!(
                code = "runtime_install_rejected",
                "The runtime archive could not be installed"
            );
            Installation {
                phase: RuntimePhase::Failed,
                error: Some(error),
            }
        };
        if let Ok(mut installation) = self.installation.lock() {
            *installation = state;
        }
        if installed {
            self.ready.send_replace(true);
        }
        self.snapshot(state)
    }

    fn snapshot(&self, installation: Installation) -> RuntimeStatus {
        let package = self.package();
        RuntimeStatus {
            state: installation.phase,
            engine: self.engine.clone(),
            archive_bytes: package.map(Package::archive_bytes),
            can_download: package
                .filter(|package| package.can_download())
                .map(|_| true),
            error: installation.error,
        }
    }
}
