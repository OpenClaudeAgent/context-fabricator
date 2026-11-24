use std::{path::PathBuf, time::Duration};

use ctxfab_inference::generation::GenerationError;

use super::{
    InferenceSetup, RuntimeSource, SetupAsset, SetupIntent, SetupLease, SetupPhase, SetupRun,
};
use crate::{
    application::runtime_setup::RuntimePhase,
    domain::model::{Snapshot, State, staged_weight_peak},
};

const POLL: Duration = Duration::from_millis(350);
const SPACE_POLL: Duration = Duration::from_secs(1);

type StageResult = Result<(), &'static str>;

impl InferenceSetup {
    pub(super) async fn execute(&self, run: SetupRun, archive: Option<PathBuf>) {
        let lease = run.lease;
        let mut closing = self.closing.subscribe();
        let result = tokio::select! {
            biased;
            _ = closing.wait_for(|stop| *stop) => return,
            result = self.run(run, archive) => result,
        };
        if let Err(error) = result
            && let Ok(Some(current)) = self.journal.current()
            && current.operation_id == lease.id
            && current.phase.is_active()
        {
            if self
                .journal
                .transition(lease, current.phase, SetupPhase::Failed, Some(error))
                .is_err()
            {
                tracing::warn!(
                    code = "inference_setup_checkpoint_failed",
                    "Could not record setup failure"
                );
            } else {
                tracing::warn!(
                    code = "inference_setup_failed",
                    "Confirmed inference setup did not finish"
                );
            }
        }
    }

    async fn run(&self, run: SetupRun, archive: Option<PathBuf>) -> StageResult {
        let SetupRun {
            lease,
            status,
            intent,
        } = run;
        if !self.approved_runtime(&intent) {
            return Err("The approved runtime changed. Review a new installation estimate.");
        }
        let mut phase = status.phase;
        if phase == SetupPhase::Runtime {
            self.check_selection(lease)?;
            self.prepare_runtime(lease, &intent, archive).await?;
            self.advance(lease, phase, SetupPhase::AnswerWeights)?;
            phase = SetupPhase::AnswerWeights;
        }
        if !matches!(self.runtime.status().state, RuntimePhase::Ready) {
            return Err(
                "The verified runtime is no longer available. Review a new installation estimate.",
            );
        }
        if phase == SetupPhase::AnswerWeights {
            self.check_selection(lease)?;
            self.install_model(lease, phase, &intent.answer).await?;
            self.advance(lease, phase, SetupPhase::ActivatingAnswer)?;
            phase = SetupPhase::ActivatingAnswer;
        }
        if phase == SetupPhase::ActivatingAnswer {
            self.check_selection(lease)?;
            self.check_asset(&intent.answer)?;
            let next = if intent.embedding.is_some() {
                SetupPhase::EmbeddingWeights
            } else {
                SetupPhase::Completed
            };
            self.activate(lease, &intent.answer.profile, next).await?;
            phase = next;
        }
        if phase == SetupPhase::EmbeddingWeights {
            let asset = intent
                .embedding
                .as_ref()
                .ok_or("The approved embedding model is unavailable.")?;
            self.finish_embedding(lease, asset).await?;
        }
        tracing::info!(
            code = "inference_setup_completed",
            "Confirmed inference setup completed"
        );
        Ok(())
    }

    fn check_selection(&self, lease: SetupLease) -> StageResult {
        if self
            .journal
            .selection_unchanged(lease)
            .is_ok_and(|same| same)
        {
            Ok(())
        } else {
            Err("The model selection changed or could not be read. Review your current selection.")
        }
    }

    fn check_embedding_policy(&self, lease: SetupLease) -> StageResult {
        let state = self
            .retrieval
            .index_state()
            .map_err(|_| "The embedding policy is unavailable.")?;
        if !self
            .journal
            .embedding_unchanged(lease, &state.profile)
            .map_err(|_| "The embedding policy is unavailable.")?
        {
            return Err(
                "The embedding policy changed during setup. Answers are ready. Review Embedding Index in Settings.",
            );
        }
        Ok(())
    }

