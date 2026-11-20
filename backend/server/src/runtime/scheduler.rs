//! App-session scheduler with one worker per registered collection.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::sleep;

use super::clock::{Clock, milliseconds};
use crate::application::telemetry::{Observer, Stage, TraceLink};
use crate::application::{
    Failure,
    sync::{PageSource, SyncStorage},
};
use crate::domain::corpus::{Batch, Checkpoint, CollectionAccess, Commit, Record};
use crate::domain::sync::{JobState, JobTransition, Page, Phase, Reply, RequestKind};

mod github;

#[derive(Clone)]
pub struct Refresh(mpsc::Sender<Option<TraceLink>>);

impl Refresh {
    /// Returns true when this request occupies the only pending manual slot.
    #[must_use]
    pub fn request(&self) -> bool {
        self.request_traced(None)
    }

    #[must_use]
    pub fn request_traced(&self, parent: Option<TraceLink>) -> bool {
        self.0.try_send(parent).is_ok()
    }
}

pub struct Scheduler {
    stop: watch::Sender<bool>,
    triggers: HashMap<String, Refresh>,
    workers: JoinSet<()>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self {
            stop: watch::channel(false).0,
            triggers: HashMap::new(),
            workers: JoinSet::new(),
        }
    }
}

impl Scheduler {
    #[must_use]
    pub fn stop_signal(&self) -> watch::Sender<bool> {
        self.stop.clone()
    }

    #[must_use]
    pub fn register(&mut self, job: CollectionJob) -> Refresh {
        let mut stop = self.stop.subscribe();
        self.triggers
            .entry(job.collection.clone())
            .or_insert_with(|| {
                let (refresh, mut requests) = mpsc::channel(1);
                self.workers.spawn(async move {
                    tokio::select! {
                        biased;
                        _ = stop.wait_for(|stopping| *stopping) => {}
                        () = job.supervise(&mut requests) => {}
                    }
                });
                Refresh(refresh)
            })
            .clone()
    }

    pub async fn stop(mut self) {
        self.stop.send_replace(true);
        while let Some(result) = self.workers.join_next().await {
            if result.is_err() {
                tracing::warn!(
                    code = "sync_worker_stopped",
                    "A collection worker stopped unexpectedly"
                );
            }
        }
    }
}

pub struct CollectionJob {
    collection: String,
    source: Arc<dyn PageSource>,
    store: Arc<dyn SyncStorage>,
    clock: Clock,
    cadence_ms: i64,
    observer: Option<Arc<dyn Observer>>,
}

impl CollectionJob {
    #[must_use]
    pub fn new(
        collection: String,
        source: Arc<dyn PageSource>,
        store: Arc<dyn SyncStorage>,
        clock: Clock,
        cadence: Duration,
    ) -> Self {
        Self {
            collection,
            source,
            store,
            clock,
            cadence_ms: milliseconds(cadence).max(1_000),
            observer: None,
        }
    }

    #[must_use]
    pub fn with_observer(mut self, observer: Option<Arc<dyn Observer>>) -> Self {
        self.observer = observer;
        self
    }

    async fn supervise(&self, refresh: &mut mpsc::Receiver<Option<TraceLink>>) {
        loop {
            if self.run(refresh).await.is_err() {
                tracing::warn!(
                    code = "sync_storage_unavailable",
                    "Collection is waiting for local storage"
                );
            }
            sleep(Duration::from_secs(1)).await;
        }
    }

    async fn run(&self, refresh: &mut mpsc::Receiver<Option<TraceLink>>) -> Result<(), Failure> {
        let mut deferred = None;
        loop {
            let (_, state) = self.current()?;
            let request = if deferred.is_some() {
                deferred.take()
            } else if let Some(due) = state.due_at {
                tokio::select! {
                    biased;
                    () = self.clock.wait_until(due) => None,
                    trigger = refresh.recv() => trigger,
                }
            } else {
                refresh.recv().await
            };
            let (checkpoint, state) = self.current()?;
            let now = self.clock.now();
            let Some(kind) = state.request(request.is_some(), now) else {
                continue;
            };
            let transition = JobTransition::Start {
                now,
                failures: state.failures,
            };
            if self
                .store
                .transition_job(&self.collection, checkpoint.generation, transition)?
                .is_none()
            {
                deferred = request;
                continue;
            }
            let result = self
                .collect(&checkpoint, kind, state.failures, request.flatten())
                .await?;
            let settled = self.store.transition_job(
                &self.collection,
                checkpoint.generation,
                JobTransition::Finish(result),
            )?;
            if let Ok(pending) = refresh.try_recv()
                && settled != Some(self.current()?.0.generation)
            {
                // A queued refresh for a replacement scope cannot share the old result.
                deferred = Some(pending);
            }
        }
    }

