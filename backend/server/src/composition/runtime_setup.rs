use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ctxfab_inference::supervisor::Runtime;
#[cfg(target_os = "linux")]
use ctxfab_server::adapters::outbound::linux_runtime as installed;
#[cfg(target_os = "macos")]
use ctxfab_server::adapters::outbound::managed_runtime::bundled as installed;
use ctxfab_server::{
    application::runtime_setup::{RuntimeEngine, RuntimeSetup},
    domain::model::DownloadError,
};

pub(super) enum DesktopRuntime {
    Bundled(installed::BundledRuntime),
    #[cfg(target_os = "macos")]
    Archive(macos::PinnedPackage),
}

impl DesktopRuntime {
    pub(super) fn resolve_bundled() -> Result<Option<Self>, DownloadError> {
        bundled_runtime()
            .map(|runtime| runtime.map(Self::Bundled))
            .map_err(|error| {
                tracing::warn!(
                    code = "bundled_runtime_rejected", %error,
                    "The embedded inference engine could not be verified"
                );
                DownloadError::Unavailable
            })
    }

    #[cfg(target_os = "macos")]
    pub(super) fn resolve_archive(
        directory: &Path,
        outbound: &ctxfab_server::runtime::outbound::OutboundPool,
        budget: ctxfab_server::adapters::outbound::artifact_storage::DesktopArtifactBudget,
        transfer_slot: Arc<tokio::sync::Semaphore>,
    ) -> Result<Option<Self>, DownloadError> {
        macos::PinnedPackage::for_release(directory, outbound, budget, transfer_slot)
            .map(|package| package.map(Self::Archive))
            .map_err(|_| DownloadError::Unavailable)
    }

    pub(super) fn load(&self, runtime: &Runtime) -> Result<(), DownloadError> {
        match self {
            Self::Bundled(bundle) => runtime
                .publish(bundle.binary.clone())
                .map_err(|_| DownloadError::Unavailable),
            #[cfg(target_os = "macos")]
            Self::Archive(package) => {
                package.load(runtime);
                Ok(())
            }
        }
    }

    pub(super) fn into_setup(
        self,
        runtime: Arc<Runtime>,
        #[cfg(target_os = "macos")] published: impl Fn() + Send + Sync + 'static,
    ) -> RuntimeSetup {
        match self {
            Self::Bundled(bundle) => {
                #[cfg(target_os = "linux")]
                let build = Some(bundle.build);
                #[cfg(target_os = "macos")]
                let build = bundle.build_version();
                let engine = build.map(|build| RuntimeEngine {
                    name: "llama.cpp",
                    build,
                });
                RuntimeSetup::bundled(runtime, bundle.manifest_sha256).with_engine(engine)
            }
            #[cfg(target_os = "macos")]
            Self::Archive(package) => RuntimeSetup::new(Box::new(package), runtime, published),
        }
    }
}

fn bundled_runtime() -> io::Result<Option<installed::BundledRuntime>> {
    #[cfg(all(target_os = "macos", debug_assertions))]
    if std::env::var_os("CTXFAB_TEST_PREVIEW_RUNTIME").as_deref() == Some(std::ffi::OsStr::new("1"))
        || std::env::var_os("CTXFAB_INFERENCE_DIRECTORY").is_some()
    {
        return Ok(None);
    }
    let helper = std::env::current_exe()?.canonicalize()?;
    installed::verify(&helper).map_err(io::Error::other)
}

