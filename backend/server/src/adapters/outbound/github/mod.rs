//! Read-only GitHub REST adapter. All requests use the provider quota bucket.

mod current;
mod request;

use std::sync::Arc;

use hyper::http::uri::Authority;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

use crate::application::github::{GitHubFuture, Provider, current::Store};
use crate::domain::github::{
    Access, ConfiguredRepository, Discovery, Error, MAX_REQUESTS_PER_MINUTE, Repository,
    RepositoryName, Selection, TrafficStatus,
};
use crate::domain::sync::RequestKind;
use crate::runtime::outbound::OutboundGate;

use super::http::HttpTransport;
use request::{json, read_page, skip_body};

pub use crate::domain::github::BUCKET;

pub struct GitHubRest {
    transport: HttpTransport,
    authority: Authority,
    scheme: &'static str,
    scope_authority: Option<Arc<dyn Store>>,
}

impl GitHubRest {
    /// # Errors
    /// Returns an error if the HTTP adapter cannot be initialized.
    pub fn production(gate: Arc<OutboundGate>) -> Result<Self, native_tls::Error> {
        Ok(Self {
            transport: HttpTransport::new(gate)?,
            authority: Authority::from_static("api.github.com"),
            scheme: "https",
            scope_authority: None,
        })
    }

    /// # Errors
    /// Returns an error if the loopback fixture cannot be initialized.
    #[cfg(debug_assertions)]
    pub fn fixture(gate: Arc<OutboundGate>, port: u16) -> Result<Self, Error> {
        Ok(Self {
            transport: HttpTransport::new(gate).map_err(|_| Error::Upstream)?,
            authority: format!("127.0.0.1:{port}")
                .parse()
                .map_err(|_| Error::InvalidSelection)?,
            scheme: "http",
            scope_authority: None,
        })
    }

    #[must_use]
    pub fn with_scope_authority(mut self, store: Arc<dyn Store>) -> Self {
        self.scope_authority = Some(store);
        self
    }

    async fn probe(&self, secret: &[u8], path: &str) -> Result<Access, Error> {
        match self
            .get(secret, RequestKind::Validation, path, skip_body)
            .await
        {
            Ok(()) => Ok(Access::Available),
            Err(Error::Access) => Ok(Access::Unavailable),
            Err(error) => Err(error),
        }
    }
}

impl Provider for GitHubRest {
    fn traffic(&self) -> Result<Option<TrafficStatus>, Error> {
        let gate = self.transport.status().map_err(|_| Error::Storage)?;
        Ok(Some(TrafficStatus {
            attempts: gate.traffic.attempts,
            failures: gate.traffic.failures,
            pending: gate.traffic.pending,
            uncertain: gate.traffic.uncertain,
            queued: gate.queued,
            concurrency: gate.concurrency,
            effective_requests_per_minute: gate.requests_per_minute,
            maximum_configurable_requests_per_minute: MAX_REQUESTS_PER_MINUTE,
            requested_requests_per_minute: gate.traffic.requests_per_minute,
            cooldown_until_ms: gate.cooldown_until_ms,
        }))
    }

    fn verify<'a>(&'a self, secret: &'a [u8]) -> GitHubFuture<'a, String> {
        Box::pin(async move {
            #[derive(Deserialize)]
            struct Account {
                login: String,
            }
            let account: Account = self
                .get(secret, RequestKind::Validation, "/user", json)
                .await?;
            if account.login.is_empty() {
                return Err(Error::Upstream);
            }
            Ok(account.login)
        })
    }

    fn discover<'a>(&'a self, secret: &'a [u8], page: u16) -> GitHubFuture<'a, Discovery> {
        Box::pin(async move {
            let path = format!("/user/repos?per_page=30&sort=full_name&page={page}");
            let kind = if page == 1 {
                RequestKind::Validation
            } else {
                RequestKind::Pagination
            };
            let (repos, next) = self.get(secret, kind, &path, read_page).await?;
            let repositories = repos
                .into_iter()
                .map(GitHubRepository::validated)
                .collect::<Result<_, _>>()?;
            Ok(Discovery {
                repositories,
                next_page: (next && page < 1_000).then(|| page + 1),
                traffic: None,
            })
        })
    }

    fn inspect<'a>(
        &'a self,
        secret: &'a [u8],
        name: RepositoryName<'a>,
        selection: &'a Selection,
    ) -> GitHubFuture<'a, ConfiguredRepository> {
        Box::pin(async move {
            let path = format!("/repos/{}/{}", name.owner, name.name);
            let raw: GitHubRepository = self
                .get(secret, RequestKind::Validation, &path, json)
                .await?;
            let repository = raw.validated()?;
            if selection.expected_id.is_some_and(|id| id != repository.id) {
                return Err(Error::Conflict);
            }
            let branch = selection
                .branch
                .clone()
                .unwrap_or_else(|| repository.default_branch.clone());
            let path = format!("/repos/{}", repository.full_name);
            let mut contents = Access::Unchecked;
            let mut actions = Access::Unchecked;
            if selection.features.needs_contents() {
                let encoded = utf8_percent_encode(&branch, NON_ALPHANUMERIC);
                contents = self
                    .probe(secret, &format!("{path}/branches/{encoded}"))
                    .await?;
            }
            if selection.features.actions {
                let runs = self
                    .probe(secret, &format!("{path}/actions/runs?per_page=1"))
                    .await?;
                actions = if runs == Access::Available && contents == Access::Available {
                    Access::Available
                } else {
                    Access::Unavailable
                };
            }
            Ok(ConfiguredRepository {
                repository,
                branch,
                interval_minutes: selection.interval_minutes,
                features: selection.features,
                contents,
                actions,
                verified: true,
                code_collection: None,
            })
        })
    }
}

#[derive(Deserialize)]
struct GitHubRepository {
    id: i64,
    full_name: String,
    default_branch: String,
    private: bool,
}

impl GitHubRepository {
    fn validated(self) -> Result<Repository, Error> {
        let name = RepositoryName::parse(&self.full_name).map_err(|_| Error::Upstream)?;
        if self.full_name != format!("{}/{}", name.owner, name.name)
            || self.id <= 0
            || self.default_branch.is_empty()
            || self.default_branch.len() > 250
        {
            return Err(Error::Upstream);
        }
        Ok(Repository {
            id: self.id,
            full_name: self.full_name,
            default_branch: self.default_branch,
            private: self.private,
        })
    }
}