    fn current(&self) -> Result<(Checkpoint, JobState), Failure> {
        let checkpoint = self
            .store
            .checkpoint(&self.collection)?
            .unwrap_or(Checkpoint {
                generation: 0,
                cursor: None,
                access: CollectionAccess::Available,
            });
        let state = if checkpoint.access == CollectionAccess::Lost {
            JobState::blocked(Phase::Credentials)
        } else {
            self.store
                .load_job(&self.collection)?
                .filter(|job| job.generation == checkpoint.generation)
                .map_or_else(
                    || JobState::due(Phase::Queued, self.clock.now()),
                    |job| job.state,
                )
        };
        Ok((checkpoint, state))
    }

    async fn collect(
        &self,
        checkpoint: &Checkpoint,
        kind: RequestKind,
        failures: u8,
        parent: Option<TraceLink>,
    ) -> Result<JobState, Failure> {
        let span = self
            .observer
            .as_ref()
            .map(|observer| observer.start(Stage::Collection, parent));
        let link = span.as_ref().map(|span| span.link());
        let reply = match self.source.fetch(checkpoint.cursor.as_deref(), kind).await {
            Ok(reply) => reply,
            Err(error) => {
                if let Some(span) = span {
                    span.finish(false);
                }
                return Err(error);
            }
        };

        let state = match reply {
            Reply::Success(page) => {
                let index = self
                    .observer
                    .as_ref()
                    .map(|observer| observer.start(Stage::Index, link));
                let indexed = self.index(checkpoint, &page, failures);
                if let Some(index) = index {
                    index.finish(matches!(
                        indexed.phase,
                        Phase::Succeeded | Phase::Pagination
                    ));
                }
                indexed
            }
            Reply::Throttled(delay) => JobState::due(
                Phase::Throttled,
                self.clock
                    .now()
                    .saturating_add(milliseconds(delay.max(Duration::from_secs(1)))),
            ),
            Reply::Credentials => JobState::blocked(Phase::Credentials),
            Reply::ExpiredCursor => JobState::blocked(Phase::ExpiredCursor),
            Reply::Temporary => self.retry(failures),
            Reply::Permanent => JobState::blocked(Phase::Failed),
        };
        if let Some(span) = span {
            span.finish(matches!(state.phase, Phase::Succeeded | Phase::Pagination));
        }
        Ok(state)
    }

    fn index(&self, checkpoint: &Checkpoint, page: &Page, failures: u8) -> JobState {
        if checkpoint.cursor.as_deref() == Some(page.cursor.as_str())
            && (page.more || !page.documents.is_empty())
        {
            return JobState::blocked(Phase::Failed);
        }
        let documents: Vec<_> = page.documents.iter().map(Record::borrowed).collect();
        let committed = self.store.commit_batch(&Batch {
            collection: &self.collection,
            generation: checkpoint.generation,
            previous_cursor: checkpoint.cursor.as_deref(),
            next_cursor: &page.cursor,
            documents: &documents,
        });
        let now = self.clock.now();
        match committed {
            Ok(Commit::Indexed | Commit::AlreadyIndexed) if !page.more => {
                JobState::due(Phase::Succeeded, now.saturating_add(self.cadence_ms))
            }
            Ok(Commit::Indexed) => JobState::due(Phase::Pagination, now),
            Ok(Commit::AlreadyIndexed | Commit::InvalidProvenance) => {
                JobState::blocked(Phase::Failed)
            }
            Ok(Commit::AccessLost) => JobState::blocked(Phase::Credentials),
            Ok(Commit::StaleCheckpoint) | Err(_) => self.retry(failures),
        }
    }

    fn retry(&self, failures: u8) -> JobState {
        let jitter_ms = getrandom::u32().map_or(0, |random| random.to_le_bytes()[0]);
        JobState::after_failure(self.clock.now(), failures, jitter_ms)
    }
}