pub(crate) fn verify_worker(binary: &Path, model: &Path, alias: &str) -> io::Result<PathBuf> {
    if let Some(bundle) = bundled_runtime()? {
        use ctxfab_server::adapters::outbound::sqlite::verified_weights::guardian_weight;
        if binary != bundle.binary || !guardian_weight(model, alias)? {
            return Err(io::Error::other(
                "bundled worker or weight verification failed",
            ));
        }
        return Ok(bundle.binary);
    }
    #[cfg(target_os = "macos")]
    if let Some(pin) = macos::release_pin()? {
        use ctxfab_server::adapters::outbound::{
            managed_runtime, sqlite::verified_weights::guardian_weight,
        };
        let worker = managed_runtime::verify_worker(binary, &pin)
            .map_err(|_| io::Error::other("managed runtime verification failed"))?;
        let support = model
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| io::Error::other("managed weight verification failed"))?;
        if !worker.starts_with(support.canonicalize()?.join("managed-runtime"))
            || !guardian_weight(model, alias)?
        {
            return Err(io::Error::other("managed weight verification failed"));
        }
        return Ok(worker);
    }
    Ok(binary.to_owned())
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{num::NonZeroU16, sync::Arc, time::Duration};

    use ctxfab_inference::supervisor::Runtime;
    use ctxfab_server::{
        adapters::outbound::artifact_storage::{DesktopArtifactBudget, DesktopArtifactReservation},
        adapters::outbound::managed_runtime::{
            self, ManagedRuntimePin,
            network::{self, RuntimeDownload},
        },
        application::artifact_storage::AdmissionError,
        application::runtime_setup::{Package, PackageDownload, PackageIdentity, STORAGE_LIMIT},
        runtime::outbound::{OutboundPool, QuotaPolicy},
    };

    use super::{Path, PathBuf, io};

    pub(in crate::composition) struct PinnedPackage {
        root: PathBuf,
        pin: ManagedRuntimePin,
        download: Option<Arc<RuntimeDownload>>,
        release_url: Option<&'static str>,
        budget: DesktopArtifactBudget,
    }

    impl PinnedPackage {
        pub(in crate::composition) fn for_release(
            directory: &Path,
            outbound: &OutboundPool,
            budget: DesktopArtifactBudget,
            transfer_slot: Arc<tokio::sync::Semaphore>,
        ) -> io::Result<Option<Self>> {
            let Some(pin) = release_pin()? else {
                return Ok(None);
            };
            let release_url = option_env!("CTXFAB_RUNTIME_RELEASE_URL");
            let download = if release_url.is_some() {
                let gate = outbound
                    .gate(
                        network::BUCKET,
                        QuotaPolicy {
                            concurrency: NonZeroU16::MIN,
                            spacing: Duration::from_secs(1),
                        },
                    )
                    .map_err(|_| io::Error::other("runtime quota unavailable"))?;
                Some(Arc::new(
                    RuntimeDownload::new(gate)
                        .map_err(|_| io::Error::other("runtime transport unavailable"))?
                        .with_transfer_slot(transfer_slot),
                ))
            } else {
                None
            };
            Ok(Some(Self {
                root: directory.join("managed-runtime"),
                pin,
                download,
                release_url,
                budget,
            }))
        }

        pub(in crate::composition) fn load(&self, runtime: &Runtime) {
            match managed_runtime::reopen(&self.root, &self.pin) {
                Ok(Some(binary)) => {
                    if runtime.publish(binary).is_err() {
                        warn_reopen();
                    }
                }
                Err(_) => warn_reopen(),
                Ok(None) => {}
            }
        }
    }

    fn reserve(
        budget: &DesktopArtifactBudget,
        pin: &ManagedRuntimePin,
        archive_copies: u64,
    ) -> Result<DesktopArtifactReservation, String> {
        let required = peak_bytes(pin, archive_copies);
        budget.try_reserve(required).map_err(|error| match error {
            AdmissionError::Unavailable {
                required,
                available,
            } => {
                tracing::warn!(
                    code = STORAGE_LIMIT,
                    required,
                    available,
                    "Runtime storage reservation refused"
                );
                STORAGE_LIMIT.into()
            }
            _ => "runtime staging unavailable".into(),
        })
    }

    fn peak_bytes(pin: &ManagedRuntimePin, archive_copies: u64) -> u64 {
        pin.archive_bytes * archive_copies + pin.archive_expanded_bytes
    }

    impl Package for PinnedPackage {
        fn identity(&self) -> Option<PackageIdentity> {
            Some(PackageIdentity {
                archive_sha256: self.pin.archive_sha256.clone(),
                archive_bytes: self.pin.archive_bytes,
                expanded_bytes: self.pin.archive_expanded_bytes,
                bundle_bytes: self.pin.bundle_bytes,
                bundle_manifest_sha256: self.pin.bundle_manifest_sha256.clone(),
                apple_team_id: self.pin.apple_team_id.clone(),
                hosted_release: self.release_url.map(str::to_owned),
            })
        }

        fn archive_bytes(&self) -> u64 {
            self.pin.archive_bytes
        }

        fn storage_peaks(&self) -> Option<(u64, Option<u64>)> {
            Some((
                peak_bytes(&self.pin, 1),
                self.can_download().then(|| peak_bytes(&self.pin, 2)),
            ))
        }

        fn install(&self, archive: &Path) -> Result<PathBuf, String> {
            let _reservation = reserve(&self.budget, &self.pin, 1)?;
            managed_runtime::verify_and_stage(archive, &self.root, &self.pin)
        }
        fn can_download(&self) -> bool {
            self.download.is_some()
        }

        fn download(&self) -> PackageDownload<'_> {
            Box::pin(async move {
                let downloader = self
                    .download
                    .as_ref()
                    .ok_or("runtime download unavailable")?;
                let url = self.release_url.ok_or("runtime download unavailable")?;
                let root = self.root.clone();
                let pin = self.pin.clone();
                let fetch_pin = pin.clone();
                let budget = self.budget.clone();
                let reservation =
                    tokio::task::spawn_blocking(move || reserve(&budget, &fetch_pin, 2))
                        .await
                        .map_err(|_| "runtime staging unavailable")??;
                let stage_root = root.clone();
                tokio::task::spawn_blocking(move || {
                    use std::os::unix::fs::DirBuilderExt;
                    match std::fs::DirBuilder::new().mode(0o700).create(&stage_root) {
                        Ok(()) => Ok(()),
                        Err(error)
                            if error.kind() == io::ErrorKind::AlreadyExists
                                && std::fs::symlink_metadata(&stage_root)
                                    .is_ok_and(|meta| meta.is_dir()) =>
                        {
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                })
                .await
                .map_err(|_| "runtime staging unavailable")?
                .map_err(|_| "runtime staging unavailable")?;
                let archive = downloader
                    .download(url, pin.archive_bytes, &root)
                    .await
                    .map_err(|_| "runtime download failed")?;
                tokio::task::spawn_blocking(move || {
                    let _reservation = reservation;
                    managed_runtime::verify_and_stage(archive.path(), &root, &pin)
                })
                .await
                .map_err(|_| "runtime verification unavailable".to_owned())?
            })
        }
    }

    fn warn_reopen() {
        tracing::warn!(
            code = "runtime_verification_failed",
            "The installed runtime could not be verified"
        );
    }

    pub(super) fn release_pin() -> io::Result<Option<ManagedRuntimePin>> {
        let config = (
            option_env!("CTXFAB_RUNTIME_ARCHIVE_BYTES"),
            option_env!("CTXFAB_RUNTIME_ARCHIVE_SHA256"),
            option_env!("CTXFAB_RUNTIME_APPLE_TEAM_ID"),
        );
        let expanded = option_env!("CTXFAB_RUNTIME_ARCHIVE_EXPANDED_BYTES");
        let bundle = option_env!("CTXFAB_RUNTIME_BUNDLE_BYTES");
        let manifest = option_env!("CTXFAB_RUNTIME_BUNDLE_MANIFEST_SHA256");
        let invalid = || io::Error::other("invalid managed runtime release configuration");
        let url = option_env!("CTXFAB_RUNTIME_RELEASE_URL");
        if let Some(url) = url {
            network::validate_release_url(url).map_err(|_| invalid())?;
        }
        if let (None, None, None) = config {
            return if url.is_some() || expanded.is_some() || bundle.is_some() || manifest.is_some()
            {
                Err(invalid())
            } else {
                Ok(None)
            };
        }
        let (Some(bytes), Some(sha), Some(team)) = config else {
            return Err(invalid());
        };
        let archive_bytes = bytes
            .parse::<u64>()
            .ok()
            .filter(|bytes| (1..=512 * 1024 * 1024).contains(bytes))
            .ok_or_else(invalid)?;
        let valid_sha = sha.len() == 64
            && sha
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'));
        let valid_team = team.len() == 10
            && team
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit());
        let archive_expanded_bytes = expanded
            .and_then(|bytes| bytes.parse::<u64>().ok())
            .filter(|bytes| (1..=4 * 1024 * 1024 * 1024).contains(bytes))
            .ok_or_else(invalid)?;
        let bundle_bytes = bundle
            .and_then(|bytes| bytes.parse::<u64>().ok())
            .filter(|bytes| (1..=archive_expanded_bytes).contains(bytes))
            .ok_or_else(invalid)?;
        let bundle_manifest_sha256 = manifest
            .filter(|sha| {
                sha.len() == 64
                    && sha
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            })
            .ok_or_else(invalid)?;
        if !valid_sha || !valid_team {
            return Err(invalid());
        }
        Ok(Some(ManagedRuntimePin {
            archive_bytes,
            archive_expanded_bytes,
            bundle_bytes,
            bundle_manifest_sha256: bundle_manifest_sha256.to_owned(),
            archive_sha256: sha.to_owned(),
            apple_team_id: team.to_owned(),
        }))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use ctxfab_server::{
            adapters::outbound::{
                artifact_storage::DesktopArtifactCapacityProbe, sqlite::SqliteStore,
            },
            application::sync::TrafficLedger,
            runtime::clock::Clock,
        };

        #[tokio::test]
        async fn hosted_admission_reserves_one_extra_archive_copy()
        -> Result<(), Box<dyn std::error::Error>> {
            let directory = tempfile::tempdir()?;
            let store = Arc::new(SqliteStore::open(
                &directory.path().join("context.sqlite3"),
            )?);
            let gate = OutboundPool::new(store.clone(), Clock::system()).gate(
                network::BUCKET,
                QuotaPolicy {
                    concurrency: NonZeroU16::MIN,
                    spacing: Duration::ZERO,
                },
            )?;
            let package = PinnedPackage {
                root: directory.path().join("managed-runtime"),
                pin: ManagedRuntimePin {
                    archive_bytes: 60,
                    archive_expanded_bytes: 100,
                    bundle_bytes: 90,
                    archive_sha256: "0".repeat(64),
                    bundle_manifest_sha256: "1".repeat(64),
                    apple_team_id: "AAAAAAAAAA".into(),
                },
                budget: DesktopArtifactBudget::new(DesktopArtifactCapacityProbe::new(
                    directory.path().to_owned(),
                    199,
                    0,
                    64,
                )),
                download: Some(Arc::new(RuntimeDownload::new(gate)?)),
                release_url: Some("invalid-fixture-url"),
            };
            assert!(reserve(&package.budget, &package.pin, 1).is_ok());
            assert_eq!(package.download().await, Err(STORAGE_LIMIT.into()));
            assert_eq!(store.traffic(network::BUCKET)?.attempts, 0);
            assert!(!package.root.exists());
            Ok(())
        }
    }
}
