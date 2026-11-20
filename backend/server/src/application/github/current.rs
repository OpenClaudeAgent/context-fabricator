//! Current-code collection from a configured GitHub scope.

use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use super::{CredentialError, CredentialStore, GitHubFuture, Provider};
use crate::application::ports::SnapshotStore;
use crate::application::telemetry::{Observer, Stage};
use crate::domain::corpus::snapshot::{Phase, Plan, Snapshot, SnapshotError};
use crate::domain::github::{
    Error,
    current::{Due, Entry, Pin, Published, Scope},
};
use crate::domain::sync::{JobState, Phase as JobPhase};

mod walk;

pub trait Source: Send + Sync {
    fn verify_repository<'a>(
        &'a self,
        secret: &'a [u8],
        scope: &'a Scope,
    ) -> GitHubFuture<'a, String>;
    fn verify_contents<'a>(&'a self, secret: &'a [u8], scope: &'a Scope) -> GitHubFuture<'a, ()>;
    fn branch<'a>(&'a self, secret: &'a [u8], scope: &'a Scope) -> GitHubFuture<'a, Pin>;
    fn commit_tree<'a>(
        &'a self,
        secret: &'a [u8],
        scope: &'a Scope,
        id: &'a str,
    ) -> GitHubFuture<'a, String>;
    fn tree<'a>(
        &'a self,
        secret: &'a [u8],
        scope: &'a Scope,
        id: &'a str,
    ) -> GitHubFuture<'a, Vec<Entry>>;
    fn blob<'a>(
        &'a self,
        secret: &'a [u8],
        scope: &'a Scope,
        id: &'a str,
        bytes: u32,
    ) -> GitHubFuture<'a, Option<String>>;
}

pub trait ScheduleStore: Send + Sync {
    /// # Errors
    /// Returns a storage error if eligible collection jobs cannot be read.
    fn next(&self) -> Result<Option<Due>, Error>;
    /// # Errors
    /// Rejects a stale scope or an unavailable job store.
    fn enqueue(&self, scope: &Scope, now: i64) -> Result<(), Error>;
    /// # Errors
    /// Rejects a stale scope, a job that is not due, or an unavailable job store.
    fn claim(&self, scope: &Scope, manual: bool, now: i64) -> Result<u8, Error>;
    /// # Errors
    /// Returns a storage error if the job result cannot be saved.
    fn finish(
        &self,
        scope: &Scope,
        state: JobState,
        error: Option<Error>,
        now: i64,
    ) -> Result<(), Error>;
}

pub trait Store: SnapshotStore + ScheduleStore {
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
    ) -> Result<Published, Error> {
        let _active = self.active.try_lock().map_err(|_| Error::Conflict)?;
        let scope = self.store.current_scope(id)?;
        self.run(&scope, true, credentials, provider, None).await
    }

    pub(super) fn queue(&self, id: i64) -> Result<(), Error> {
        self.store
            .enqueue(&self.store.current_scope(id)?, (self.now)())?;
        self.changed();
        Ok(())
    }

    pub(super) fn changed(&self) {
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
        self.run(&scope, false, credentials, provider, observer)
            .await
    }

    async fn run(
        &self,
        scope: &Scope,
        manual: bool,
        credentials: &(dyn CredentialStore + Send + Sync),
        provider: &dyn Provider,
        observer: Option<&dyn Observer>,
    ) -> Result<Published, Error> {
        let failures = self.store.claim(scope, manual, (self.now)())?;
        if manual {
            self.changed();
        }
        let span = observer.map(|observer| observer.start(Stage::GitHubCurrentCode, None));
        let result = self.with_credential(scope, credentials).await;
        let error = result.as_ref().err().copied();
        let now = (self.now)();
        let recorded =
            self.store
                .finish(scope, outcome(error, failures, now, provider), error, now);
        if let Some(span) = span {
            span.finish(result.is_ok() && recorded.is_ok());
        }
        recorded?;
        result
    }

    async fn with_credential(
        &self,
        scope: &Scope,
        credentials: &(dyn CredentialStore + Send + Sync),
    ) -> Result<Published, Error> {
        let secret = match credentials.read(&scope.credential) {
            Ok(secret) => secret,
            Err(CredentialError::Missing) => {
                self.store.expire_scope_credential(scope)?;
                return Err(Error::Reconnect);
            }
            Err(_) => return Err(Error::Credentials),
        };
        let result = self.collect(scope, &secret).await;
        if matches!(result, Err(Error::Reconnect)) {
            self.store.expire_scope_credential(scope)?;
        } else if matches!(result, Err(Error::Access)) && self.store.check_scope(scope).is_ok() {
            match self.source.verify_contents(&secret, scope).await {
                Err(Error::Access) => self.store.retire_scope(scope)?,
                Err(Error::Reconnect) => self.store.expire_scope_credential(scope)?,
                _ => {}
            }
        }
        result
    }

    async fn collect(&self, scope: &Scope, secret: &[u8]) -> Result<Published, Error> {
        self.store.check_scope(scope)?;
        let full_name = self.source.verify_repository(secret, scope).await?;
        let canonical = Scope {
            full_name,
            ..scope.clone()
        };
        let scope = &canonical;
        self.store.check_scope(scope)?;
        let (mut stage, root) = self.stage(scope, secret).await?;
        if stage.phase == Phase::Published {
            return Ok(Published {
                commit: stage.revision,
                changed: false,
            });
        }
        let prepared = if stage.phase == Phase::Staging {
            walk::stage_files(self, scope, secret, &mut stage, root).await
        } else {
            // A sealed stage can outlive upstream permission changes across a restart.
            self.source
                .commit_tree(secret, scope, &stage.revision)
                .await
                .map(|_| ())
        };
        if matches!(prepared, Err(Error::ExpiredPin)) {
            let branch = self.source.branch(secret, scope).await?;
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
        if self.source.verify_repository(secret, scope).await? != scope.full_name {
            return Err(Error::Conflict);
        }
        self.store.check_scope(scope)?;
        self.store
            .publish_snapshot(stage.id)
            .map_err(snapshot_error)?;
        Ok(Published {
            commit: stage.revision,
            changed: true,
        })
    }

    async fn stage(
        &self,
        scope: &Scope,
        secret: &[u8],
    ) -> Result<(Snapshot, Option<String>), Error> {
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
        let pin = self.source.branch(secret, scope).await?;
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
    }
}
