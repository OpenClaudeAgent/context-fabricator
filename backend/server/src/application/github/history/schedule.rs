//! A scheduled observation is valid only for the selected authority and due time.
use super::{Collector, CredentialStore, Error, Refresh, Scope, Snapshot, preparation::Attempts};
use crate::application::github::finish_collection;
use crate::application::telemetry::{Observer, Stage, TraceLink, TraceOrigin};
use crate::{application::github::GitHub, domain::github::ScheduleDue};

pub trait ScheduleStore: Send + Sync {
    /// # Errors
    /// Returns a storage error for unreadable preparation history.
    fn preparation_attempts(&self, scope: &Scope) -> Result<Attempts, Error>;
    /// # Errors
    /// Rejects obsolete authority and stale collection results.
    fn save_preparation_attempts(
        &self,
        scope: &Scope,
        tip: &str,
        attempts: &Attempts,
    ) -> Result<(), Error>;

    /// # Errors
    /// Returns a storage error when eligible history schedules cannot be read.
    fn history_due(&self, id: Option<i64>) -> Result<Option<ScheduleDue>, Error>;
    /// # Errors
    /// Rejects obsolete authority, cadence or unavailable storage.
    fn claim_history(
        &self,
        scope: &Scope,
        due: &ScheduleDue,
        trace: Option<TraceLink>,
    ) -> Result<(), Error>;
    /// # Errors
    /// Rejects stale scope or unavailable storage.
    fn history_failed(
        &self,
        scope: &Scope,
        expected_due_ms: i64,
        checked: i64,
        error: Error,
    ) -> Result<(), Error>;
}

impl GitHub {
    /// # Errors
    /// Returns a storage error if the durable schedule is unavailable.
    pub fn next_history(&self) -> Result<Option<ScheduleDue>, Error> {
        if self.credentials.is_none() {
            return Ok(None);
        }
        self.history
            .as_ref()
            .map_or(Ok(None), |c| c.store.history_due(None))
    }

    /// # Errors
    /// Rejects changed authority, obsolete cadence, or a failed collection.
    pub async fn run_history(
        &self,
        due: &ScheduleDue,
        observer: Option<&dyn Observer>,
    ) -> Result<(), Error> {
        self.history
            .as_ref()
            .ok_or(Error::Storage)?
            .refresh(
                due.repository_id,
                Some(due),
                self.credentials.as_deref().ok_or(Error::Credentials)?,
                observer.map(|observer| TraceOrigin {
                    observer,
                    parent: None,
                }),
                |reference| self.store.expire(reference),
            )
            .await
            .map(|_| ())
    }
}

impl Collector {
    pub(in crate::application::github) async fn refresh(
        &self,
        id: i64,
        expected: Option<&ScheduleDue>,
        credentials: &(dyn CredentialStore + Send + Sync),
        origin: Option<TraceOrigin<'_>>,
        expire: impl FnOnce(&str) -> Result<(), Error>,
    ) -> Result<Refresh, Error> {
        let scope = self.store.history_scope(id)?;
        let observed = self.store.history_due(Some(id))?.ok_or(Error::Access)?;
        if observed.generation != scope.generation
            || expected.is_some_and(|due| *due != observed || due.at_ms > (self.now)())
        {
            return Err(Error::Conflict);
        }
        let _owner = self.active.lock().await;
        let current = self.store.history_due(Some(id))?.ok_or(Error::Access)?;
        if current != observed {
            // An explicit request can share a successful refresh of the same authority.
            // A stale scheduler selection never initiates work under a different schedule.
            if expected.is_none()
                && current.generation == observed.generation
                && current.interval_minutes == observed.interval_minutes
            {
                let state = self.state(id)?;
                if state.schedule.is_some_and(|entry| {
                    entry.last_error.is_none() && entry.next_at_ms == current.at_ms
                }) && let Some(snapshot) = state.snapshot
                    && snapshot.branch == scope.branch
                {
                    return Ok(Refresh {
                        snapshot,
                        origin: state.published_trace,
                    });
                }
            }
            return Err(Error::Conflict);
        }
        let (context, job) = origin
            .filter(|_| expected.is_some())
            .map(|origin| origin.start(Stage::GitHubHistoryJob))
            .unzip();
        let result = self
            .collect_due(
                &scope,
                &observed,
                expected.is_some(),
                credentials,
                context.map(TraceOrigin::from).or(origin),
                expire,
            )
            .await;
        finish_collection(job, &result);
        result.map(|snapshot| Refresh {
            snapshot,
            origin: None,
        })
    }

    async fn collect_due(
        &self,
        scope: &Scope,
        due: &ScheduleDue,
        scheduled: bool,
        credentials: &(dyn CredentialStore + Send + Sync),
        origin: Option<TraceOrigin<'_>>,
        expire: impl FnOnce(&str) -> Result<(), Error>,
    ) -> Result<Snapshot, Error> {
        let (trace, span) = origin
            .map(|origin| origin.start(Stage::GitHubHistoryCollection))
            .unzip();
        if let Err(error) =
            self.store
                .claim_history(scope, due, trace.map(|context| context.parent))
        {
            finish_collection::<()>(span, &Err(error));
            return Err(error);
        }
        let result = self.collect(scope, due.at_ms, credentials, trace).await;
        finish_collection(span, &result);
        if let Ok(snapshot) = &result
            && scheduled
        {
            // Lineage and its next cadence are already durable, independently of comparisons.
            self.prepare_next(scope, snapshot, credentials, origin, expire)
                .await?;
            return result;
        }
        if let Err(error) = &result {
            self.store
                .history_failed(scope, due.at_ms, (self.now)(), *error)?;
            if *error == Error::Reconnect {
                expire(&scope.credential)?;
            }
        }
        result
    }
}
