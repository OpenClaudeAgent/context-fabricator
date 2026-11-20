use std::future::Future;
use std::pin::Pin;

use crate::domain::sync::{
    Admission, JobTransition, Outcome, Page, Reply, RequestKind, Traffic, VersionedJob,
};

use super::Failure;

pub trait PageSource: Send + Sync {
    /// Fetches one logical page. Adapters must send each HTTP request through
    /// the shared quota transport, including discovery, pagination, redirects and retries.
    fn fetch<'a>(&'a self, cursor: Option<&'a str>, kind: RequestKind) -> PageFetch<'a>;
}

pub type PageFetch<'a> = Pin<Box<dyn Future<Output = Result<Reply<Page>, Failure>> + Send + 'a>>;

pub trait CollectionJobs: Send + Sync {
    /// # Errors
    /// Returns a storage error when the job cannot be read.
    fn load_job(&self, collection: &str) -> Result<Option<VersionedJob>, Failure>;

    /// Applies a transition only for the current collection generation. Completion requires
    /// a running job. A credential denial updates access and the job in the same transaction.
    /// Returns the resulting generation, or `None` if the transition is stale.
    ///
    /// # Errors
    /// Returns a storage error when the state cannot be saved.
    fn transition_job(
        &self,
        collection: &str,
        generation: u32,
        transition: JobTransition,
    ) -> Result<Option<u32>, Failure>;
}

pub trait TrafficLedger: Send + Sync {
    /// Reserves a transport handoff and persists the next eligible time atomically.
    ///
    /// # Errors
    /// Returns a storage error and the runtime must withhold transport.
    fn reserve(&self, bucket: &str, now: i64, spacing: i64) -> Result<Admission, Failure>;

    /// # Errors
    /// Returns a storage error when the response cannot be accounted for.
    fn complete(&self, bucket: &str, outcome: Outcome, cooldown_until: i64) -> Result<(), Failure>;

    /// # Errors
    /// Returns a storage error if the provider cooldown cannot be saved.
    fn defer(&self, bucket: &str, cooldown_until: i64) -> Result<(), Failure>;

    /// # Errors
    /// Returns a storage error when the accounting snapshot cannot be read.
    fn traffic(&self, bucket: &str) -> Result<Traffic, Failure>;
}

pub trait SyncStorage: super::ports::CorpusStore + CollectionJobs {}

impl<T: super::ports::CorpusStore + CollectionJobs> SyncStorage for T {}
