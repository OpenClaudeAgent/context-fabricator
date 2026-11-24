use std::borrow::Cow;

use serde::Serialize;

use super::StorageCapacity;
use crate::domain::model::{Snapshot, State, staged_weight_peak};

#[derive(Default, Serialize)]
pub struct StoragePlan {
    #[serde(skip_serializing_if = "Option::is_none")]
    capacity: Option<StorageCapacity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<RuntimePlan>,
    models: Vec<ModelPlan>,
}

impl StoragePlan {
    #[must_use]
    pub fn estimate(
        capacity: StorageCapacity,
        runtime_peaks: Option<(u64, Option<u64>)>,
        models: Vec<Snapshot>,
    ) -> Self {
        let available = capacity.available_bytes;
        let runtime = runtime_peaks.map(|(local, hosted)| RuntimePlan {
            local_import: InstallPlan::estimate(local, available),
            hosted_download: hosted.map(|peak| InstallPlan::estimate(peak, available)),
        });
        let models = models
            .into_iter()
            .map(|model| {
                let peak = if model.state == State::Installed {
                    0
                } else {
                    staged_weight_peak(model.total_bytes, model.downloaded_bytes)
                };
                ModelPlan {
                    profile: model.profile,
                    estimate: InstallPlan::estimate(peak, available),
                }
            })
            .collect();
        Self {
            capacity: Some(capacity),
            runtime,
            models,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimePlan {
    local_import: InstallPlan,
    #[serde(skip_serializing_if = "Option::is_none")]
    hosted_download: Option<InstallPlan>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InstallPlan {
    peak_bytes: u64,
    fits: bool,
}

impl InstallPlan {
    fn estimate(peak_bytes: u64, available: u64) -> Self {
        Self {
            peak_bytes,
            fits: peak_bytes <= available,
        }
    }
}

#[derive(Serialize)]
struct ModelPlan {
    profile: Cow<'static, str>,
    #[serde(flatten)]
    estimate: InstallPlan,
}
