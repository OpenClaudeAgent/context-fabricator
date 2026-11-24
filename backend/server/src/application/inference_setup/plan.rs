//! Read-only, release-pinned first-run inference setup planning.

use super::{SetupAsset, SetupIntent};
use crate::application::{
    artifact_storage::StorageCapacity,
    model_management::ModelManagement,
    runtime_setup::{RuntimeIdentity, RuntimePhase, RuntimeSetup},
};
use crate::domain::model::{Snapshot, State, staged_weight_peak};
use ctxfab_inference::{PROFILE as EMBEDDING_PROFILE, generation::GENERATION_PROFILE};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeSource {
    Hosted,
    LocalArchive,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetupSelection {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answer_profile: Option<String>,
    pub embedding: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_profile: Option<String>,
    pub runtime_source: RuntimeSource,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupPlan {
    pub selection: SetupSelection,
    pub fingerprint: String,
    pub peak_additional_bytes: u64,
    pub maximum_transfer_bytes: u64,
    pub available_bytes: u64,
    #[serde(skip)]
    pub(super) intent: SetupIntent,
    pub can_start: bool,
    pub runtime_required: bool,
    pub answer_installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_installed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocker: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SetupPhase {
    Runtime,
    WaitingForArchive,
    AnswerWeights,
    ActivatingAnswer,
    EmbeddingWeights,
    Completed,
    Failed,
    Interrupted,
}

impl SetupPhase {
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::Runtime
                | Self::WaitingForArchive
                | Self::AnswerWeights
                | Self::ActivatingAnswer
                | Self::EmbeddingWeights
        )
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatus {
    pub operation_id: uuid::Uuid,
    pub phase: SetupPhase,
    pub answer_ready: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<SetupSelection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("Choose supported answer and embedding models from the local catalog.")]
    InvalidSelection,
    #[error("Inference setup planning is unavailable.")]
    Unavailable,
}

/// Calculate a read-only setup plan from current local snapshots and release pins.
/// # Errors
/// Rejects unsupported choices and unavailable local state or runtime pins.
pub async fn plan(
    capacity: StorageCapacity,
    runtime: &RuntimeSetup,
    downloads: &ModelManagement,
    mut selection: SetupSelection,
) -> Result<SetupPlan, PlanError> {
    let profile = selection
        .answer_profile
        .as_deref()
        .unwrap_or(GENERATION_PROFILE)
        .to_owned();
    selection.answer_profile = Some(profile.clone());
    let answer_artifact = downloads
        .setup_answer(&profile)
        .map_err(|error| match error {
            crate::domain::model::DownloadError::UnknownProfile => PlanError::InvalidSelection,
            _ => PlanError::Unavailable,
        })?;
    let embedding_artifact = match (selection.embedding, selection.embedding_profile.as_deref()) {
        (true, profile) => Some(
            downloads
                .setup_embedding(profile.unwrap_or(EMBEDDING_PROFILE))
                .map_err(|error| match error {
                    crate::domain::model::DownloadError::UnknownProfile => {
                        PlanError::InvalidSelection
                    }
                    _ => PlanError::Unavailable,
                })?,
        ),
        (false, None) => None,
        _ => return Err(PlanError::InvalidSelection),
    };
    selection.embedding_profile = embedding_artifact
        .as_ref()
        .map(|artifact| artifact.profile.to_string());
    let status = runtime.status();
    let runtime_required = !matches!(status.state, RuntimePhase::Ready);
    let identity = runtime.identity().ok_or(PlanError::Unavailable)?;
    let snapshots = downloads
        .snapshot()
        .await
        .map_err(|_| PlanError::Unavailable)?;
    let answer = snapshots
        .iter()
        .find(|model| model.profile == profile)
        .ok_or(PlanError::Unavailable)?;
    let embedding = embedding_artifact
        .as_ref()
        .map(|artifact| {
            snapshots
                .iter()
                .find(|model| model.profile == artifact.profile)
                .ok_or(PlanError::Unavailable)
        })
        .transpose()?;
    let Estimates {
        peak,
        transfer,
        answer_cost,
        embedding_cost,
    } = estimate(
        &identity,
        selection.runtime_source,
        runtime_required,
        answer,
        embedding,
    );
    let blocker = find_blocker(
        &status,
        &selection,
        runtime_required,
        answer,
        embedding,
        peak,
        capacity.available_bytes,
    );
    let embedding_pin = embedding_artifact
        .as_ref()
        .map_or("", |artifact| artifact.sha256.as_ref());
    let intent = SetupIntent {
        version: SetupIntent::VERSION,
        selection: selection.clone(),
        runtime: identity,
        runtime_required,
        answer: SetupAsset::from_plan(answer, &answer_artifact.sha256, answer_cost.0),
        embedding: embedding
            .map(|model| SetupAsset::from_plan(model, embedding_pin, embedding_cost.0)),
    };
    let fingerprint = plan_fingerprint(&intent, peak, transfer)?;
    Ok(SetupPlan {
        selection,
        fingerprint,
        peak_additional_bytes: peak,
        maximum_transfer_bytes: transfer,
        available_bytes: capacity.available_bytes,
        intent,
        can_start: blocker.is_none(),
        runtime_required,
        answer_installed: answer.state == State::Installed,
        embedding_installed: embedding.map(|model| model.state == State::Installed),
        blocker: blocker.map(str::to_owned),
    })
}

struct Estimates {
    peak: u64,
    transfer: u64,
    answer_cost: (u64, u64),
    embedding_cost: (u64, u64),
}

fn estimate(
    identity: &RuntimeIdentity,
    source: RuntimeSource,
    runtime_required: bool,
    answer: &Snapshot,
    embedding: Option<&Snapshot>,
) -> Estimates {
    let (runtime_peak, runtime_retained, runtime_transfer) = if let RuntimeIdentity::Archive(pin) =
        identity
        && runtime_required
    {
        let copies = if source == RuntimeSource::Hosted {
            2
        } else {
            1
        };
        (
            pin.archive_bytes
                .saturating_mul(copies)
                .saturating_add(pin.expanded_bytes),
            pin.archive_bytes.saturating_add(pin.bundle_bytes),
            if source == RuntimeSource::Hosted {
                pin.archive_bytes
            } else {
                0
            },
        )
    } else {
        (0, 0, 0)
    };
    let answer_cost = weight_cost(answer);
    let embedding_cost = embedding.map_or((0, 0), weight_cost);
    let peak = sequential_peak(&[
        (runtime_peak, runtime_retained),
        answer_cost,
        embedding_cost,
    ]);
    let transfer = runtime_transfer
        .saturating_add(if answer.state == State::Installed {
            0
        } else {
            answer.total_bytes
        })
        .saturating_add(
            embedding
                .filter(|model| model.state != State::Installed)
                .map_or(0, |model| model.total_bytes),
        );
    Estimates {
        peak,
        transfer,
        answer_cost,
        embedding_cost,
    }
}

fn find_blocker(
    status: &crate::application::runtime_setup::RuntimeStatus,
    selection: &SetupSelection,
    runtime_required: bool,
    answer: &Snapshot,
    embedding: Option<&Snapshot>,
    peak: u64,
    available: u64,
) -> Option<&'static str> {
    let blocked_job = std::iter::once(answer).chain(embedding).any(|model| {
        matches!(
            model.state,
            State::Installing | State::Pausing | State::Paused
        )
    });
    if matches!(status.state, RuntimePhase::Unavailable) {
        Some("runtime_unavailable")
    } else if matches!(status.state, RuntimePhase::Installing) {
        Some("runtime_busy")
    } else if blocked_job {
        Some("model_job_busy")
    } else if runtime_required
        && selection.runtime_source == RuntimeSource::Hosted
        && status.can_download != Some(true)
    {
        Some("runtime_download_unavailable")
    } else if peak > available {
        Some("storage_limit")
    } else {
        None
    }
}

fn weight_cost(model: &Snapshot) -> (u64, u64) {
    if model.state == State::Installed {
        (0, 0)
    } else {
        let remaining = staged_weight_peak(model.total_bytes, model.downloaded_bytes);
        (remaining, remaining)
    }
}

fn sequential_peak(steps: &[(u64, u64)]) -> u64 {
    let mut retained = 0_u64;
    let mut peak = 0_u64;
    for &(step_peak, step_retained) in steps {
        peak = peak.max(retained.saturating_add(step_peak));
        retained = retained.saturating_add(step_retained);
    }
    peak
}

fn fingerprint(
    selection: &SetupSelection,
    runtime: &str,
    answer: &str,
    embedding: &str,
    peak: u64,
    transfer: u64,
) -> Result<String, PlanError> {
    let data = serde_json::to_vec(&(selection, runtime, answer, embedding, peak, transfer))
        .map_err(|_| PlanError::Unavailable)?;
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(data) {
        write!(&mut hex, "{byte:02x}").map_err(|_| PlanError::Unavailable)?;
    }
    Ok(hex)
}

fn plan_fingerprint(intent: &SetupIntent, peak: u64, transfer: u64) -> Result<String, PlanError> {
    let pin = serde_json::to_string(intent).map_err(|_| PlanError::Unavailable)?;
    fingerprint(
        &intent.selection,
        &pin,
        &intent.answer.sha256,
        intent
            .embedding
            .as_ref()
            .map_or("", |model| model.sha256.as_str()),
        peak,
        transfer,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequential_peak_keeps_prior_retained_bytes() {
        assert_eq!(sequential_peak(&[(100, 70), (80, 50), (90, 20)]), 210);
    }

    #[test]
    fn fingerprint_ignores_capacity_but_changes_with_the_pin() -> Result<(), PlanError> {
        let selection = SetupSelection {
            answer_profile: None,
            embedding: true,
            embedding_profile: None,
            runtime_source: RuntimeSource::Hosted,
        };
        let first = fingerprint(&selection, "runtime-a", "answer", "embedding", 123, 456)?;
        assert_eq!(
            first,
            fingerprint(&selection, "runtime-a", "answer", "embedding", 123, 456)?
        );
        assert_ne!(
            first,
            fingerprint(&selection, "runtime-b", "answer", "embedding", 123, 456)?
        );
        Ok(())
    }
    use crate::application::model_management::{ArtifactResult, ArtifactStore, LocalArtifactState};
    use crate::application::runtime_setup::{Package, PackageIdentity};
    use crate::domain::model::{Artifact, DownloadError};
    use std::{
        path::{Path, PathBuf},
        sync::Arc,
    };
    struct FakePackage;
    impl Package for FakePackage {
        fn archive_bytes(&self) -> u64 {
            100
        }
        fn identity(&self) -> Option<PackageIdentity> {
            Some(PackageIdentity {
                archive_sha256: "archive".into(),
                archive_bytes: 100,
                expanded_bytes: 150,
                bundle_bytes: 120,
                bundle_manifest_sha256: "bundle".into(),
                apple_team_id: "team".into(),
                hosted_release: Some("fixture-release".into()),
            })
        }
        fn can_download(&self) -> bool {
            true
        }
        fn install(&self, _: &Path) -> Result<PathBuf, String> {
            Err("unexpected install".into())
        }
    }
    struct FakeArtifacts;
    impl ArtifactStore for FakeArtifacts {
        fn install<'a>(&'a self, _: &'a Artifact) -> ArtifactResult<'a, ()> {
            Box::pin(async { Err(DownloadError::Unavailable) })
        }
        fn install_controlled<'a>(
            &'a self,
            _: &'a Artifact,
            _: tokio::sync::watch::Receiver<bool>,
        ) -> ArtifactResult<'a, ()> {
            Box::pin(async { Err(DownloadError::Unavailable) })
        }
        fn local_state<'a>(&'a self, _: &'a Artifact) -> ArtifactResult<'a, LocalArtifactState> {
            Box::pin(async {
                Ok(LocalArtifactState {
                    installed: false,
                    staged_bytes: 10,
                })
            })
        }
        fn remove(&self, _: &Artifact) -> Result<(), DownloadError> {
            Err(DownloadError::Unavailable)
        }
        fn discard_partial(&self, _: &Artifact) -> Result<(), DownloadError> {
            Err(DownloadError::Unavailable)
        }
    }
    fn artifact(profile: &'static str, bytes: u64) -> Artifact {
        Artifact {
            profile: profile.into(),
            label: None,
            kind: None,
            source: None,
            file: "fake".into(),
            remote_file: None,
            bytes,
            sha256: if profile == GENERATION_PROFILE {
                ctxfab_inference::generation::GENERATION_SHA256
            } else {
                "embedding-pin"
            }
            .into(),
        }
    }

    #[tokio::test]
    async fn preinstalled_engine_sets_up_only_verified_weights()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = Arc::new(ctxfab_inference::supervisor::Runtime::default());
        runtime
            .publish(std::env::current_exe()?)
            .map_err(|_| "fixture executable was unavailable")?;
        let setup = RuntimeSetup::bundled(runtime.clone(), "bundled-runtime-a".into());
        let downloads = ModelManagement::new(
            vec![
                artifact(GENERATION_PROFILE, 200),
                artifact(EMBEDDING_PROFILE, 50),
            ],
            Arc::new(FakeArtifacts),
        );
        let capacity = StorageCapacity {
            limit_bytes: 1000,
            used_bytes: 0,
            reserved_bytes: 0,
            available_bytes: 1000,
            volume_free_bytes: 1000,
            system_reserve_bytes: 0,
        };
        let selection = SetupSelection {
            answer_profile: None,
            embedding: false,
            embedding_profile: None,
            runtime_source: RuntimeSource::Hosted,
        };
        let answer_only = plan(capacity, &setup, &downloads, selection.clone()).await?;
        assert!(answer_only.can_start && !answer_only.runtime_required);
        assert_eq!(answer_only.maximum_transfer_bytes, 200);
        assert_eq!(answer_only.peak_additional_bytes, 190);
        assert!(setup.status().archive_bytes.is_none() && setup.storage_peaks().is_none());
        let changed = RuntimeSetup::bundled(runtime, "bundled-runtime-b".into());
        let changed_plan = plan(capacity, &changed, &downloads, selection).await?;
        assert_ne!(answer_only.fingerprint, changed_plan.fingerprint);
        Ok(())
    }

    #[tokio::test]
    async fn plan_is_read_only_and_budget_change_does_not_change_fingerprint()
    -> Result<(), PlanError> {
        let downloads = ModelManagement::new(
            vec![
                artifact(GENERATION_PROFILE, 200),
                artifact(EMBEDDING_PROFILE, 50),
            ],
            Arc::new(FakeArtifacts),
        );
        let runtime = RuntimeSetup::new(
            Box::new(FakePackage),
            Arc::new(ctxfab_inference::supervisor::Runtime::default()),
            || {},
        );
        let capacity = StorageCapacity {
            limit_bytes: 1000,
            used_bytes: 0,
            reserved_bytes: 0,
            available_bytes: 1000,
            volume_free_bytes: 1000,
            system_reserve_bytes: 0,
        };
        let selection = SetupSelection {
            answer_profile: None,
            embedding: true,
            embedding_profile: None,
            runtime_source: RuntimeSource::Hosted,
        };
        let wrong = Artifact {
            sha256: "incorrect".into(),
            ..artifact(GENERATION_PROFILE, 200)
        };
        let bad_catalog = ModelManagement::new(
            vec![wrong, artifact(EMBEDDING_PROFILE, 50)],
            Arc::new(FakeArtifacts),
        );
        assert!(matches!(
            plan(capacity, &runtime, &bad_catalog, selection.clone()).await,
            Err(PlanError::Unavailable)
        ));
        let initial = plan(capacity, &runtime, &downloads, selection.clone()).await?;
        assert_eq!(
            initial.selection.answer_profile.as_deref(),
            Some(GENERATION_PROFILE)
        );
        assert_eq!(initial.peak_additional_bytes, 450);
        assert_eq!(initial.maximum_transfer_bytes, 350);
        assert!(initial.can_start);
        let lower = plan(
            StorageCapacity {
                available_bytes: 100,
                ..capacity
            },
            &runtime,
            &downloads,
            selection,
        )
        .await?;
        assert!(!lower.can_start);
        assert_eq!(lower.blocker.as_deref(), Some("storage_limit"));
        assert_eq!(initial.fingerprint, lower.fingerprint);
        Ok(())
    }
    #[test]
    fn explicit_pause_blocks_but_interrupted_is_retryable() {
        use crate::application::runtime_setup::RuntimeStatus;
        let status = RuntimeStatus {
            state: RuntimePhase::AvailableForInstall,
            engine: None,
            archive_bytes: Some(100),
            can_download: Some(true),
            error: None,
        };
        let selection = SetupSelection {
            answer_profile: None,
            embedding: false,
            embedding_profile: None,
            runtime_source: RuntimeSource::Hosted,
        };
        let mut model = Snapshot {
            profile: GENERATION_PROFILE.into(),
            label: None,
            kind: None,
            source: None,
            setup_role: None,
            deletion_block: None,
            state: State::Paused,
            installation_stage: None,
            operation_id: None,
            downloaded_bytes: 0,
            total_bytes: 100,
            error: None,
        };
        assert_eq!(
            find_blocker(&status, &selection, true, &model, None, 100, 1000),
            Some("model_job_busy")
        );
        model.state = State::Interrupted;
        assert_eq!(
            find_blocker(&status, &selection, true, &model, None, 100, 1000),
            None
        );
        let unavailable = RuntimeStatus {
            can_download: None,
            ..status
        };
        assert_eq!(
            find_blocker(&unavailable, &selection, true, &model, None, 100, 1000),
            Some("runtime_download_unavailable")
        );
    }
}
