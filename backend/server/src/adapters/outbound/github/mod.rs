//! Read-only GitHub adapter. REST and Git requests share the provider quota bucket.

mod actions;
mod branches;
mod current;
mod git;
mod history;
mod objects;
mod request;

use std::sync::Arc;
use std::{io, path::Path};

use hyper::http::uri::Authority;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

use crate::application::github::{
    AccessProbe, GitHubFuture, MetadataRequest, Provider, current::Store,
};
use crate::domain::github::{
    Discovery, Error, MAX_REQUESTS_PER_MINUTE, Repository, RepositoryName, TrafficStatus,
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
    git_snapshots: Option<git::Cache>,
}

impl GitHubRest {
    /// # Errors
    /// Returns an error if the HTTP adapter or private Git workspace cannot be initialized.
    pub fn production(gate: Arc<OutboundGate>, data_directory: &Path) -> io::Result<Self> {
        Ok(Self {
            transport: HttpTransport::new(gate).map_err(io::Error::other)?,
            authority: Authority::from_static("api.github.com"),
            scheme: "https",
            scope_authority: None,
            git_snapshots: Some(git::Cache::open(data_directory)?),
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
            git_snapshots: None,
        })
    }

    #[cfg(debug_assertions)]
    #[must_use]
    pub fn with_git_fixture(mut self) -> Self {
        self.git_snapshots = Some(git::Cache::default());
        self
    }

    #[must_use]
    pub fn with_scope_authority(mut self, store: Arc<dyn Store>) -> Self {
        self.scope_authority = Some(store);
        self
    }
}

impl Provider for GitHubRest {
    fn public_actions<'a>(
        &'a self,
        repository: &'a str,
        revision: &'a str,
    ) -> GitHubFuture<'a, Vec<crate::domain::github::actions::Snapshot>> {
        Box::pin(self.actions_for_commit(repository, revision))
    }

    fn public_snapshot<'a>(
        &'a self,
        name: RepositoryName<'a>,
        revision: &'a str,
    ) -> GitHubFuture<'a, Arc<dyn crate::application::github::current::Objects>> {
        Box::pin(git::public_snapshot(self, name, revision))
    }

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

    fn branches<'a>(
        &'a self,
        source: MetadataRequest<'a>,
        name: RepositoryName<'a>,
        page: u16,
    ) -> GitHubFuture<'a, crate::domain::github::branches::Page> {
        Box::pin(self.list_branches(source, name, page))
    }

    fn repository<'a>(
        &'a self,
        secret: &'a [u8],
        name: RepositoryName<'a>,
    ) -> GitHubFuture<'a, Repository> {
        Box::pin(async move {
            let path = format!("/repos/{}/{}", name.owner, name.name);
            let raw: GitHubRepository = self
                .get(secret, RequestKind::Validation, &path, json)
                .await?;
            raw.validated()
        })
    }

    fn probe_access<'a>(
        &'a self,
        secret: &'a [u8],
        repository: &'a Repository,
        probe: AccessProbe<'a>,
    ) -> GitHubFuture<'a, bool> {
        Box::pin(async move {
            let repository = &repository.full_name;
            let path = match probe {
                AccessProbe::Contents(branch) => {
                    let encoded = utf8_percent_encode(branch, NON_ALPHANUMERIC);
                    format!("/repos/{repository}/branches/{encoded}")
                }
                AccessProbe::WorkflowRuns => format!("/repos/{repository}/actions/runs?per_page=1"),
            };
            match self
                .get(secret, RequestKind::Validation, &path, skip_body)
                .await
            {
                Ok(()) => Ok(true),
                Err(Error::Access) => Ok(false),
                Err(error) => Err(error),
            }
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
