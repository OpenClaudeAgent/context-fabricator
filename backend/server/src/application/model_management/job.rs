//! Download lifecycle and its durable representation.

use tokio::{sync::watch, task::JoinHandle};

use super::{Artifact, DownloadError, InstallationStage, State, StoredDownload};

pub(super) struct Job {
    pub(super) artifact: Artifact,
    pub(super) verified: bool,
    pub(super) selecting: bool,
    pub(super) phase: Phase,
    pub(super) operation_id: Option<String>,
}

impl Job {
    pub(super) fn new(artifact: Artifact, verified: bool) -> Self {
        Self {
            artifact,
            verified,
            selecting: false,
            phase: Phase::Idle,
            operation_id: None,
        }
    }

    pub(super) fn clear(&mut self) {
        self.phase = Phase::Idle;
        self.operation_id = None;
    }

    /// Fence completion before releasing the lock and aborting the transfer.
    pub(super) fn interrupt(&mut self) -> Option<(JoinHandle<()>, Option<SavedPhase>)> {
        let phase = std::mem::replace(&mut self.phase, Phase::Idle);
        if let Phase::Active(transfer) = phase {
            if transfer.is_pausing() {
                self.phase = Phase::Paused;
                Some((transfer.task, None))
            } else {
                self.phase = Phase::Interrupted;
                Some((transfer.task, Some(SavedPhase::Interrupted)))
            }
        } else {
            self.phase = phase;
            None
        }
    }
}

pub(super) enum Phase {
    Idle,
    Active(Transfer),
    Paused,
    Interrupted,
    Failed(DownloadError),
}

impl Phase {
    pub(super) const fn can_discard(&self) -> bool {
        matches!(self, Self::Paused | Self::Interrupted)
    }

    pub(super) const fn can_resume(&self) -> bool {
        self.can_discard() || matches!(self, Self::Failed(_))
    }

    pub(super) fn is_paused(&self) -> bool {
        match self {
            Self::Paused => true,
            Self::Active(transfer) => transfer.is_pausing(),
            _ => false,
        }
    }

    pub(super) fn status(&self) -> State {
        match self {
            Self::Idle => State::NotInstalled,
            Self::Active(transfer) if transfer.is_pausing() => State::Pausing,
            Self::Active(_) => State::Installing,
            Self::Paused => State::Paused,
            Self::Interrupted => State::Interrupted,
            Self::Failed(_) => State::Failed,
        }
    }

    pub(super) fn installation_stage(&self) -> Option<InstallationStage> {
        match self {
            Self::Active(transfer) => Some(*transfer.stage.borrow()),
            _ => None,
        }
    }

    pub(super) const fn failure(&self) -> Option<DownloadError> {
        match self {
            Self::Failed(error) => Some(*error),
            _ => None,
        }
    }
}

pub(super) struct Transfer {
    pub(super) task: JoinHandle<()>,
    pub(super) pause: watch::Sender<bool>,
    pub(super) stage: watch::Receiver<InstallationStage>,
}

impl Transfer {
    pub(super) fn is_pausing(&self) -> bool {
        *self.pause.borrow()
    }
}

#[derive(Clone, Copy)]
pub(super) enum SavedPhase {
    Running,
    Paused,
    Interrupted,
    Failed(DownloadError),
}

impl SavedPhase {
    pub(super) fn record(self, artifact: &Artifact, operation_id: &str) -> StoredDownload {
        StoredDownload {
            profile: artifact.profile.to_string(),
            fingerprint: artifact.sha256.to_string(),
            operation_id: operation_id.to_owned(),
            phase: self.code().into(),
            error: self.error().map(str::to_owned),
        }
    }

    pub(super) fn parse(job: &StoredDownload) -> Option<Self> {
        match job.phase.as_str() {
            "running" => Some(Self::Running),
            "paused" => Some(Self::Paused),
            "interrupted" => Some(Self::Interrupted),
            "failed" => Some(Self::Failed(match job.error.as_deref() {
                Some("model_transfer_failed") => DownloadError::Transfer,
                Some("model_integrity_failed") => DownloadError::Integrity,
                Some("model_storage_limit") => DownloadError::StorageLimit,
                _ => DownloadError::Unavailable,
            })),
            _ => None,
        }
    }

    pub(super) const fn restore(self) -> Phase {
        match self {
            Self::Running | Self::Interrupted => Phase::Interrupted,
            Self::Paused => Phase::Paused,
            Self::Failed(error) => Phase::Failed(error),
        }
    }

    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Interrupted => "interrupted",
            Self::Failed(_) => "failed",
        }
    }

    const fn error(self) -> Option<&'static str> {
        match self {
            Self::Failed(error) => Some(error.code()),
            _ => None,
        }
    }
}
