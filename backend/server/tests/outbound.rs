use std::error::Error;
use std::num::NonZeroU16;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Duration;

use ctxfab_server::adapters::outbound::{http::HttpTransport, sqlite::SqliteStore};
use ctxfab_server::application::{ports::Repository, sync::TrafficLedger};
use ctxfab_server::domain::sync::{Admission, Reply, RequestKind};
use ctxfab_server::runtime::{
    clock::Clock,
    outbound::{OutboundGate, OutboundPool, QuotaPolicy, QuotaRegistrationError},
};
use http_body_util::{BodyExt, Full};
use hyper::{Request, StatusCode};
use tokio::sync::{Mutex, Notify};
use tokio::task::{JoinSet, yield_now};
use tokio::time::{advance, sleep};

const BUCKET: &str = "fixture-quota";
const EPOCH: i64 = 1_000_000;

fn gateway(
    store: Arc<SqliteStore>,
    clock: Clock,
    concurrency: NonZeroU16,
) -> Result<Arc<OutboundGate>, QuotaRegistrationError> {
    OutboundPool::new(store, clock).gate(
        BUCKET,
        QuotaPolicy {
            concurrency,
            spacing: Duration::from_millis(100),
        },
    )
}

#[tokio::test(start_paused = true)]
async fn overlapping_operations_share_spacing_concurrency_and_accounting()
-> Result<(), Box<dyn Error>> {
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let clock = Clock::anchored(EPOCH);
    let pool = OutboundPool::new(store.clone(), clock);
    let policy = QuotaPolicy {
        concurrency: NonZeroU16::MIN.saturating_add(1),
        spacing: Duration::from_millis(100),
    };
    let gate = pool.gate(BUCKET, policy)?;
    assert!(Arc::ptr_eq(&gate, &pool.gate(BUCKET, policy)?));
    assert!(matches!(
        pool.gate(
            BUCKET,
            QuotaPolicy {
                concurrency: NonZeroU16::MIN,
                ..policy
            }
        ),
        Err(QuotaRegistrationError::Conflict)
    ));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let handoffs = Arc::new(Mutex::new(Vec::new()));
    let kinds = [
        RequestKind::Manual,
        RequestKind::Scheduled,
        RequestKind::Validation,
        RequestKind::Pagination,
        RequestKind::Retry,
        RequestKind::Redirect,
        RequestKind::ModelDownload,
    ];
    let mut tasks = JoinSet::new();
    for kind in kinds.into_iter().cycle().take(21) {
        let (gate, active, peak, handoffs) =
            (gate.clone(), active.clone(), peak.clone(), handoffs.clone());
        tasks.spawn(async move {
            gate.handoff(kind, async |kind| {
                peak.fetch_max(active.fetch_add(1, Relaxed) + 1, Relaxed);
                handoffs.lock().await.push((clock.now(), kind));
                sleep(Duration::from_millis(230)).await;
                active.fetch_sub(1, Relaxed);
                Reply::Success(())
            })
            .await
        });
    }
    while gate.queued() < 21 {
        yield_now().await;
    }
    assert_eq!(
        store.status()?.outbound_attempts,
        0,
        "queued work is still waiting"
    );
    while let Some(result) = tasks.join_next().await {
        assert!(matches!(result??, Reply::Success(())));
    }
    let mut handoffs = handoffs.lock().await;
    handoffs.sort_unstable_by_key(|(time, _)| *time);
    assert_eq!(handoffs.len(), 21);
    assert_eq!(peak.load(Relaxed), 2);
    assert_eq!(gate.queued(), 0);
    for (offset, (start, _)) in handoffs.iter().enumerate() {
        let in_window = handoffs[offset..]
            .iter()
            .take_while(|(time, _)| *time < start + 1_000)
            .count();
        assert!(
            in_window <= 10,
            "at most ten attempts in any sliding second"
        );
    }
    assert!(handoffs.windows(2).all(|pair| pair[1].0 >= pair[0].0 + 100));
    for kind in kinds {
        assert_eq!(
            handoffs
                .iter()
                .filter(|(_, observed)| *observed == kind)
                .count(),
            3
        );
    }
    let dashboard = store.status()?;
    let stats = store.traffic(BUCKET)?;
    assert_eq!(stats.attempts, 21);
    assert_eq!(dashboard.outbound_attempts, stats.attempts);
    assert_eq!(stats.pending, 0);
    assert_eq!(stats.uncertain, 0);
    let latest_window = handoffs
        .iter()
        .filter(|(time, _)| *time >= stats.window_start)
        .count();
    assert_eq!(stats.window_attempts, i64::try_from(latest_window)?);
    println!("21 handoffs, peak concurrency 2, at most 10 requests per sliding second");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn restart_preserves_the_shared_cooldown_and_unanswered_attempt() -> Result<(), Box<dyn Error>>
{
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("traffic.sqlite3");
    let store = Arc::new(SqliteStore::open(&database)?);
    let clock = Clock::anchored(EPOCH);
    let gate = gateway(store.clone(), clock, NonZeroU16::MIN)?;
    let reply = gate
        .handoff::<()>(RequestKind::Scheduled, async |_| {
            Reply::Throttled(Duration::from_secs(3))
        })
        .await?;
    assert!(matches!(reply, Reply::Throttled(_)));
    let until = store.traffic(BUCKET)?.cooldown_until;

    let call = {
        let gate = gate.clone();
        tokio::spawn(async move {
            gate.handoff::<()>(RequestKind::Manual, async |_| std::future::pending().await)
                .await
        })
    };
    advance(Duration::from_secs(2)).await;
    assert_eq!(store.status()?.outbound_attempts, 1);
    // Stop while the caller is queued, then reopen the file with the cooldown still active.
    call.abort();
    assert!(call.await.is_err());
    drop(gate);
    drop(store);

    let store = Arc::new(SqliteStore::open(&database)?);
    store.recover_traffic(clock.now())?;
    assert_eq!(
        store.reserve(BUCKET, clock.now(), 100)?,
        Admission::WaitUntil(until)
    );
    clock.wait_until(until).await;
    // Persist the handoff without its in-process guard to simulate a hard process exit.
    assert_eq!(store.reserve(BUCKET, clock.now(), 100)?, Admission::Granted);
    assert_eq!(store.status()?.outbound_pending, 1);
    drop(store);

    let store = Arc::new(SqliteStore::open(&database)?);
    store.recover_traffic(clock.now())?;
    store.recover_traffic(clock.now())?;
    let stats = store.traffic(BUCKET)?;
    assert_eq!(stats.attempts, 2);
    assert_eq!(stats.pending, 0);
    assert_eq!(stats.uncertain, 1);
    assert_eq!(stats.failures, 1);
    assert_eq!(store.status()?.outbound_uncertain, 1);
    let gate = gateway(store.clone(), clock, NonZeroU16::MIN)?;
    let previous = clock.now();
    assert!(matches!(
        gate.handoff(RequestKind::Retry, async |_| Reply::Success(()))
            .await?,
        Reply::Success(())
    ));
    assert!(clock.now() >= previous + 1_000);
    println!(
        "Restart preserved the three-second cooldown and classified one interrupted handoff as uncertain"
    );
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn timed_out_transport_releases_capacity_and_records_the_failure()
-> Result<(), Box<dyn Error>> {
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let clock = Clock::anchored(EPOCH);
    let gate = gateway(store.clone(), clock, NonZeroU16::MIN)?;
    let reply = gate
        .handoff::<()>(RequestKind::Validation, async |_| {
            std::future::pending().await
        })
        .await?;
    assert!(matches!(reply, Reply::Temporary));
    assert_eq!(store.traffic(BUCKET)?.failures, 1);
    assert_eq!(store.status()?.outbound_pending, 0);
    assert!(matches!(
        gate.handoff(RequestKind::Retry, async |_| Reply::Success(()))
            .await?,
        Reply::Success(())
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn abort_after_handoff_settles_as_uncertain_without_restarting() -> Result<(), Box<dyn Error>>
{
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let clock = Clock::anchored(EPOCH);
    let gate = gateway(store.clone(), clock, NonZeroU16::MIN)?;
    let started = Arc::new(Notify::new());
    let request = {
        let gate = gate.clone();
        let started = started.clone();
        tokio::spawn(async move {
            gate.handoff::<()>(RequestKind::Scheduled, async |_| {
                started.notify_one();
                std::future::pending().await
            })
            .await
        })
    };
    started.notified().await;
    assert_eq!(store.traffic(BUCKET)?.pending, 1);
    request.abort();
    assert!(request.await.is_err_and(|error| error.is_cancelled()));
    let stopped = store.traffic(BUCKET)?;
    assert_eq!(
        (
            stopped.attempts,
            stopped.pending,
            stopped.uncertain,
            stopped.failures
        ),
        (1, 0, 1, 0)
    );
    assert!(stopped.cooldown_until >= clock.now() + 1_000);
    assert!(matches!(
        gate.handoff(RequestKind::Manual, async |_| {
            assert!(clock.now() >= stopped.cooldown_until);
            Reply::Success(())
        })
        .await?,
        Reply::Success(())
    ));
    let final_counts = store.traffic(BUCKET)?;
    assert_eq!(
        (
            final_counts.attempts,
            final_counts.pending,
            final_counts.uncertain
        ),
        (2, 0, 1)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_fixture_counts_every_explicit_hop_and_uses_retry_after() -> Result<(), Box<dyn Error>>
{
    use axum::Router;
    use axum::extract::{OriginalUri, State};
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;
    use tokio::net::TcpListener;

    type Hits = Arc<Mutex<Vec<(String, i64)>>>;
    let clock = Clock::system();
    let hits = Hits::default();
    let fixture = Router::new()
        .fallback(
            |State((hits, clock)): State<(Hits, Clock)>, OriginalUri(uri): OriginalUri| async move {
                hits.lock().await.push((uri.path().to_owned(), clock.now()));
                match uri.path().rsplit('/').next() {
                    Some("redirect") => {
                        (StatusCode::FOUND, [(header::LOCATION, "/repo-a/artifact")])
                            .into_response()
                    }
                    Some("limited") => {
                        (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "1")])
                            .into_response()
                    }
                    _ => StatusCode::OK.into_response(),
                }
            },
        )
        .with_state((hits.clone(), clock));
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, fixture).await });
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let transport = HttpTransport::new(gateway(store.clone(), clock, NonZeroU16::MIN)?)?;
    let fetch =
        |kind, path: &'static str| fixture_http_hop(&transport, kind, format!("{endpoint}{path}"));

    for (kind, path, expected) in [
        (
            RequestKind::Validation,
            "/repo-a/validation",
            StatusCode::OK,
        ),
        (RequestKind::Scheduled, "/repo-a/scheduled", StatusCode::OK),
        (RequestKind::Pagination, "/repo-b/page", StatusCode::OK),
        (RequestKind::Manual, "/repo-a/redirect", StatusCode::FOUND),
        (RequestKind::Redirect, "/repo-a/artifact", StatusCode::OK),
        (RequestKind::ModelDownload, "/repo-b/model", StatusCode::OK),
    ] {
        assert!(matches!(fetch(kind, path).await?, Reply::Success(code) if code == expected));
    }
    assert!(matches!(
        fetch(RequestKind::Scheduled, "/repo-a/limited").await?,
        Reply::Throttled(duration) if duration == Duration::from_secs(1)
    ));
    let cooled = store.traffic(BUCKET)?.cooldown_until;
    let (other_repo, retry) = tokio::join!(
        fetch(RequestKind::Scheduled, "/repo-b/after-cooldown"),
        fetch(RequestKind::Retry, "/repo-a/retry"),
    );
    assert!(matches!(other_repo?, Reply::Success(StatusCode::OK)));
    assert!(matches!(retry?, Reply::Success(StatusCode::OK)));
    let actual = hits.lock().await;
    let (before_cooldown, after_cooldown) = actual.split_at(7);
    assert_eq!(
        before_cooldown
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        [
            "/repo-a/validation",
            "/repo-a/scheduled",
            "/repo-b/page",
            "/repo-a/redirect",
            "/repo-a/artifact",
            "/repo-b/model",
            "/repo-a/limited",
        ]
    );
    let mut final_paths: Vec<_> = after_cooldown
        .iter()
        .map(|(path, _)| path.as_str())
        .collect();
    final_paths.sort_unstable();
    assert_eq!(final_paths, ["/repo-a/retry", "/repo-b/after-cooldown"]);
    assert!(after_cooldown.iter().all(|(_, sent_at)| *sent_at >= cooled));
    assert_eq!(
        store.status()?.outbound_attempts,
        i64::try_from(actual.len())?
    );
    assert_eq!(store.traffic(BUCKET)?.failures, 1);
    server.abort();
    println!(
        "Loopback HTTP observed nine handoffs and one Retry-After cooldown shared by two repositories"
    );
    Ok(())
}

async fn fixture_http_hop(
    transport: &HttpTransport,
    kind: RequestKind,
    url: String,
) -> Result<Reply<StatusCode>, Box<dyn Error>> {
    let request = Request::get(url).body(Full::default())?;
    Ok(transport
        .send(kind, request, Duration::from_secs(3), async |response| {
            let status = response.status();
            match response.body_mut().collect().await {
                Ok(_) => Reply::Success(status),
                Err(_) => Reply::Temporary,
            }
        })
        .await?)
}

#[tokio::test(start_paused = true)]
async fn missing_accounting_storage_withholds_the_transport() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("failed.sqlite3");
    let store = Arc::new(SqliteStore::open(&database)?);
    rusqlite::Connection::open(&database)?.execute_batch("DROP TABLE outbound_buckets")?;
    let gate = gateway(store, Clock::anchored(EPOCH), NonZeroU16::MIN)?;
    let sent = AtomicUsize::new(0);
    let result = gate
        .handoff(RequestKind::Scheduled, async |_| {
            sent.fetch_add(1, Relaxed);
            Reply::Success(())
        })
        .await;
    assert!(result.is_err());
    assert_eq!(sent.load(Relaxed), 0);
    assert_eq!(gate.queued(), 0);
    Ok(())
}
