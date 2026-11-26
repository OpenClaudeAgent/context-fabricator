//! GitHub onboarding and repository configuration, independent of HTTP and Keychain.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::domain::github::{
    Access, ConfiguredRepository, Discovery, Error, MAX_REQUESTS_PER_MINUTE, Repository,
    RepositoryName, Selection, Snapshot, TokenSetup, TrafficStatus,
};

use super::credentials::{CredentialError, CredentialStore};
use super::telemetry::{Observation, Observer, TraceContext, TraceLink, TraceOrigin};
use crate::domain::github::current::{Due, Published};
use current::Collector;

pub mod actions;
pub mod branches;
pub mod current;
pub mod graph;
pub mod history;

pub type GitHubFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

#[derive(Clone, Copy)]
pub struct MetadataRequest<'a> {
    pub secret: &'a [u8],
    pub guard: &'a (dyn Fn() -> Result<(), Error> + Send + Sync),
    pub trace: Option<TraceContext<'a>>,
}

fn finish_collection<T>(span: Option<Box<dyn Observation>>, result: &Result<T, Error>) {
    if let Some(mut span) = span {
        if let Err(error) = result {
            span.problem_code(error.schedule_code());
        }
        span.finish(result.is_ok());
    }
}

#[derive(Clone, Copy)]
pub enum AccessProbe<'a> {
    Contents(&'a str),
    WorkflowRuns,
}

pub trait Provider: Send + Sync {
    fn public_actions<'a>(
        &'a self,
        _repository: &'a str,
        _revision: &'a str,
    ) -> GitHubFuture<'a, Vec<crate::domain::github::actions::Snapshot>> {
        Box::pin(async { Err(Error::Upstream) })
    }

    fn public_snapshot<'a>(
        &'a self,
        _name: RepositoryName<'a>,
        _revision: &'a str,
    ) -> GitHubFuture<'a, Arc<dyn current::Objects>> {
        Box::pin(async { Err(Error::Upstream) })
    }

    /// # Errors
    /// Returns an error if the outbound accounting store is unavailable.
    fn traffic(&self) -> Result<Option<TrafficStatus>, Error> {
        Ok(None)
    }
    fn verify<'a>(&'a self, secret: &'a [u8]) -> GitHubFuture<'a, String>;
    fn discover<'a>(&'a self, secret: &'a [u8], page: u16) -> GitHubFuture<'a, Discovery>;
    fn branches<'a>(
        &'a self,
        source: MetadataRequest<'a>,
        name: RepositoryName<'a>,
        page: u16,
    ) -> GitHubFuture<'a, crate::domain::github::branches::Page>;
    fn repository<'a>(
        &'a self,
        secret: &'a [u8],
        name: RepositoryName<'a>,
    ) -> GitHubFuture<'a, Repository>;
    fn probe_access<'a>(
        &'a self,
        secret: &'a [u8],
        repository: &'a Repository,
        probe: AccessProbe<'a>,
    ) -> GitHubFuture<'a, bool>;
}

pub struct Binding {
    pub revision: u32,
    pub reference: Option<String>,
    pub reconnect: bool,
}

