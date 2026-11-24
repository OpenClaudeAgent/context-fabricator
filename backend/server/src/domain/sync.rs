use std::time::Duration;

use super::corpus::Record;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestKind {
    Manual,
    Scheduled,
    Validation,
    Pagination,
    Retry,
    Redirect,
    ModelDownload,
    ModelCatalog,
}

#[derive(Debug)]
pub enum Reply<T> {
    Success(T),
    Throttled(Duration),
    Credentials,
    ExpiredCursor,
    Temporary,
    Permanent,
}

pub struct Page {
    pub cursor: String,
    pub documents: Vec<Record<String>>,
    pub more: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Traffic {
    pub attempts: i64,
    pub pending: i64,
    pub uncertain: i64,
    pub failures: i64,
    pub window_start: i64,
    pub window_attempts: i64,
    pub cooldown_until: i64,
    pub requests_per_minute: Option<u16>,
}

pub(crate) fn minute_spacing(requests: u16) -> i64 {
    let requests = i64::from(requests.max(1));
    (60_000 + requests - 1) / requests
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admission {
    Granted,
    WaitUntil(i64),
}

pub const UNCERTAIN_COOLDOWN_MS: i64 = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Succeeded,
    Failed,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Queued,
    Running,
    Pagination,
    Succeeded,
    Retry,
    Throttled,
    Credentials,
    ExpiredCursor,
    Failed,
}

impl Phase {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Pagination => "pagination",
            Self::Succeeded => "succeeded",
            Self::Retry => "retry",
            Self::Throttled => "throttled",
            Self::Credentials => "credentials",
            Self::ExpiredCursor => "expired_cursor",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn from_code(value: &str) -> Option<Self> {
        Some(match value {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "pagination" => Self::Pagination,
            "succeeded" => Self::Succeeded,
            "retry" => Self::Retry,
            "throttled" => Self::Throttled,
            "credentials" => Self::Credentials,
            "expired_cursor" => Self::ExpiredCursor,
            "failed" => Self::Failed,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn diagnostic(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Succeeded => Some(("sync_succeeded", "Collection checkpoint is searchable.")),
            Self::Retry => Some((
                "sync_retry",
                "Temporary collection failure. A retry is scheduled.",
            )),
            Self::Throttled => Some((
                "sync_throttled",
                "Collection is waiting for the provider cooldown.",
            )),
            Self::Credentials => Some((
                "sync_credentials",
                "Update provider credentials to resume collection.",
            )),
            Self::ExpiredCursor => Some((
                "sync_expired_cursor",
                "The checkpoint expired. Reconcile this scope to resume.",
            )),
            Self::Failed => Some((
                "sync_failed",
                "Collection stopped. Check the source before retrying.",
            )),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JobState {
    pub phase: Phase,
    pub due_at: Option<i64>,
    pub failures: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VersionedJob {
    pub generation: u32,
    pub state: JobState,
}

#[derive(Clone, Copy, Debug)]
pub enum JobTransition {
    Queue(i64),
    Start { now: i64, failures: u8 },
    Finish(JobState),
}

impl JobState {
    #[must_use]
    pub fn after_failure(now: i64, failures: u8, jitter_ms: u8) -> Self {
        if failures >= 4 {
            return Self::blocked(Phase::Failed);
        }
        let backoff_ms = 1_000_i64 << failures;
        Self {
            phase: Phase::Retry,
            due_at: Some(now.saturating_add(backoff_ms + i64::from(jitter_ms))),
            failures: failures + 1,
        }
    }

    #[must_use]
    pub const fn request(self, manual: bool, now: i64) -> Option<RequestKind> {
        if manual && matches!(self.phase, Phase::Queued | Phase::Succeeded | Phase::Failed) {
            return Some(RequestKind::Manual);
        }
        if manual || !matches!(self.due_at, Some(due) if due <= now) {
            return None;
        }
        Some(match self.phase {
            Phase::Pagination => RequestKind::Pagination,
            Phase::Running | Phase::Retry | Phase::Throttled => RequestKind::Retry,
            _ => RequestKind::Scheduled,
        })
    }

    #[must_use]
    pub const fn due(phase: Phase, due_at: i64) -> Self {
        Self {
            phase,
            due_at: Some(due_at),
            failures: 0,
        }
    }

    #[must_use]
    pub const fn blocked(phase: Phase) -> Self {
        Self {
            phase,
            due_at: None,
            failures: 0,
        }
    }
}
