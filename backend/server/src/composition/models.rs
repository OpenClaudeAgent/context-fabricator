use std::num::NonZeroU16;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ctxfab_inference::generation::Model;
use ctxfab_server::adapters::outbound::{
    hugging_face::{BUCKET, HuggingFace},
    sqlite::SqliteStore,
};
use ctxfab_server::application::artifact_storage::StorageReport;
use ctxfab_server::application::inference_setup::InferenceSetup;
use ctxfab_server::application::model_management::{
    ModelInventory, ModelManagement, RetainedModel,
};
use ctxfab_server::application::runtime_setup::RuntimeSetup;
use ctxfab_server::domain::model::DownloadError;
use ctxfab_server::runtime::{
    inference::Inference,
    outbound::{OutboundPool, QuotaPolicy},
};
use ctxfab_server::{
    adapters::outbound::artifact_storage::{DesktopArtifactBudget, DesktopArtifactCapacityProbe},
    application::{artifact_storage::ArtifactStorageBudget, ports::Repository},
};

use super::runtime_setup::DesktopRuntime;

pub(crate) struct Models {
    pub inference: Arc<Inference>,
    pub downloads: Arc<ModelManagement>,
    pub runtime_setup: Option<Arc<RuntimeSetup>>,
    pub artifact_storage: Option<Arc<dyn StorageReport>>,
    pub inference_setup: Option<Arc<InferenceSetup>>,
}

pub(crate) fn configure(
    support: &Path,
    outbound: &OutboundPool,
    store: &Arc<SqliteStore>,
) -> Result<Models, DownloadError> {
    let transfer_slot = Arc::new(tokio::sync::Semaphore::new(1));
    let budget = storage_budget(support, store.clone());
    let source = DesktopRuntime::resolve_bundled()?;
    #[cfg(target_os = "macos")]
    let source = match source {
        Some(source) => Some(source),
        None => DesktopRuntime::resolve_archive(
            support,
            outbound,
            budget.clone(),
            transfer_slot.clone(),
        )?,
    };
    let managed = source.is_some();
    let directory = if managed {
        support.join("inference")
    } else {
        Inference::asset_directory(support)
    };
    let gate = model_gate(outbound)?;
    let retained = store.load_models()?;
    let imported = retained_profiles(&directory, &retained);
    let (inference, runtime_setup) = if let Some(source) = source {
        let receipts = store.clone();
        let runtime = Arc::new(ctxfab_inference::supervisor::Runtime::with_weight_policy(
            move |path, profile| receipts.accepts_verified_weight(path, profile, None),
        ));
        source.load(&runtime)?;
        let inference = Arc::new(Inference::managed(
            &directory,
            store,
            imported,
            runtime.clone(),
        ));
        #[cfg(target_os = "macos")]
        let activated = inference.clone();
        let setup = source.into_setup(
            runtime,
            #[cfg(target_os = "macos")]
            move || activated.runtime_published(),
        );
        (inference, Some(Arc::new(setup)))
    } else {
        (
            Arc::new(Inference::configured_with_models(
                &directory, store, imported,
            )),
            None,
        )
    };
    let (catalog, adapter) = model_adapter(directory, gate)?;
    let adapter = if managed {
        adapter.with_managed_weights(store.clone(), budget.clone())
    } else {
        adapter
    };
    let adapter = Arc::new(adapter.with_transfer_slot(transfer_slot));
    let downloads = ModelManagement::new(catalog, adapter.clone())
        .with_usage(inference.clone())
        .with_discovery(adapter, store.clone(), retained)?
        .with_jobs(store.clone())?;
    let artifact_storage = managed.then(|| Arc::new(budget) as Arc<dyn StorageReport>);
    let downloads = Arc::new(downloads);
    let inference_setup = managed_setup(
        artifact_storage.as_ref(),
        runtime_setup.as_ref(),
        &inference,
        &downloads,
        store,
    )?;
    Ok(Models {
        inference,
        downloads,
        runtime_setup,
        artifact_storage,
        inference_setup,
    })
}

impl Models {
    pub async fn stop(self) {
        if let Some(setup) = self.inference_setup {
            setup.stop().await;
        }
        if let Some(runtime) = self.runtime_setup {
            runtime.stop();
        }
        self.downloads.stop().await;
        self.inference.shutdown();
    }
}

fn managed_setup(
    capacity: Option<&Arc<dyn StorageReport>>,
    runtime: Option<&Arc<RuntimeSetup>>,
    inference: &Inference,
    downloads: &Arc<ModelManagement>,
    store: &Arc<SqliteStore>,
) -> Result<Option<Arc<InferenceSetup>>, DownloadError> {
    let (Some(capacity), Some(runtime), Some(answers), Some(retrieval)) =
        (capacity, runtime, &inference.answers, &inference.retrieval)
    else {
        return Ok(None);
    };
    InferenceSetup::open(
        capacity.clone(),
        runtime.clone(),
        downloads.clone(),
        answers.clone(),
        retrieval.clone(),
        store.clone(),
    )
    .map(Arc::new)
    .map(Some)
    .map_err(|_| DownloadError::Unavailable)
}

fn model_adapter(
    directory: std::path::PathBuf,
    gate: Arc<ctxfab_server::runtime::outbound::OutboundGate>,
) -> Result<(Vec<ctxfab_server::domain::model::Artifact>, HuggingFace), DownloadError> {
    #[cfg(debug_assertions)]
    let fixture = std::env::var("CTXFAB_TEST_HUGGING_FACE_PORT")
        .ok()
        .and_then(|port| port.parse::<u16>().ok());
    #[cfg(not(debug_assertions))]
    let fixture = None;
    if let Some(port) = fixture {
        HuggingFace::discovery_fixture(
            directory,
            gate,
            std::net::SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, port),
        )
    } else {
        HuggingFace::production(directory, gate)
    }
}

fn model_gate(
    outbound: &OutboundPool,
) -> Result<Arc<ctxfab_server::runtime::outbound::OutboundGate>, DownloadError> {
    outbound
        .gate(
            BUCKET,
            QuotaPolicy {
                concurrency: NonZeroU16::MIN,
                spacing: Duration::from_secs(1),
            },
        )
        .map_err(|_| DownloadError::Unavailable)
}

fn retained_profiles(directory: &Path, retained: &[RetainedModel]) -> Vec<Model> {
    retained
        .iter()
        .filter(|model| model.verified)
        .filter_map(|model| {
            let artifact = &model.artifact;
            let metadata = directory
                .join(artifact.file.as_ref())
                .symlink_metadata()
                .ok()?;
            if !metadata.is_file() || metadata.len() != artifact.bytes {
                return None;
            }
            Model::imported(
                artifact.profile.to_string(),
                artifact.label.as_ref()?.to_string(),
                artifact.sha256.to_string(),
            )
        })
        .collect()
}

fn storage_budget(support: &Path, store: Arc<SqliteStore>) -> DesktopArtifactBudget {
    const SYSTEM_RESERVE: u64 = 256 * 1024 * 1024;
    ArtifactStorageBudget::new(
        DesktopArtifactCapacityProbe::new(support.to_owned(), 0, SYSTEM_RESERVE, 64)
            .with_live_limit(move || {
                store
                    .settings()
                    .map(|settings| settings.inference_artifact_limit_bytes())
                    .map_err(|_| std::io::Error::other("inference storage settings unavailable"))
            }),
    )
}