    async fn finish_embedding(&self, lease: SetupLease, asset: &SetupAsset) -> StageResult {
        self.check_embedding_policy(lease)?;
        self.install_model(lease, SetupPhase::EmbeddingWeights, asset)
            .await?;
        let retrieval = self.retrieval.clone();
        let target = asset.profile.clone();
        let _prepared = tokio::task::spawn_blocking(move || retrieval.prepare_embedding(&target))
            .await
            .ok()
            .and_then(Result::ok)
            .ok_or("The selected embedding model failed its readiness check. Answers are ready.")?;
        if !self
            .retrieval
            .complete_setup_index(&asset.profile, |previous, profile| {
                self.journal
                    .commit_embedding(lease, previous, profile)
                    .map_err(|_| crate::application::retrieval::RetrievalError::Storage)
            })
            .map_err(|_| "The embedding policy is unavailable.")?
        {
            return Err(
                "The embedding policy changed during setup. Answers are ready. Review Embedding Index in Settings.",
            );
        }
        Ok(())
    }

    fn advance(&self, lease: SetupLease, from: SetupPhase, to: SetupPhase) -> StageResult {
        if self
            .journal
            .transition(lease, from, to, None)
            .is_ok_and(|changed| changed)
        {
            Ok(())
        } else {
            Err("The installation step is no longer active.")
        }
    }

    async fn wait_for_space(
        &self,
        lease: SetupLease,
        phase: SetupPhase,
        required: u64,
    ) -> StageResult {
        if required == 0 {
            return Ok(());
        }
        let mut waiting = false;
        loop {
            let available = self
                .capacity_now()
                .await
                .map_err(|_| "Could not read available space for setup.")?
                .available_bytes;
            if available >= required {
                return if waiting {
                    self.advance(lease, phase, phase)
                } else {
                    Ok(())
                };
            }
            if !waiting {
                let note =
                    "Waiting for disk space. Free space or raise the inference storage limit.";
                if !self
                    .journal
                    .transition(lease, phase, phase, Some(note))
                    .is_ok_and(|changed| changed)
                {
                    return Err("The installation step is no longer active.");
                }
                waiting = true;
            }
            tokio::time::sleep(SPACE_POLL).await;
        }
    }

    async fn prepare_runtime(
        &self,
        lease: SetupLease,
        intent: &SetupIntent,
        archive: Option<PathBuf>,
    ) -> StageResult {
        if matches!(self.runtime.status().state, RuntimePhase::Ready) {
            return Ok(());
        }
        if !intent.runtime_required {
            return Err(
                "The runtime changed after confirmation. Review a new installation estimate.",
            );
        }
        if !matches!(self.runtime.status().state, RuntimePhase::Installing) {
            let (local, hosted) = self
                .runtime
                .storage_peaks()
                .ok_or("The runtime installation estimate is unavailable.")?;
            let required = match intent.selection.runtime_source {
                RuntimeSource::Hosted => {
                    hosted.ok_or("The approved runtime download is unavailable.")?
                }
                RuntimeSource::LocalArchive => local,
            };
            self.wait_for_space(lease, SetupPhase::Runtime, required)
                .await?;
            self.check_selection(lease)?;
        }
        if matches!(self.runtime.status().state, RuntimePhase::Ready) {
            return Ok(());
        }
        if let Some(archive) = archive {
            let runtime = self.runtime.clone();
            let status = tokio::task::spawn_blocking(move || runtime.install(&archive))
                .await
                .map_err(|_| "The local runtime ZIP could not be opened.")?;
            if matches!(status.state, RuntimePhase::Failed) {
                return Err(status
                    .error
                    .unwrap_or("The local runtime ZIP could not be verified."));
            }
        } else if intent.selection.runtime_source == RuntimeSource::Hosted {
            let _ = self.runtime.request_download();
        } else if !matches!(self.runtime.status().state, RuntimePhase::Installing) {
            return Err("Choose the approved runtime ZIP to continue.");
        }
        loop {
            let status = self.runtime.status();
            match status.state {
                RuntimePhase::Ready => return Ok(()),
                RuntimePhase::Installing => tokio::time::sleep(POLL).await,
                _ => {
                    return Err(status
                        .error
                        .unwrap_or("The local runtime installation is unavailable."));
                }
            }
        }
    }

