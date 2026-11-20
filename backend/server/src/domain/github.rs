use serde::{Deserialize, Serialize};

pub mod current;

pub const BUCKET: &str = "github";
pub const MAX_REQUESTS_PER_MINUTE: u16 = 60;
pub const TOKEN_URL: &str = "https://github.com/settings/personal-access-tokens/new?name=Context+Fabricator&expires_in=30&contents=read";
pub const ACTIONS_TOKEN_URL: &str = "https://github.com/settings/personal-access-tokens/new?name=Context+Fabricator&expires_in=30&contents=read&actions=read";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub id: i64,
    pub full_name: String,
    pub default_branch: String,
    pub private: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Four independent toggles in the OpenAPI settings contract"
)]
pub struct Features {
    pub current_code: bool,
    pub history: bool,
    pub actions: bool,
    pub code_graph: bool,
}

impl Features {
    #[must_use]
    pub const fn needs_contents(self) -> bool {
        self.current_code || self.history || self.actions || self.code_graph
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub revision: u32,
    pub repository: String,
    pub expected_id: Option<i64>,
    pub branch: Option<String>,
    pub interval_minutes: u16,
    pub features: Features,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Access {
    Unchecked,
    Available,
    Unavailable,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfiguredRepository {
    pub repository: Repository,
    pub branch: String,
    pub interval_minutes: u16,
    pub features: Features,
    pub contents: Access,
    pub actions: Access,
    pub verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_collection: Option<current::CollectionStatus>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Discovery {
    pub repositories: Vec<Repository>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic: Option<TrafficStatus>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CredentialState {
    Unconfigured,
    Connected,
    ReconnectRequired,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub revision: u32,
    pub credential: CredentialState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
    pub authorization_url: &'static str,
    pub actions_authorization_url: &'static str,
    pub defaults: Defaults,
    pub repositories: Vec<ConfiguredRepository>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic: Option<TrafficStatus>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficStatus {
    pub attempts: i64,
    pub failures: i64,
    pub pending: i64,
    pub uncertain: i64,
    pub queued: usize,
    pub concurrency: u16,
    pub effective_requests_per_minute: i64,
    pub maximum_configurable_requests_per_minute: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_requests_per_minute: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_until_ms: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Defaults {
    pub interval_minutes: u16,
    pub features: Features,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            interval_minutes: 60,
            features: Features {
                current_code: true,
                history: false,
                actions: false,
                code_graph: false,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("Choose a GitHub request limit between 1 and 60 requests per minute.")]
    InvalidRate,
    #[error(
        "Enter a GitHub repository as owner/repo or a github.com repository URL. Check the branch and sync interval."
    )]
    InvalidSelection,
    #[error("Enter a fine-grained GitHub personal access token.")]
    InvalidToken,
    #[error("Connect GitHub with a valid token before selecting repositories.")]
    Reconnect,
    #[error(
        "GitHub could not verify access to this repository or account. Check the repository selection and organization approval."
    )]
    Access,
    #[error("GitHub is enforcing a request limit. Wait before trying again.")]
    Throttled,
    #[error("The GitHub API is temporarily unavailable.")]
    Upstream,
    #[error("The current GitHub snapshot exceeds the supported collection limits.")]
    CollectionLimit,
    #[error(
        "GitHub no longer serves an object in the pending snapshot. Refresh will resolve the current branch again."
    )]
    ExpiredPin,
    #[error("GitHub settings changed. Refresh and try again.")]
    Conflict,
    #[error("The local GitHub settings store is unavailable.")]
    Storage,
    #[error("The credential store is unavailable. Unlock the macOS Keychain and try again.")]
    Credentials,
}

pub struct RepositoryName<'a> {
    pub owner: &'a str,
    pub name: &'a str,
}

impl<'a> RepositoryName<'a> {
    /// # Errors
    /// Rejects invalid paths and URLs outside the GitHub repository namespace.
    pub fn parse(input: &'a str) -> Result<Self, Error> {
        let input = input.trim();
        let input = input.strip_prefix("https://github.com/").unwrap_or(input);
        let input = input.strip_suffix('/').unwrap_or(input);
        let input = input.strip_suffix(".git").unwrap_or(input);
        let (owner, name) = input.split_once('/').ok_or(Error::InvalidSelection)?;
        if owner.is_empty()
            || owner.len() > 39
            || !owner
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
            || name.is_empty()
            || name.len() > 100
            || matches!(name, "." | "..")
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        {
            return Err(Error::InvalidSelection);
        }
        Ok(Self { owner, name })
    }
}

impl Selection {
    /// # Errors
    /// Rejects an unsupported repository, branch, interval, or identity.
    pub fn validate(&self) -> Result<RepositoryName<'_>, Error> {
        if self.repository.len() > 240
            || !(15..=1_440).contains(&self.interval_minutes)
            || self.expected_id.is_some_and(|id| id <= 0)
            || self
                .branch
                .as_deref()
                .is_some_and(|branch| !valid_branch(branch))
        {
            return Err(Error::InvalidSelection);
        }
        RepositoryName::parse(&self.repository)
    }
}

pub(crate) fn valid_branch(branch: &str) -> bool {
    !branch.is_empty()
        && branch.len() <= 250
        && !branch
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}
