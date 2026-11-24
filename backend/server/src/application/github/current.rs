//! Current-code collection from a configured GitHub scope.

use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use super::{CredentialError, CredentialStore, GitHubFuture, Provider};
use crate::application::ports::SnapshotStore;
use crate::application::telemetry::{Event, Observer, Stage, TraceContext, TraceLink};
use crate::domain::corpus::snapshot::{Phase, Plan, Snapshot, SnapshotError};
use crate::domain::github::{
    Error,
    current::{Due, Entry, Pin, Published, Scope},
};
use crate::domain::sync::{JobState, Phase as JobPhase};

mod walk;

#[derive(Clone, Copy)]
pub struct SourceRequest<'a> {
    pub secret: &'a [u8],
    pub scope: &'a Scope,
    pub trace: Option<TraceContext<'a>>,
}

pub trait Source: Send + Sync {
    fn verify_repository<'a>(&'a self, request: SourceRequest<'a>) -> GitHubFuture<'a, String>;
    fn verify_contents<'a>(&'a self, request: SourceRequest<'a>) -> GitHubFuture<'a, ()>;
    fn branch<'a>(&'a self, request: SourceRequest<'a>) -> GitHubFuture<'a, Pin>;
    fn prepare<'a>(
        &'a self,
        request: SourceRequest<'a>,
        revision: &'a str,
    ) -> GitHubFuture<'a, Option<Arc<dyn Objects>>>;
    fn evict_inactive(&self);
    fn commit_tree<'a>(
        &'a self,
        request: SourceRequest<'a>,
        id: &'a str,
    ) -> GitHubFuture<'a, String>;
    fn tree<'a>(&'a self, request: SourceRequest<'a>, id: &'a str) -> GitHubFuture<'a, Vec<Entry>>;
    fn blob<'a>(
        &'a self,
        request: SourceRequest<'a>,
        id: &'a str,
        bytes: u32,
    ) -> GitHubFuture<'a, Option<String>>;
}

pub trait Objects: Send + Sync {
    /// # Errors
    /// Rejects an invalid or unavailable commit.
    fn root(&self, revision: &str) -> Result<String, Error>;
    /// # Errors
    /// Rejects an invalid or unavailable tree.
    fn tree(&self, id: &str) -> Result<Vec<Entry>, Error>;
    /// # Errors
    /// Rejects an invalid or unavailable blob.
    fn blob(&self, id: &str, bytes: u32) -> Result<Option<String>, Error>;
}

pub trait ScheduleStore: Send + Sync {
    /// # Errors
    /// Returns a storage error if eligible collection jobs cannot be read.
    fn next(&self) -> Result<Option<Due>, Error>;
    /// # Errors
    /// Rejects a stale scope or an unavailable job store.
    fn enqueue(
        &self,
        repository_id: i64,
        now: i64,
        trigger: Option<TraceLink>,
    ) -> Result<Option<TraceLink>, Error>;
    /// # Errors
    /// Rejects a missing or inaccessible scope when resuming manually.
    fn resume_scope(&self, repository_id: i64) -> Result<Scope, Error>;
    /// # Errors
    /// Rejects a stale scope, a job that is not due, or an unavailable job store.
    fn claim(&self, scope: &Scope, request: ClaimRequest<'_>, now: i64) -> Result<Claim, Error>;
    /// # Errors
    /// Returns a storage error if the job result cannot be saved.
    /// Returns `false` when the captured scope is no longer current.
    fn finish(
        &self,
        scope: &Scope,
        state: JobState,
        error: Option<Error>,
        now: i64,
    ) -> Result<bool, Error>;
}

pub struct ClaimRequest<'a> {
    pub manual: bool,
    pub trigger: Option<TraceLink>,
    /// Lazily allocate a scheduler root when this job has no explicit trigger.
    pub new_origin: &'a mut dyn FnMut() -> Option<TraceLink>,
}

pub struct Claim {
    pub failures: u8,
    pub origin: Option<TraceLink>,
}

pub struct Refresh {
    pub origin: Option<TraceLink>,
    pub result: Result<Published, Error>,
}

enum Trigger {
    Manual(Option<TraceLink>),
    Scheduled,
}

pub trait Store: SnapshotStore + ScheduleStore {
    /// Publish the eligible source projections together under the captured scope.
    /// # Errors
    /// Rejects a stale pin, publication predecessor or credential generation.
    fn publish_current(&self, scope: &Scope, stage: i64) -> Result<(), Error>;
    /// Rebuild a stale graph projection from the already published manifest.
    /// # Errors
    /// Rejects a replaced publication, changed scope, or insufficient graph storage.
    fn refresh_graph_profile(&self, scope: &Scope, publication: i64) -> Result<bool, Error>;
    /// # Errors
    /// Rejects a repository without verified current-code access.
    fn current_scope(&self, id: i64) -> Result<Scope, Error>;
    /// # Errors
    /// Returns a conflict if the scope or credential changed.
    fn check_scope(&self, scope: &Scope) -> Result<(), Error>;
    /// # Errors
    /// Returns a storage error if current credential expiry cannot be recorded.
    fn expire_scope_credential(&self, scope: &Scope) -> Result<(), Error>;
    /// # Errors
    /// Returns a storage error if confirmed Contents denial cannot be recorded.
    fn retire_scope(&self, scope: &Scope) -> Result<(), Error>;
}

