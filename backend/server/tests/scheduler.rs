use std::collections::VecDeque;
use std::error::Error;
use std::num::NonZeroU16;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ctxfab_server::adapters::outbound::sqlite::SqliteStore;
use ctxfab_server::application::{
    SearchQuery,
    ports::{CorpusStore, Repository},
    sync::{CollectionJobs, PageFetch, PageSource},
};
use ctxfab_server::domain::corpus::{CollectionAccess, Record, RecordKind};
use ctxfab_server::domain::sync::{JobState, Page, Phase, Reply, RequestKind};
use ctxfab_server::runtime::{
    clock::Clock,
    outbound::{OutboundGate, OutboundPool, QuotaPolicy, QuotaRegistrationError},
    scheduler::{CollectionJob, Scheduler},
};
use tokio::sync::Notify;
use tokio::time::{sleep, timeout};

const COLLECTION: &str = "fixture-collection";
const EPOCH: i64 = 3_000_000;

type Call = (Option<String>, RequestKind);

struct Step {
    reply: Reply<Page>,
    release: Option<Arc<Notify>>,
}

struct Fixture {
    replies: Mutex<VecDeque<Step>>,
    calls: Mutex<Vec<Call>>,
}

impl Fixture {
    fn new(replies: impl IntoIterator<Item = Reply<Page>>) -> Self {
        Self {
            replies: Mutex::new(
                replies
                    .into_iter()
                    .map(|reply| Step {
                        reply,
                        release: None,
                    })
                    .collect(),
            ),
            calls: Mutex::default(),
        }
    }

    fn calls(&self) -> Result<Vec<Call>, Box<dyn Error>> {
        Ok(self.calls.lock().map_err(|_| "fixture calls lock")?.clone())
    }
}

struct FixtureSource {
    fixture: Arc<Fixture>,
    gate: Arc<OutboundGate>,
}

impl PageSource for FixtureSource {
    fn fetch<'a>(&'a self, cursor: Option<&'a str>, kind: RequestKind) -> PageFetch<'a> {
        Box::pin(self.gate.handoff(kind, async move |kind| {
            if let Ok(mut calls) = self.fixture.calls.lock() {
                calls.push((cursor.map(str::to_owned), kind));
            }
            let step = self
                .fixture
                .replies
                .lock()
                .ok()
                .and_then(|mut replies| replies.pop_front());
            let Some(step) = step else {
                return Reply::Permanent;
            };
            if let Some(release) = step.release {
                release.notified().await;
            }
            step.reply
        }))
    }
}

fn page(cursor: &str, content: &str, more: bool) -> Reply<Page> {
    Reply::Success(Page {
        cursor: cursor.to_owned(),
        documents: vec![Record {
            source: "fixture:scope:src/main.rs".into(),
            revision: cursor.to_owned(),
            kind: RecordKind::Current {
                tracked_ref: "refs/heads/main".into(),
            },
            title: "Fixture".into(),
            content: content.to_owned(),
        }],
        more,
    })
}

fn job(
    store: Arc<SqliteStore>,
    source: Arc<Fixture>,
    clock: Clock,
) -> Result<CollectionJob, QuotaRegistrationError> {
    let gate = OutboundPool::new(store.clone(), clock).gate(
        "fixture-quota",
        QuotaPolicy {
            concurrency: NonZeroU16::MIN,
            spacing: Duration::from_millis(100),
        },
    )?;
    Ok(CollectionJob::new(
        COLLECTION.into(),
        Arc::new(FixtureSource {
            fixture: source,
            gate,
        }),
        store,
        clock,
        Duration::from_secs(5),
    ))
}

fn memory() -> rusqlite::Result<Arc<SqliteStore>> {
    Ok(Arc::new(SqliteStore::open(std::path::Path::new(
        ":memory:",
    ))?))
}

