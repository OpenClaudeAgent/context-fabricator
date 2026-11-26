pub mod actions;
pub mod branches;
use serde::{Deserialize, Serialize};

pub mod current;
pub mod graph;
pub mod history;
pub mod source;

pub const BUCKET: &str = "github";
pub const MAX_REQUESTS_PER_MINUTE: u16 = 60;
pub const CLASSIC_TOKEN_URL: &str =
    "https://github.com/settings/tokens/new?description=Context+Fabricator&scopes=repo";
pub const CLASSIC_TOKEN_SETTINGS: &str = "https://github.com/settings/tokens";
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

impl Access {
    pub(crate) const fn from_available(available: bool) -> Self {
        if available {
            Self::Available
        } else {
            Self::Unavailable
        }
    }
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
    pub token_setup: Option<TokenSetup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_collection: Option<current::CollectionStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_activity: Option<CollectionActivity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actions_activity: Option<CollectionActivity>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenSetup {
    authorization_url: String,
    actions_authorization_url: String,
}

impl TokenSetup {
    pub(crate) fn for_repository(full_name: &str) -> Option<Self> {
        let owner = RepositoryName::parse(full_name).ok()?.owner;
        Some(Self {
            authorization_url: format!("{TOKEN_URL}&target_name={owner}"),
            actions_authorization_url: format!("{ACTIONS_TOKEN_URL}&target_name={owner}"),
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionActivity {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_attempt: Option<super::telemetry::TraceLink>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_snapshot: Option<super::telemetry::TraceLink>,
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
    pub classic_authorization_url: &'static str,
    pub classic_settings_url: &'static str,
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
    pub refresh_interval: RefreshInterval,
}

const MIN_REFRESH_INTERVAL_MINUTES: u16 = 15;
const MAX_REFRESH_INTERVAL_MINUTES: u16 = 1_440;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshInterval {
    minimum_minutes: u16,
    maximum_minutes: u16,
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
            refresh_interval: RefreshInterval {
                minimum_minutes: MIN_REFRESH_INTERVAL_MINUTES,
                maximum_minutes: MAX_REFRESH_INTERVAL_MINUTES,
            },
        }
    }
}

#[derive(serde::Serialize)]
pub struct DataState {
    pub paused: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageBudget {
    pub revision: u32,
    pub limit_gb: u16,
    pub bytes: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum Error {
    #[error("Choose a GitHub request limit between 1 and 60 requests per minute.")]
    InvalidRate,
    #[error(
        "Choose a refresh interval between {MIN_REFRESH_INTERVAL_MINUTES} and {MAX_REFRESH_INTERVAL_MINUTES} minutes."
    )]
    InvalidInterval,
    #[error(
        "Enter a GitHub repository as owner/repo or a github.com repository URL, and check the branch name."
    )]
    InvalidSelection,
    #[error("Enter a valid GitHub personal access token (classic or fine-grained).")]
    InvalidToken,
    #[error("Connect GitHub with a valid token before selecting repositories.")]
    Reconnect,
    #[error(
        "GitHub could not verify repository access. For a classic token, check the repo scope and your organization’s token policy. For a fine-grained token, select the repository’s owner and include the repository."
    )]
    Access,
    #[error(
        "GitHub requires SSO authorization for this organization. In your classic token settings, use Configure SSO to authorize the token for the organization, then try again."
    )]
    SsoRequired,
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
    #[error("Choose a GitHub storage limit between 1 and 1000 GB.")]
    InvalidStorageLimit,
    #[error(
        "The GitHub logical storage limit has been reached. Increase the limit or remove retained data before retrying."
    )]
    StorageLimit,
    #[error("The local GitHub settings store is unavailable.")]
    Storage,
    #[cfg_attr(
        target_os = "linux",
        error(
            "The credential store is unavailable. Start or unlock your Secret Service keyring and try again."
        )
    )]
    #[cfg_attr(
        not(target_os = "linux"),
        error("The credential store is unavailable. Unlock the macOS Keychain and try again.")
    )]
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
        if !(MIN_REFRESH_INTERVAL_MINUTES..=MAX_REFRESH_INTERVAL_MINUTES)
            .contains(&self.interval_minutes)
        {
            return Err(Error::InvalidInterval);
        }
        if self.repository.len() > 240
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub next_at_ms: i64,
    pub last_checked_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScheduleDue {
    pub repository_id: i64,
    pub generation: i64,
    pub at_ms: i64,
    pub interval_minutes: i64,
}

impl Error {
    pub(crate) const fn schedule_code(self) -> &'static str {
        match self {
            Self::Throttled => "throttled",
            Self::Reconnect | Self::Credentials => "credentials",
            Self::Access => "access",
            Self::Conflict => "staleScope",
            Self::CollectionLimit => "collectionLimit",
            Self::StorageLimit => "storageLimit",
            _ => "unavailable",
        }
    }
}
