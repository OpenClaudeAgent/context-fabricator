use std::{path::PathBuf, sync::Arc};

use uuid::Uuid;

use super::{
    InferenceSetup, JournalError, RuntimeSource, SetupError, SetupPhase, SetupRun, SetupSession,
    SetupStatus, valid_archive,
};
use crate::application::runtime_setup::RuntimePhase;

impl InferenceSetup {
    /// Starts the recovered operation only after the app session is published.
    ///
    /// # Errors
    /// Returns an error if the archive wait state cannot be recorded.
    pub async fn resume_after_startup(self: &Arc<Self>) -> Result<(), JournalError> {
        let mut session = self.admission.lock().await;
        let Some(mut run) = session.pending.take() else {
            return Ok(());
        };
        if !self.approved_runtime(&run.intent) {
            self.journal.transition(
                run.lease,
                run.status.phase,
                SetupPhase::Failed,
                Some("The approved runtime changed. Review a new installation estimate."),
            )?;
        } else if run.intent.selection.runtime_source == RuntimeSource::LocalArchive
            && matches!(
                run.status.phase,
                SetupPhase::Runtime | SetupPhase::WaitingForArchive
            )
            && !matches!(self.runtime.status().state, RuntimePhase::Ready)
        {
            if self.journal.set_archive_wait(run.lease, true)? {
                run.status.phase = SetupPhase::WaitingForArchive;
                session.pending = Some(run);
                self.watch_verified_runtime(&mut session);
            }
        } else {
            self.clear_archive_wait(&mut run)?;
            self.launch(&mut session, run, None);
        }
        Ok(())
    }

    /// Supplies the missing local ZIP for the same confirmed operation.
    ///
    /// # Errors
    /// Returns an error if the local path or waiting operation does not match.
    pub async fn provide_archive(
        self: &Arc<Self>,
        operation_id: Uuid,
        path: PathBuf,
    ) -> Result<SetupStatus, SetupError> {
        if !valid_archive(&path) {
            return Err(SetupError::ArchiveRequired);
        }
        let mut session = self.admission.lock().await;
        if *self.closing.borrow() {
            return Err(SetupError::Unavailable);
        }
        let waiting = session.pending.as_ref().is_some_and(|run| {
            run.lease.id == operation_id && run.status.phase == SetupPhase::WaitingForArchive
        });
        if !waiting {
            return Err(JournalError::Conflict.into());
        }
        let mut run = session.pending.take().ok_or(SetupError::Unavailable)?;
        if let Err(error) = self.clear_archive_wait(&mut run) {
            session.pending = Some(run);
            return Err(error.into());
        }
        let status = run.status.clone();
        if let Some(watcher) = session.task.take() {
            watcher.abort();
        }
        self.launch(&mut session, run, Some(path));
        Ok(status)
    }

    /// Cancels an operation that is waiting for its local ZIP.
    ///
    /// # Errors
    /// Returns an error if no matching pending operation can be cancelled.
    pub async fn cancel_archive(&self, operation_id: Uuid) -> Result<SetupStatus, SetupError> {
        let mut session = self.admission.lock().await;
        let pending = session
            .pending
            .as_ref()
            .filter(|run| {
                run.lease.id == operation_id && run.status.phase == SetupPhase::WaitingForArchive
            })
            .ok_or(JournalError::Conflict)?;
        if !self.journal.transition(
            pending.lease,
            SetupPhase::WaitingForArchive,
            SetupPhase::Interrupted,
            None,
        )? {
            return Err(JournalError::Conflict.into());
        }
        let mut status = pending.status.clone();
        status.phase = SetupPhase::Interrupted;
        status.error = None;
        session.pending = None;
        session.lease = None;
        if let Some(watcher) = session.task.take() {
            watcher.abort();
        }
        Ok(status)
    }

    fn watch_verified_runtime(self: &Arc<Self>, session: &mut SetupSession) {
        let owner = self.clone();
        let mut ready = self.runtime.subscribe_ready();
        let mut closing = self.closing.subscribe();
        session.task = Some(tokio::spawn(async move {
            let verified = tokio::select! {
                biased;
                _ = closing.wait_for(|closed| *closed) => false,
                available = ready.wait_for(|verified| *verified) => available.is_ok(),
            };
            if verified {
                owner.resume_with_verified_runtime().await;
            }
        }));
    }

    async fn resume_with_verified_runtime(self: &Arc<Self>) {
        let mut session = self.admission.lock().await;
        if *self.closing.borrow() {
            return;
        }
        if let Some(mut run) = session.pending.take() {
            if self.clear_archive_wait(&mut run).is_ok() {
                self.launch(&mut session, run, None);
            } else {
                session.pending = Some(run);
                tracing::warn!(
                    code = "inference_setup_checkpoint_failed",
                    "Could not resume after runtime verification"
                );
            }
        }
    }

    fn clear_archive_wait(&self, run: &mut SetupRun) -> Result<(), JournalError> {
        if run.status.phase == SetupPhase::WaitingForArchive {
            if !self.journal.set_archive_wait(run.lease, false)? {
                return Err(JournalError::Conflict);
            }
            run.status.phase = SetupPhase::Runtime;
        }
        Ok(())
    }

    pub(super) fn launch(
        self: &Arc<Self>,
        session: &mut SetupSession,
        run: SetupRun,
        archive: Option<PathBuf>,
    ) {
        session.lease = Some(run.lease);
        let owner = self.clone();
        session.task = Some(tokio::spawn(
            async move { owner.execute(run, archive).await },
        ));
    }

    pub(super) fn approved_runtime(&self, intent: &super::SetupIntent) -> bool {
        intent.version == super::SetupIntent::VERSION
            && self.runtime.identity().as_ref() == Some(&intent.runtime)
    }
}