pub trait Store: graph::Store + Send + Sync {
    /// Journal a new reference before writing to the credential store.
    /// # Errors
    /// Returns an error if the reference cannot be staged safely.
    fn stage_credential(&self, reference: &str) -> Result<(), Error>;
    /// List pending references that are not currently bound.
    /// # Errors
    /// Returns an error if the cleanup journal cannot be read.
    fn pending_credentials(&self) -> Result<Vec<String>, Error>;
    /// Forget a successfully deleted (or already missing) reference.
    /// # Errors
    /// Returns an error if the cleanup journal cannot be updated.
    fn forget_credential(&self, reference: &str) -> Result<(), Error>;
    /// # Errors
    /// Returns an error if GitHub state cannot be read.
    fn snapshot(&self) -> Result<Snapshot, Error>;
    /// # Errors
    /// Returns an error if the binding cannot be read.
    fn binding(&self) -> Result<Binding, Error>;
    /// # Errors
    /// Returns a conflict for a stale revision or an error if persistence fails.
    fn bind(&self, revision: u32, credential: Option<(&str, &str)>) -> Result<(), Error>;
    /// # Errors
    /// Returns an error if expiry cannot be recorded.
    fn expire(&self, reference: &str) -> Result<(), Error>;
    /// # Errors
    /// Returns a conflict for a stale revision or an error if persistence fails.
    fn save(&self, revision: u32, config: &ConfiguredRepository) -> Result<(), Error>;
    /// Persist verified permissions if the captured configuration is still current.
    /// # Errors
    /// Rejects stale revisions and unavailable storage.
    fn record_access(&self, revision: u32, config: &ConfiguredRepository) -> Result<bool, Error>;
    /// # Errors
    /// Returns a conflict for a stale revision or an error if persistence fails.
    fn remove(&self, revision: u32, id: i64) -> Result<(), Error>;
    /// # Errors
    /// Rejects an unknown repository or an unavailable store.
    fn purge_data(&self, id: i64) -> Result<crate::domain::github::StorageBudget, Error>;
    /// # Errors
    /// Returns an error if repository state is unavailable.
    fn data_state(&self, id: i64) -> Result<crate::domain::github::DataState, Error>;
    /// # Errors
    /// Returns an error if the pause cannot be cleared.
    fn resume_data(&self, id: i64, stream: &str) -> Result<(), Error>;
    /// # Errors
    /// Returns an error if logical storage accounting is unavailable.
    fn storage_budget(&self) -> Result<crate::domain::github::StorageBudget, Error>;
    /// # Errors
    /// Rejects invalid limits or a stale settings revision.
    fn set_storage_budget(
        &self,
        revision: u32,
        limit_gb: u16,
    ) -> Result<crate::domain::github::StorageBudget, Error>;
    /// # Errors
    /// Returns a conflict for a stale revision or an error if persistence fails.
    fn set_rate(&self, revision: u32, requests_per_minute: u16) -> Result<(), Error>;
}

pub struct GitHub {
    provider: Arc<dyn Provider>,
    store: Arc<dyn Store>,
    credentials: Option<Arc<dyn CredentialStore + Send + Sync>>,
    create_reference: fn() -> Result<String, Error>,
    current_code: Option<Collector>,
    history: Option<history::Collector>,
    actions: Option<actions::Collector>,
}

// Credential operations are synchronous. Serialize them across GitHub instances so a
// sweep cannot delete a staged reference while another authorization is saving it.
static CREDENTIAL_OPERATIONS: Mutex<()> = Mutex::new(());

impl GitHub {
    /// # Errors
    /// Returns an error when GitHub cannot supply the public runs for this revision.
    pub async fn public_actions(
        &self,
        repository: &str,
        revision: &str,
    ) -> Result<Vec<crate::domain::github::actions::Snapshot>, Error> {
        self.provider.public_actions(repository, revision).await
    }

    /// # Errors
    /// Rejects unavailable public Git objects.
    pub async fn public_snapshot(
        &self,
        name: RepositoryName<'_>,
        revision: &str,
    ) -> Result<Arc<dyn current::Objects>, Error> {
        self.provider.public_snapshot(name, revision).await
    }

    #[must_use]
    pub fn new(
        provider: Arc<dyn Provider>,
        store: Arc<dyn Store>,
        credentials: Option<Arc<dyn CredentialStore + Send + Sync>>,
        create_reference: fn() -> Result<String, Error>,
    ) -> Self {
        let github = Self {
            provider,
            store,
            credentials,
            create_reference,
            current_code: None,
            history: None,
            actions: None,
        };
        github.retry_credential_cleanup();
        github
    }

