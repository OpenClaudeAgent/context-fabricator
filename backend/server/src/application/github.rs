//! GitHub onboarding and repository configuration, independent of HTTP and Keychain.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::domain::github::{
    ConfiguredRepository, Discovery, Error, MAX_REQUESTS_PER_MINUTE, RepositoryName, Selection,
    Snapshot, TrafficStatus,
};

use super::credentials::{CredentialError, CredentialStore};
use super::telemetry::Observer;
use crate::domain::github::current::{Due, Published};
use current::Collector;

pub mod current;

pub type GitHubFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

pub trait Provider: Send + Sync {
    /// # Errors
    /// Returns an error if the outbound accounting store is unavailable.
    fn traffic(&self) -> Result<Option<TrafficStatus>, Error> {
        Ok(None)
    }
    fn verify<'a>(&'a self, secret: &'a [u8]) -> GitHubFuture<'a, String>;
    fn discover<'a>(&'a self, secret: &'a [u8], page: u16) -> GitHubFuture<'a, Discovery>;
    fn inspect<'a>(
        &'a self,
        secret: &'a [u8],
        name: RepositoryName<'a>,
        selection: &'a Selection,
    ) -> GitHubFuture<'a, ConfiguredRepository>;
}

pub struct Binding {
    pub revision: u32,
    pub reference: Option<String>,
    pub reconnect: bool,
}

pub trait Store: Send + Sync {
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
    /// # Errors
    /// Returns a conflict for a stale revision or an error if persistence fails.
    fn remove(&self, revision: u32, id: i64) -> Result<(), Error>;
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
}

impl GitHub {
    #[must_use]
    pub fn new(
        provider: Arc<dyn Provider>,
        store: Arc<dyn Store>,
        credentials: Option<Arc<dyn CredentialStore + Send + Sync>>,
        create_reference: fn() -> Result<String, Error>,
    ) -> Self {
        Self {
            provider,
            store,
            credentials,
            create_reference,
            current_code: None,
        }
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

    /// # Errors
    /// Returns a typed configuration, provider or publication error.
    pub async fn refresh_current_code(&self, id: i64) -> Result<Published, Error> {
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
            )
            .await
    }

    /// # Errors
    /// Rejects an unavailable or unverified collection.
    pub fn queue_current_code(&self, id: i64) -> Result<Snapshot, Error> {
        if id <= 0 {
            return Err(Error::InvalidSelection);
        }
        self.credentials.as_ref().ok_or(Error::Credentials)?;
        self.current_code
            .as_ref()
            .ok_or(Error::Storage)?
            .queue(id)?;
        self.snapshot()
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
        if !token.strip_prefix("github_pat_").is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        }) {
            return Err(Error::InvalidToken);
        }
        let keychain = self.credentials.as_deref().ok_or(Error::Credentials)?;
        let previous = self.at_revision(revision)?;
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

        let reference = (self.create_reference)()?;
        keychain
            .save(&reference, token.as_bytes())
            .map_err(|_| Error::Credentials)?;
        if let Err(error) = self.store.bind(revision, Some((&reference, &login))) {
            remove_secret(keychain, &reference);
            return Err(error);
        }
        self.collection_changed();
        if let Some(previous) = previous.reference {
            remove_secret(keychain, &previous);
        }
        self.snapshot()
    }

    /// # Errors
    /// Returns an error if the revision is stale or the stored binding cannot be removed.
    pub fn disconnect(&self, revision: u32) -> Result<Snapshot, Error> {
        let binding = self.at_revision(revision)?;
        if let Some(reference) = binding.reference {
            let keychain = self.credentials.as_deref().ok_or(Error::Credentials)?;
            self.store.bind(revision, None)?;
            self.collection_changed();
            remove_secret(keychain, &reference);
        }
        self.snapshot()
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
        let result = self.provider.inspect(&secret, name, selection).await;
        let config = self.observe(&reference, result)?;
        self.store.save(selection.revision, &config)?;
        self.collection_changed();
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

fn remove_secret(keychain: &dyn CredentialStore, reference: &str) {
    if !matches!(
        keychain.delete(reference),
        Ok(()) | Err(CredentialError::Missing)
    ) {
        tracing::warn!(
            code = "github_keychain_cleanup",
            "A superseded GitHub credential could not be removed"
        );
    }
}