pub(super) struct Collector {
    source: Arc<dyn Source>,
    store: Arc<dyn Store>,
    active: Mutex<()>,
    now: Box<dyn Fn() -> i64 + Send + Sync>,
    wake: Notify,
}

impl Collector {
    pub(super) fn new(
        source: Arc<dyn Source>,
        store: Arc<dyn Store>,
        now: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Self {
        Self {
            source,
            store,
            active: Mutex::new(()),
            now: Box::new(now),
            wake: Notify::new(),
        }
    }

    pub(super) async fn refresh(
        &self,
        id: i64,
        credentials: &(dyn CredentialStore + Send + Sync),
        provider: &dyn Provider,
        observer: Option<&dyn Observer>,
        trigger: Option<TraceLink>,
    ) -> Result<Refresh, Error> {
        let _active = self.active.try_lock().map_err(|_| Error::Conflict)?;
        let scope = self.store.resume_scope(id)?;
        self.run(
            &scope,
            Trigger::Manual(trigger),
            credentials,
            provider,
            observer,
        )
        .await
    }

    pub(super) fn queue(
        &self,
        id: i64,
        trigger: Option<TraceLink>,
    ) -> Result<Option<TraceLink>, Error> {
        let origin = self.store.enqueue(id, (self.now)(), trigger)?;
        self.changed();
        Ok(origin)
    }

    pub(super) fn changed(&self) {
        self.source.evict_inactive();
        self.wake.notify_one();
    }

    pub(super) async fn wait_for_change(&self) {
        self.wake.notified().await;
    }

    pub(super) fn next(&self) -> Result<Option<Due>, Error> {
        self.store.next()
    }

    pub(super) async fn run_due(
        &self,
        due: &Due,
        credentials: &(dyn CredentialStore + Send + Sync),
        provider: &dyn Provider,
        observer: Option<&dyn Observer>,
    ) -> Result<Published, Error> {
        let _active = self.active.lock().await;
        let scope = self.store.current_scope(due.repository_id)?;
        if scope.collection != due.collection || scope.generation != due.generation {
            return Err(Error::Conflict);
        }
        self.run(&scope, Trigger::Scheduled, credentials, provider, observer)
            .await?
            .result
    }

    async fn run(
        &self,
        scope: &Scope,
        trigger: Trigger,
        credentials: &(dyn CredentialStore + Send + Sync),
        provider: &dyn Provider,
        observer: Option<&dyn Observer>,
    ) -> Result<Refresh, Error> {
        let mut job_root = None;
        let (manual, trigger) = match trigger {
            Trigger::Manual(trace) => (true, trace),
            Trigger::Scheduled => (false, None),
        };
        let claim = self.store.claim(
            scope,
            ClaimRequest {
                manual,
                trigger,
                new_origin: &mut || {
                    job_root =
                        observer.map(|observer| observer.start(Stage::GitHubCurrentJob, None));
                    job_root.as_ref().map(|span| span.link())
                },
            },
            (self.now)(),
        )?;
        if let Some(root) = job_root {
            root.finish(true);
        }
        if manual {
            self.changed();
        }
        let span =
            observer.map(|observer| observer.start(Stage::GitHubCurrentCollection, claim.origin));
        let trace = observer
            .zip(span.as_ref())
            .map(|(observer, span)| TraceContext {
                observer,
                parent: span.link(),
            });
        let result = self.with_credential(scope, credentials, trace).await;
        let error = result.as_ref().err().copied();
        let now = (self.now)();
        let state = outcome(error, claim.failures, now, provider);
        let recorded = self.store.finish(scope, state, error, now);
        if let Some(mut span) = span {
            if let Some(event) = match (state.phase, &result, &recorded) {
                (_, _, Ok(false)) => Some(Event::CollectionScopeChanged),
                (JobPhase::Throttled, _, Ok(true)) => Some(Event::CollectionCooldown),
                (JobPhase::Retry, _, Ok(true)) => Some(Event::CollectionRetry),
                (JobPhase::Succeeded, Ok(publication), Ok(true)) if publication.changed => {
                    Some(Event::CollectionPublished)
                }
                (JobPhase::Succeeded, Ok(_), Ok(true)) => Some(Event::CollectionUnchanged),
                _ => None,
            } {
                span.event(event);
            }
            span.finish(result.is_ok() && recorded == Ok(true));
        }
        Ok(Refresh {
            origin: claim.origin,
            result: recorded.and(result),
        })
    }

    async fn with_credential(
        &self,
        scope: &Scope,
        credentials: &(dyn CredentialStore + Send + Sync),
        trace: Option<TraceContext<'_>>,
    ) -> Result<Published, Error> {
        let secret = match credentials.read(&scope.credential) {
            Ok(secret) => secret,
            Err(CredentialError::Missing) => {
                self.store.expire_scope_credential(scope)?;
                self.source.evict_inactive();
                return Err(Error::Reconnect);
            }
            Err(_) => return Err(Error::Credentials),
        };
        let request = SourceRequest {
            scope,
            secret: &secret,
            trace,
        };
        let result = self.collect(request).await;
        if matches!(result, Err(Error::Reconnect)) {
            self.store.expire_scope_credential(scope)?;
            self.source.evict_inactive();
        } else if matches!(result, Err(Error::Access)) && self.store.check_scope(scope).is_ok() {
            match self.source.verify_contents(request).await {
                Err(Error::Access) => self.store.retire_scope(scope)?,
                Err(Error::Reconnect) => self.store.expire_scope_credential(scope)?,
                _ => {}
            }
            self.source.evict_inactive();
        }
        result
    }

    async fn collect(&self, request: SourceRequest<'_>) -> Result<Published, Error> {
        self.store.check_scope(request.scope)?;
        let full_name = self.source.verify_repository(request).await?;
        let canonical = Scope {
            full_name,
            ..request.scope.clone()
        };
        let scope = &canonical;
        let request = SourceRequest { scope, ..request };
        self.store.check_scope(scope)?;
        let (mut stage, root) = self.stage(request).await?;
        if stage.phase == Phase::Published {
            return Ok(Published {
                commit: stage.revision,
                changed: self.store.refresh_graph_profile(scope, stage.id)?,
            });
        }
        let prepared = if stage.phase == Phase::Staging {
            walk::stage_files(self, request, &mut stage, root).await
        } else {
            // A sealed stage can outlive upstream permission changes across a restart.
            self.source
                .commit_tree(request, &stage.revision)
                .await
                .map(|_| ())
        };
        if matches!(prepared, Err(Error::ExpiredPin)) {
            let branch = self.source.branch(request).await?;
            if branch.revision == stage.revision {
                return Err(Error::Upstream);
            }
        }
        if matches!(prepared, Err(Error::CollectionLimit | Error::ExpiredPin)) {
            self.store.check_scope(scope)?;
            self.store
                .discard_snapshot(stage.id)
                .map_err(snapshot_error)?;
        }
        prepared?;
        if self.source.verify_repository(request).await? != scope.full_name {
            return Err(Error::Conflict);
        }
        self.store.check_scope(scope)?;
        self.store.publish_current(scope, stage.id)?;
        Ok(Published {
            commit: stage.revision,
            changed: true,
        })
    }

    async fn stage(&self, request: SourceRequest<'_>) -> Result<(Snapshot, Option<String>), Error> {
        let scope = request.scope;
        if let Some(pending) = self
            .store
            .pending_snapshot(&scope.collection)
            .map_err(snapshot_error)?
        {
            if pending.generation != scope.generation {
                return Err(Error::Conflict);
            }
            return Ok((pending, None));
        }
        let pin = self.source.branch(request).await?;
        let snapshot = self
            .store
            .begin_snapshot(&Plan {
                collection: &scope.collection,
                generation: scope.generation,
                predecessor: scope.predecessor,
                revision: &pin.revision,
                tracked_ref: &format!("refs/heads/{}", scope.branch),
            })
            .map_err(snapshot_error)?;
        Ok((snapshot, Some(pin.tree)))
    }
}

fn outcome(error: Option<Error>, failures: u8, now: i64, provider: &dyn Provider) -> JobState {
    match error {
        None => JobState::due(JobPhase::Succeeded, now),
        Some(Error::Throttled) => JobState {
            phase: JobPhase::Throttled,
            due_at: Some(
                provider
                    .traffic()
                    .ok()
                    .flatten()
                    .and_then(|traffic| traffic.cooldown_until_ms)
                    .unwrap_or_default()
                    .max(now.saturating_add(1_000)),
            ),
            failures,
        },
        Some(
            Error::Upstream
            | Error::Credentials
            | Error::Storage
            | Error::Conflict
            | Error::ExpiredPin,
        ) => {
            let jitter = getrandom::u32().map_or(0, |random| random.to_le_bytes()[0]);
            JobState::after_failure(now, failures, jitter)
        }
        _ => JobState::blocked(JobPhase::Failed),
    }
}

fn snapshot_error(error: SnapshotError) -> Error {
    match error {
        SnapshotError::Conflict => Error::Conflict,
        SnapshotError::AccessLost => Error::Access,
        SnapshotError::Invalid => Error::CollectionLimit,
        SnapshotError::Storage => Error::Storage,
        SnapshotError::StorageLimit => Error::StorageLimit,
    }
}
