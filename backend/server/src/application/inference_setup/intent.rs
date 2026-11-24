use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{SetupSelection, SetupStatus};
use crate::{application::runtime_setup::RuntimeIdentity, domain::model::Snapshot};

/// Identifies the active runner for one permanently confirmed setup operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SetupLease {
    pub id: Uuid,
    pub revision: i64,
}

/// Immutable download permission. Local input paths are deliberately session-only.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupIntent {
    pub version: u8,
    pub selection: SetupSelection,
    pub runtime: RuntimeIdentity,
    pub runtime_required: bool,
    pub answer: SetupAsset,
    pub embedding: Option<SetupAsset>,
}

impl SetupIntent {
    pub(crate) const VERSION: u8 = 1;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupAsset {
    pub profile: String,
    pub sha256: String,
    pub bytes: u64,
    pub was_installed: bool,
    pub additional_bytes: u64,
    pub previous_job: Option<String>,
}

impl SetupAsset {
    pub(super) fn from_plan(snapshot: &Snapshot, sha256: &str, additional_bytes: u64) -> Self {
        Self {
            profile: snapshot.profile.to_string(),
            sha256: sha256.to_owned(),
            bytes: snapshot.total_bytes,
            was_installed: snapshot.state == crate::domain::model::State::Installed,
            additional_bytes,
            previous_job: snapshot.operation_id.clone(),
        }
    }
}

pub struct SetupRun {
    pub lease: SetupLease,
    pub status: SetupStatus,
    pub intent: SetupIntent,
}