    /// Read whether collection is paused after explicit data deletion.
    /// # Errors
    /// Rejects invalid IDs and storage failures.
    pub fn data_state(&self, id: i64) -> Result<crate::domain::github::DataState, Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        self.store.data_state(id)
    }

    /// Purge only the selected GitHub repository's retained data and pause collection.
    /// # Errors
    /// Rejects invalid IDs and storage failures.
    pub fn purge_data(&self, id: i64) -> Result<crate::domain::github::StorageBudget, Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        let budget = self.store.purge_data(id)?;
        if let Some(current) = &self.current_code {
            current.changed();
        }
        Ok(budget)
    }

    /// Resume collection after an explicit user refresh.
    /// # Errors
    /// Rejects invalid IDs and storage failures.
    pub fn resume_data(&self, id: i64, stream: &str) -> Result<(), Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        self.store.resume_data(id, stream)?;
        if let Some(current) = &self.current_code {
            current.changed();
        }
        Ok(())
    }

    /// # Errors
    /// Returns an error if logical GitHub accounting is unavailable.
    pub fn storage_budget(&self) -> Result<crate::domain::github::StorageBudget, Error> {
        self.store.storage_budget()
    }

    /// # Errors
    /// Rejects invalid limits or a stale revision.
    pub fn set_storage_budget(
        &self,
        revision: u32,
        limit_gb: u16,
    ) -> Result<crate::domain::github::StorageBudget, Error> {
        self.store.set_storage_budget(revision, limit_gb)
    }

    #[must_use]
    pub fn with_current_code(
        mut self,
        source: Arc<dyn current::Source>,
        store: Arc<dyn current::Store>,
        now: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Self {
        self.current_code = Some(Collector::new(source, store, now));
        self
    }

    /// Reads an already published graph without contacting GitHub.
    /// # Errors
    /// Rejects invalid selectors, unknown repositories and unavailable storage.
    pub fn graph(&self, id: i64, query: &graph::Query) -> Result<graph::State, Error> {
        if id <= 0 || !query.valid() {
            return Err(Error::InvalidSelection);
        }
        self.store.graph(id, query)
    }

    #[must_use]
    pub fn with_actions(
        mut self,
        source: Arc<dyn actions::Source>,
        store: Arc<dyn actions::Store>,
        now: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Self {
        self.actions = Some(actions::Collector::new(source, store, now));
        self
    }

    /// # Errors
    /// Rejects invalid IDs or unavailable storage.
    pub fn actions_state(&self, id: i64) -> Result<crate::domain::github::actions::State, Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        self.actions.as_ref().ok_or(Error::Storage)?.state(id)
    }

    /// # Errors
    /// Rejects invalid, unauthorized, stale, or unavailable Actions refreshes.
    pub async fn refresh_actions(
        &self,
        id: i64,
        trace: Option<TraceOrigin<'_>>,
    ) -> Result<crate::domain::github::actions::Snapshot, Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        self.actions
            .as_ref()
            .ok_or(Error::Storage)?
            .refresh(
                id,
                None,
                self.credentials.as_deref().ok_or(Error::Credentials)?,
                trace,
                |reference| self.store.expire(reference),
            )
            .await
    }

    #[must_use]
    pub fn with_history(
        mut self,
        source: Arc<dyn history::Source>,
        store: Arc<dyn history::Store>,
        now: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Self {
        self.history = Some(history::Collector::new(source, store, now));
        self
    }

    /// # Errors
    /// Returns an error if Actions scheduling state is unavailable.
    pub fn next_actions(&self) -> Result<Option<crate::domain::github::actions::Due>, Error> {
        if self.credentials.is_none() {
            return Ok(None);
        }
        self.actions
            .as_ref()
            .map_or(Ok(None), actions::Collector::next)
    }

    /// # Errors
    /// Returns an error if the selected schedule changed or collection fails.
    pub async fn run_actions(
        &self,
        due: &crate::domain::github::actions::Due,
        observer: Option<&dyn Observer>,
    ) -> Result<(), Error> {
        self.actions
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

    /// # Errors
    /// Rejects invalid, unconfigured, or unavailable history.
    pub fn history_state(&self, id: i64) -> Result<crate::domain::github::history::State, Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        self.history.as_ref().ok_or(Error::Storage)?.state(id)
    }

    /// # Errors
    /// Rejects invalid, unauthorized, stale, or unavailable history refreshes.
    pub async fn refresh_history(
        &self,
        id: i64,
        trace: Option<TraceOrigin<'_>>,
    ) -> Result<history::Refresh, Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        self.history
            .as_ref()
            .ok_or(Error::Storage)?
            .refresh(
                id,
                None,
                self.credentials.as_deref().ok_or(Error::Credentials)?,
                trace,
                |reference| self.store.expire(reference),
            )
            .await
    }

    /// Read a bounded comparison selected from the active history lineage.
    /// # Errors
    /// Rejects invalid, stale, unauthorized, or unavailable comparisons.
    pub async fn history_comparison(
        &self,
        id: i64,
        child: &str,
        trace: Option<TraceContext<'_>>,
    ) -> Result<crate::domain::github::history::comparison::Comparison, Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        self.history
            .as_ref()
            .ok_or(Error::Storage)?
            .comparison(
                id,
                child,
                self.credentials.as_deref().ok_or(Error::Credentials)?,
                trace,
                |reference| self.store.expire(reference),
            )
            .await
    }

    /// Read durable evidence only when its child and parent remain in the active lineage.
    /// # Errors
    /// Rejects invalid selections or unavailable storage.
    pub fn history_evidence(
        &self,
        id: i64,
        child: &str,
    ) -> Result<Option<crate::domain::github::history::HistoryEvidence>, Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        self.history
            .as_ref()
            .ok_or(Error::Storage)?
            .evidence(id, child)
    }

    /// Materialize a selected comparison as immutable, versioned paired evidence.
    /// # Errors
    /// Rejects invalid, stale, unauthorized, unavailable, or conflicting publication.
    pub async fn materialize_history(
        &self,
        id: i64,
        child: &str,
        trace: Option<TraceContext<'_>>,
    ) -> Result<crate::domain::github::history::HistoryEvidence, Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        self.history
            .as_ref()
            .ok_or(Error::Storage)?
            .materialize(
                id,
                child,
                self.credentials.as_deref().ok_or(Error::Credentials)?,
                trace,
                |reference| self.store.expire(reference),
            )
            .await
    }

    /// # Errors
    /// Returns a typed configuration, provider or publication error.
    pub async fn refresh_current_code(
        &self,
        id: i64,
        observer: Option<&dyn Observer>,
        trigger: Option<TraceLink>,
    ) -> Result<current::Refresh, Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        self.current_code
            .as_ref()
            .ok_or(Error::Storage)?
            .refresh(
                id,
                self.credentials.as_deref().ok_or(Error::Credentials)?,
                &*self.provider,
                observer,
                trigger,
            )
            .await
    }

    /// # Errors
    /// Rejects an unavailable or unverified collection.
    pub fn queue_current_code(
        &self,
        id: i64,
        trigger: Option<TraceLink>,
    ) -> Result<(Snapshot, Option<TraceLink>), Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        self.credentials.as_ref().ok_or(Error::Credentials)?;
        let origin = self
            .current_code
            .as_ref()
            .ok_or(Error::Storage)?
            .queue(id, trigger)?;
        Ok((self.snapshot()?, origin))
    }

    /// # Errors
    /// Returns an error if the schedule cannot be read.
    pub fn next_current_code(&self) -> Result<Option<Due>, Error> {
        if self.credentials.is_none() {
            return Ok(None);
        }
        self.current_code.as_ref().ok_or(Error::Storage)?.next()
    }

    pub async fn wait_for_current_code_change(&self) {
        if let Some(collector) = &self.current_code {
            collector.wait_for_change().await;
        } else {
            std::future::pending::<()>().await;
        }
    }

    /// # Errors
    /// Returns a typed error for stale authority or a failed collection attempt.
    pub async fn run_current_code(
        &self,
        due: &Due,
        observer: Option<&dyn Observer>,
    ) -> Result<Published, Error> {
        self.current_code
            .as_ref()
            .ok_or(Error::Storage)?
            .run_due(
                due,
                self.credentials.as_deref().ok_or(Error::Credentials)?,
                &*self.provider,
                observer,
            )
            .await
    }

    fn collection_changed(&self) {
        if let Some(collector) = &self.current_code {
            collector.changed();
        }
    }

    /// # Errors
    /// Returns an error if local GitHub state is unavailable.
    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        let mut state = self.store.snapshot()?;
        state.traffic = self.provider.traffic()?;
        Ok(state)
    }

    /// # Errors
    /// Returns an error if the limit, revision or configuration store is invalid.
    pub fn set_rate(&self, revision: u32, requests_per_minute: u16) -> Result<Snapshot, Error> {
        if !(1..=MAX_REQUESTS_PER_MINUTE).contains(&requests_per_minute) {
            return Err(Error::InvalidRate);
        }
        self.store.set_rate(revision, requests_per_minute)?;
        self.collection_changed();
        self.snapshot()
    }

    /// # Errors
    /// Returns an error if GitHub cannot validate the token or the credential cannot be stored.
    pub async fn authorize(&self, revision: u32, token: &str) -> Result<Snapshot, Error> {
        if token.len() > 512 {
            return Err(Error::InvalidToken);
        }
        let token = token.trim();
        let fine_grained = token.strip_prefix("github_pat_");
        let suffix = fine_grained
            .or_else(|| token.strip_prefix("ghp_"))
            .ok_or(Error::InvalidToken)?;
        if suffix.is_empty()
            || !suffix.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || (fine_grained.is_some() && byte == b'_')
            })
        {
            return Err(Error::InvalidToken);
        }
        let keychain = self.credentials.as_deref().ok_or(Error::Credentials)?;
        self.at_revision(revision)?;
        let login = self
            .provider
            .verify(token.as_bytes())
            .await
            .map_err(|error| {
                if error == Error::Reconnect {
                    Error::InvalidToken
                } else {
                    error
                }
            })?;

        let _guard = CREDENTIAL_OPERATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.sweep_credentials();
        let reference = (self.create_reference)()?;
        if let Err(error) = self.store.stage_credential(&reference) {
            cleanup_warning();
            return Err(error);
        }
        if keychain.save(&reference, token.as_bytes()).is_err() {
            self.sweep_credentials();
            return Err(Error::Credentials);
        }
        if let Err(error) = self.store.bind(revision, Some((&reference, &login))) {
            if error == Error::Storage {
                cleanup_warning();
            }
            self.sweep_credentials();
            return Err(error);
        }
        self.collection_changed();
        self.sweep_credentials();
        self.snapshot()
    }

    /// # Errors
    /// Returns an error if the revision is stale or the stored binding cannot be removed.
    pub fn disconnect(&self, revision: u32) -> Result<Snapshot, Error> {
        let _guard = CREDENTIAL_OPERATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.sweep_credentials();
        let binding = self.at_revision(revision)?;
        if binding.reference.is_some() {
            if let Err(error) = self.store.bind(revision, None) {
                if error == Error::Storage {
                    cleanup_warning();
                }
                return Err(error);
            }
            self.collection_changed();
            self.sweep_credentials();
        }
        self.snapshot()
    }

    /// Retry durable credential cleanup during startup or after an interrupted operation.
    pub fn retry_credential_cleanup(&self) {
        let _guard = CREDENTIAL_OPERATIONS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.sweep_credentials();
    }

    fn sweep_credentials(&self) {
        let Ok(references) = self.store.pending_credentials() else {
            cleanup_warning();
            return;
        };
        let Some(keychain) = self.credentials.as_deref() else {
            if !references.is_empty() {
                cleanup_warning();
            }
            return;
        };
        for reference in references {
            if remove_secret(keychain, &reference)
                && self.store.forget_credential(&reference).is_err()
            {
                cleanup_warning();
            }
        }
    }

    /// # Errors
    /// Returns an error if the requested page or current credential is invalid.
    pub async fn discover(&self, page: u16) -> Result<Discovery, Error> {
        if !(1..=1_000).contains(&page) {
            return Err(Error::InvalidSelection);
        }
        let (reference, secret) = self.secret(self.store.binding()?)?;
        let result = self.provider.discover(&secret, page).await;
        let mut result = self.observe(&reference, result)?;
        let active = self.store.binding()?;
        if active.reference.as_deref() != Some(reference.as_str()) {
            return Err(Error::Conflict);
        }
        if active.reconnect {
            return Err(Error::Reconnect);
        }
        result.traffic = self.provider.traffic()?;
        Ok(result)
    }

    /// # Errors
    /// Returns an error if selection, authorization, or revision cannot be verified.
    pub async fn save(&self, selection: &Selection) -> Result<Snapshot, Error> {
        let name = selection.validate()?;
        let (reference, secret) = self.secret(self.at_revision(selection.revision)?)?;
        let result = self.inspect(&secret, name, selection).await;
        let config = self.observe(&reference, result)?;
        self.store.save(selection.revision, &config)?;
        self.collection_changed();
        self.snapshot()
    }

    async fn inspect(
        &self,
        secret: &[u8],
        name: RepositoryName<'_>,
        selection: &Selection,
    ) -> Result<ConfiguredRepository, Error> {
        let repository = self.provider.repository(secret, name).await?;
        if selection.expected_id.is_some_and(|id| id != repository.id) {
            return Err(Error::Conflict);
        }

        let branch = selection
            .branch
            .clone()
            .unwrap_or_else(|| repository.default_branch.clone());
        let mut contents = Access::Unchecked;
        let mut actions = Access::Unchecked;
        if selection.features.needs_contents() {
            let available = self
                .provider
                .probe_access(secret, &repository, AccessProbe::Contents(&branch))
                .await?;
            contents = Access::from_available(available);
        }
        if selection.features.actions {
            let available = self
                .provider
                .probe_access(secret, &repository, AccessProbe::WorkflowRuns)
                .await?;
            actions = Access::from_available(available && contents == Access::Available);
        }

        Ok(ConfiguredRepository {
            token_setup: TokenSetup::for_repository(&repository.full_name),
            repository,
            branch,
            interval_minutes: selection.interval_minutes,
            features: selection.features,
            contents,
            actions,
            verified: true,
            code_collection: None,
            history_activity: None,
            actions_activity: None,
        })
    }

    /// Recheck the configured repository and branch using the current credential.
    /// # Errors
    /// Rejects unknown repositories, stale revisions and failed provider requests.
    pub async fn check_access(&self, revision: u32, id: i64) -> Result<Snapshot, Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        let saved = self.store.snapshot()?;
        if saved.revision != revision {
            return Err(Error::Conflict);
        }
        let mut config = saved
            .repositories
            .into_iter()
            .find(|entry| entry.repository.id == id)
            .ok_or(Error::Access)?;
        let (reference, secret) = self.secret(self.at_revision(revision)?)?;
        let selection = Selection {
            revision,
            repository: config.repository.full_name.clone(),
            expected_id: Some(id),
            branch: Some(config.branch.clone()),
            interval_minutes: config.interval_minutes,
            features: config.features,
        };
        let name = RepositoryName::parse(&selection.repository)?;
        let result = self.inspect(&secret, name, &selection).await;
        let sso_required = match self.observe(&reference, result) {
            Ok(checked) => {
                config.contents = checked.contents;
                config.actions = checked.actions;
                config.verified = checked.verified;
                false
            }
            Err(error @ (Error::Access | Error::SsoRequired)) => {
                config.contents = if config.features.needs_contents() {
                    Access::Unavailable
                } else {
                    Access::Unchecked
                };
                config.actions = if config.features.actions {
                    Access::Unavailable
                } else {
                    Access::Unchecked
                };
                config.verified = false;
                error == Error::SsoRequired
            }
            Err(Error::Reconnect) => {
                self.collection_changed();
                return Err(Error::Reconnect);
            }
            Err(error) => return Err(error),
        };
        if self.store.record_access(revision, &config)? {
            self.collection_changed();
        }
        if sso_required {
            return Err(Error::SsoRequired);
        }
        self.snapshot()
    }

    /// # Errors
    /// Returns an error if the repository cannot be removed or the revision is stale.
    pub fn remove(&self, revision: u32, id: i64) -> Result<Snapshot, Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        self.store.remove(revision, id)?;
        self.collection_changed();
        self.snapshot()
    }

    fn at_revision(&self, revision: u32) -> Result<Binding, Error> {
        let binding = self.store.binding()?;
        if binding.revision != revision {
            return Err(Error::Conflict);
        }
        Ok(binding)
    }

    fn secret(&self, binding: Binding) -> Result<(String, Vec<u8>), Error> {
        let reference = binding
            .reference
            .filter(|_| !binding.reconnect)
            .ok_or(Error::Reconnect)?;
        let keychain = self.credentials.as_deref().ok_or(Error::Credentials)?;
        match keychain.read(&reference) {
            Ok(secret) => Ok((reference, secret)),
            Err(CredentialError::Missing) => {
                self.store.expire(&reference)?;
                Err(Error::Reconnect)
            }
            Err(_) => Err(Error::Credentials),
        }
    }

    fn observe<T>(&self, reference: &str, result: Result<T, Error>) -> Result<T, Error> {
        if matches!(result, Err(Error::Reconnect)) {
            self.store.expire(reference)?;
        }
        result
    }
}

fn remove_secret(keychain: &dyn CredentialStore, reference: &str) -> bool {
    let removed = matches!(
        keychain.delete(reference),
        Ok(()) | Err(CredentialError::Missing)
    );
    if !removed {
        cleanup_warning();
    }
    removed
}

fn cleanup_warning() {
    tracing::warn!(
        code = "github_keychain_cleanup",
        "A superseded GitHub credential could not be removed"
    );
}