    fn check_asset(&self, approved: &SetupAsset) -> StageResult {
        let current = self
            .downloads
            .artifact(&approved.profile)
            .map_err(|_| "The approved model is unavailable.")?;
        if current.sha256 == approved.sha256 && current.bytes == approved.bytes {
            Ok(())
        } else {
            Err("The approved model changed. Review a new installation estimate.")
        }
    }

    async fn install_model(
        &self,
        lease: SetupLease,
        phase: SetupPhase,
        approved: &SetupAsset,
    ) -> StageResult {
        self.check_asset(approved)?;
        let profile = &approved.profile;
        let observed = self.model_snapshot(profile).await?;
        if observed.state == State::Installed {
            return Ok(());
        }
        if matches!(observed.state, State::Paused | State::Pausing) {
            return Err("The model download is paused. Resume it from Manage Models.");
        }
        let remaining = staged_weight_peak(observed.total_bytes, observed.downloaded_bytes);
        if approved.was_installed || remaining > approved.additional_bytes {
            return Err(
                "The model files changed after confirmation. Review a new installation estimate.",
            );
        }
        if observed.state != State::Installing {
            self.wait_for_space(lease, phase, remaining).await?;
        }
        if phase == SetupPhase::AnswerWeights {
            self.check_selection(lease)?;
        } else {
            self.check_embedding_policy(lease)?;
        }
        self.downloads
            .request_for_setup(
                profile,
                approved.additional_bytes,
                lease,
                approved.previous_job.as_deref(),
            )
            .map_err(|error| match error {
                crate::domain::model::DownloadError::Paused => {
                    "The model download is paused. Resume it from Manage Models."
                }
                crate::domain::model::DownloadError::InUse => {
                    "The model job was changed or removed. Review a new installation estimate."
                }
                _ => "The model download could not start.",
            })?;
        loop {
            let model = self.model_snapshot(profile).await?;
            match model.state {
                State::Installed => return Ok(()),
                State::Installing => tokio::time::sleep(POLL).await,
                State::Failed
                    if model.error
                        == Some(crate::domain::model::DownloadError::StorageLimit.code()) =>
                {
                    return Err(
                        "The model storage changed after confirmation. Review a new installation estimate.",
                    );
                }

                State::Paused | State::Pausing => {
                    return Err("The model download is paused. Resume it from Manage Models.");
                }
                _ => {
                    return Err(
                        "The model could not be verified and installed. Check Manage Models before retrying.",
                    );
                }
            }
        }
    }

    async fn model_snapshot(&self, profile: &str) -> Result<Snapshot, &'static str> {
        self.downloads
            .snapshot()
            .await
            .map_err(|_| "The model library is unavailable.")?
            .into_iter()
            .find(|model| model.profile == profile)
            .ok_or("The reviewed model is unavailable.")
    }

    async fn activate(&self, lease: SetupLease, profile: &str, next: SetupPhase) -> StageResult {
        loop {
            let answers = self.answers.clone();
            let journal = self.journal.clone();
            let profile = profile.to_owned();
            let result = tokio::task::spawn_blocking(move || {
                answers.select_setup_model(&profile, |profile, fingerprint| {
                    journal.commit_activation(lease, profile, fingerprint, next)
                })
            })
            .await
            .map_err(|_| "The answer model could not be activated.")?;
            match result {
                Ok(_) => return Ok(()),
                Err(GenerationError::Busy) => tokio::time::sleep(POLL).await,
                Err(GenerationError::Runtime) => {
                    return Err(
                        "The model selection changed or could not be saved. Review your current selection.",
                    );
                }
                Err(_) => {
                    return Err(
                        "The verified model could not start. Check Manage Models before retrying.",
                    );
                }
            }
        }
    }
}