async fn wait_for(
    store: &SqliteStore,
    matches: impl Fn(JobState) -> bool,
) -> Result<JobState, Box<dyn Error>> {
    timeout(Duration::from_secs(75), async {
        loop {
            if let Some(state) = store.load_job(COLLECTION)?.map(|job| job.state)
                && matches(state)
            {
                return Ok::<_, Box<dyn Error>>(state);
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await?
}

#[tokio::test(start_paused = true)]
async fn scheduled_pagination_and_force_refresh_coalesce_and_use_the_delta()
-> Result<(), Box<dyn Error>> {
    let store = memory()?;
    let clock = Clock::anchored(EPOCH);
    let release = Arc::new(Notify::new());
    let source = Arc::new(Fixture::new([
        page("p1", "baselinefixture", true),
        page("p2", "firstfixture", false),
        page("p3", "incrementalfixture", false),
        page("p4", "scheduledfixture", false),
    ]));
    source
        .replies
        .lock()
        .map_err(|_| "fixture replies lock")?
        .front_mut()
        .ok_or("missing fixture reply")?
        .release = Some(release.clone());

    let mut scheduler = Scheduler::default();
    let refresh = scheduler.register(job(store.clone(), source.clone(), clock)?);
    let same = scheduler.register(job(store.clone(), source.clone(), clock)?);
    wait_for(&store, |state| state.phase == Phase::Running).await?;
    assert!(refresh.request());
    assert!(
        !same.request(),
        "one pending refresh for the running collection"
    );
    release.notify_one();
    let first = wait_for(&store, |state| state.phase == Phase::Succeeded).await?;
    assert_eq!(
        source.calls()?,
        vec![
            (None, RequestKind::Scheduled),
            (Some("p1".into()), RequestKind::Pagination),
        ]
    );
    assert!(refresh.request());
    wait_for(&store, |state| {
        state.phase == Phase::Succeeded && state.due_at != first.due_at
    })
    .await?;
    assert!(
        store
            .search(&SearchQuery::parse("firstfixture")?)?
            .is_empty()
    );
    assert_eq!(
        store.search(&SearchQuery::parse("incrementalfixture")?)?[0].revision,
        "p3"
    );
    assert_eq!(source.calls()?[2], (Some("p2".into()), RequestKind::Manual));
    tokio::time::advance(Duration::from_secs(6)).await;
    wait_for(&store, |state| {
        state.phase == Phase::Succeeded && source.calls().is_ok_and(|calls| calls.len() == 4)
    })
    .await?;
    assert_eq!(
        source.calls()?[3],
        (Some("p3".into()), RequestKind::Scheduled)
    );
    assert_eq!(store.status()?.indexed_documents, 1);
    scheduler.stop().await;
    assert!(!refresh.request());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_fetched_page_is_replayed_after_index_failure_and_restart() -> Result<(), Box<dyn Error>>
{
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("sync.sqlite3");
    let store = Arc::new(SqliteStore::open(&database)?);
    let clock = Clock::anchored(EPOCH);
    let source = Arc::new(Fixture::new([
        page("p1", "baselinefixture", true),
        page("p2", "replayedfixture", false),
    ]));
    rusqlite::Connection::open(&database)?.execute_batch(
        "CREATE TRIGGER fixture_fail BEFORE INSERT ON documents WHEN new.revision = 'p2'
         BEGIN SELECT raise(ABORT, 'fixture interrupted indexing'); END;",
    )?;
    let mut scheduler = Scheduler::default();
    let _refresh = scheduler.register(job(store.clone(), source.clone(), clock)?);
    let retry = wait_for(&store, |state| state.phase == Phase::Retry).await?;
    assert_eq!(source.calls()?.len(), 2);
    assert_eq!(store.status()?.outbound_attempts, 2);
    assert_eq!(
        store
            .checkpoint(COLLECTION)?
            .and_then(|value| value.cursor)
            .as_deref(),
        Some("p1")
    );
    assert!(
        store
            .search(&SearchQuery::parse("replayedfixture")?)?
            .is_empty()
    );
    scheduler.stop().await;
    drop(store);

    let store = Arc::new(SqliteStore::open(&database)?);
    rusqlite::Connection::open(&database)?.execute_batch("DROP TRIGGER fixture_fail")?;
    store.recover_traffic(clock.now())?;
    assert_eq!(
        store.load_job(COLLECTION)?.map(|job| job.state),
        Some(retry)
    );
    let source = Arc::new(Fixture::new([page("p2", "replayedfixture", false)]));
    let mut scheduler = Scheduler::default();
    let _refresh = scheduler.register(job(store.clone(), source.clone(), clock)?);
    wait_for(&store, |state| state.phase == Phase::Succeeded).await?;
    assert_eq!(
        source.calls()?,
        vec![(Some("p1".into()), RequestKind::Retry)]
    );
    assert_eq!(
        store.search(&SearchQuery::parse("replayedfixture")?)?[0].revision,
        "p2"
    );
    assert_eq!(store.status()?.outbound_attempts, 3);
    scheduler.stop().await;
    println!("Fetched page replayed from the last searchable checkpoint after restart");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cooldown_defers_manual_refresh_and_cursor_expiry_blocks_collection()
-> Result<(), Box<dyn Error>> {
    let store = memory()?;
    let clock = Clock::anchored(EPOCH);
    let source = Arc::new(Fixture::new([
        Reply::Throttled(Duration::from_secs(3)),
        Reply::Temporary,
        Reply::ExpiredCursor,
    ]));
    let mut scheduler = Scheduler::default();
    let refresh = scheduler.register(job(store.clone(), source.clone(), clock)?);
    let throttled = wait_for(&store, |state| state.phase == Phase::Throttled).await?;
    assert!(refresh.request());
    sleep(Duration::from_secs(2)).await;
    assert!(clock.now() < throttled.due_at.ok_or("missing cooldown")?);
    assert_eq!(source.calls()?.len(), 1);
    let retry = wait_for(&store, |state| state.phase == Phase::Retry).await?;
    assert_eq!(retry.failures, 1);
    assert!(retry.due_at.is_some_and(|due| due > clock.now()));
    wait_for(&store, |state| state.phase == Phase::ExpiredCursor).await?;
    assert_eq!(
        source.calls()?,
        vec![
            (None, RequestKind::Scheduled),
            (None, RequestKind::Retry),
            (None, RequestKind::Retry),
        ]
    );
    assert!(refresh.request());
    sleep(Duration::from_secs(15)).await;
    assert_eq!(source.calls()?.len(), 3);
    assert!(store.checkpoint(COLLECTION)?.is_none());
    let diagnostics = store.diagnostics()?;
    for expected in ["sync_throttled", "sync_retry", "sync_expired_cursor"] {
        assert!(diagnostics.iter().any(|event| event.code == expected));
    }
    scheduler.stop().await;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn denied_credentials_retain_the_last_searchable_checkpoint() -> Result<(), Box<dyn Error>> {
    let store = memory()?;
    let clock = Clock::anchored(EPOCH);
    let source = Arc::new(Fixture::new([
        page("p1", "retainedfixture", false),
        Reply::Credentials,
        page("p2", "reauthorizedfixture", false),
    ]));
    let mut scheduler = Scheduler::default();
    let refresh = scheduler.register(job(store.clone(), source.clone(), clock)?);
    wait_for(&store, |state| state.phase == Phase::Succeeded).await?;
    assert!(refresh.request());
    wait_for(&store, |state| state.phase == Phase::Credentials).await?;
    let checkpoint = store.checkpoint(COLLECTION)?.ok_or("missing checkpoint")?;
    assert_eq!(checkpoint.generation, 1);
    assert_eq!(checkpoint.access, CollectionAccess::Lost);
    assert_eq!(checkpoint.cursor.as_deref(), Some("p1"));
    assert_eq!(
        store.search(&SearchQuery::parse("retainedfixture")?)?[0].access,
        "retained"
    );
    assert!(refresh.request());
    sleep(Duration::from_secs(20)).await;
    assert_eq!(source.calls()?.len(), 2);
    assert!(
        store
            .diagnostics()?
            .iter()
            .any(|event| event.code == "sync_credentials")
    );
    assert!(store.set_collection_access(
        COLLECTION,
        checkpoint.generation,
        CollectionAccess::Available
    )?);
    assert!(refresh.request());
    wait_for(&store, |state| state.phase == Phase::Succeeded).await?;
    assert_eq!(source.calls()?[2], (Some("p1".into()), RequestKind::Manual));
    assert_eq!(
        store
            .search(&SearchQuery::parse("reauthorizedfixture")?)?
            .len(),
        1
    );
    scheduler.stop().await;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_delayed_denial_cannot_block_a_replaced_collection() -> Result<(), Box<dyn Error>> {
    let store = memory()?;
    let clock = Clock::anchored(EPOCH);
    let source = Arc::new(Fixture::new([
        page("old", "oldfixture", false),
        Reply::Credentials,
        page("new", "newfixture", false),
    ]));
    let release = Arc::new(Notify::new());
    source.replies.lock().map_err(|_| "fixture replies lock")?[1].release = Some(release.clone());
    let mut scheduler = Scheduler::default();
    let refresh = scheduler.register(job(store.clone(), source.clone(), clock)?);
    wait_for(&store, |state| state.phase == Phase::Succeeded).await?;
    assert!(refresh.request());
    timeout(Duration::from_secs(2), async {
        while !source.calls().is_ok_and(|calls| calls.len() == 2) {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;

    store.delete_collection(COLLECTION)?;
    assert!(refresh.request());
    release.notify_one();
    timeout(Duration::from_secs(2), async {
        loop {
            if let Some(checkpoint) = store.checkpoint(COLLECTION)?
                && checkpoint.cursor.as_deref() == Some("new")
            {
                return Ok::<_, Box<dyn Error>>(checkpoint);
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    assert_eq!(source.calls()?[2], (None, RequestKind::Manual));
    assert!(
        !store
            .diagnostics()?
            .iter()
            .any(|event| event.code == "sync_credentials")
    );
    scheduler.stop().await;
    Ok(())
}

#[test]
fn upgrade_keeps_valid_job_state_and_discards_obsolete_denials() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("jobs.sqlite3");
    let legacy = rusqlite::Connection::open(&path)?;
    for migration in [
        include_str!("../src/adapters/outbound/sqlite/migrations/0001_foundation.sql"),
        include_str!("../src/adapters/outbound/sqlite/migrations/0002_corpus.sql"),
        include_str!("../src/adapters/outbound/sqlite/migrations/0003_sync.sql"),
        include_str!("../src/adapters/outbound/sqlite/migrations/0004_retrieval.sql"),
        include_str!("../src/adapters/outbound/sqlite/migrations/0005_canonical_lexical.sql"),
        include_str!("../src/adapters/outbound/sqlite/migrations/0006_diagnostic_storage.sql"),
    ] {
        legacy.execute_batch(migration)?;
    }
    legacy.execute_batch(
        "PRAGMA user_version = 6;
         INSERT INTO collections(name, generation, cursor, access_lost)
             VALUES ('active', 3, 'last-page', 0), ('blocked', 1, 'retained-page', 1);
         INSERT INTO sync_jobs(collection, phase, due_at, failures)
             VALUES ('active', 'credentials', NULL, 0), ('blocked', 'credentials', NULL, 0),
                    ('scheduled', 'retry', 3000500, 2);",
    )?;
    drop(legacy);
    let store = SqliteStore::open(&path)?;
    assert!(store.load_job("active")?.is_none());
    assert_eq!(
        store
            .checkpoint("active")?
            .ok_or("missing checkpoint")?
            .cursor
            .as_deref(),
        Some("last-page")
    );
    let blocked = store.load_job("blocked")?.ok_or("missing blocked job")?;
    assert_eq!(
        (blocked.generation, blocked.state.phase),
        (1, Phase::Credentials)
    );
    let scheduled = store
        .load_job("scheduled")?
        .ok_or("missing scheduled job")?;
    assert_eq!(
        (
            scheduled.generation,
            scheduled.state.phase,
            scheduled.state.failures
        ),
        (0, Phase::Retry, 2)
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn bounded_retry_reaches_failed_and_waits_for_explicit_refresh() -> Result<(), Box<dyn Error>>
{
    let store = memory()?;
    let clock = Clock::anchored(EPOCH);
    let source = Arc::new(Fixture::new(
        (0..5).map(|_| Reply::Temporary).chain([Reply::Permanent]),
    ));
    let mut scheduler = Scheduler::default();
    let refresh = scheduler.register(job(store.clone(), source.clone(), clock)?);
    wait_for(&store, |state| state.phase == Phase::Failed).await?;
    assert_eq!(source.calls()?.len(), 5);
    sleep(Duration::from_secs(30)).await;
    assert_eq!(source.calls()?.len(), 5);
    assert!(refresh.request());
    wait_for(&store, |state| {
        state.phase == Phase::Failed && source.calls().is_ok_and(|calls| calls.len() == 6)
    })
    .await?;
    assert_eq!(source.calls()?[5], (None, RequestKind::Manual));
    assert!(
        store
            .diagnostics()?
            .iter()
            .any(|event| event.code == "sync_failed")
    );
    scheduler.stop().await;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_page_with_new_content_cannot_reuse_the_current_cursor() -> Result<(), Box<dyn Error>> {
    let store = memory()?;
    let clock = Clock::anchored(EPOCH);
    let source = Arc::new(Fixture::new([
        page("p1", "baselinefixture", false),
        page("p1", "invaliddelta", false),
    ]));
    let mut scheduler = Scheduler::default();
    let refresh = scheduler.register(job(store.clone(), source.clone(), clock)?);
    wait_for(&store, |state| state.phase == Phase::Succeeded).await?;
    assert!(refresh.request());
    wait_for(&store, |state| state.phase == Phase::Failed).await?;
    assert_eq!(
        store.search(&SearchQuery::parse("baselinefixture")?)?.len(),
        1
    );
    assert!(
        store
            .search(&SearchQuery::parse("invaliddelta")?)?
            .is_empty()
    );
    assert_eq!(source.calls()?.len(), 2);
    scheduler.stop().await;
    Ok(())
}
