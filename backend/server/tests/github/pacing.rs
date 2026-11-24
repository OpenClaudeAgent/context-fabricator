use super::*;
use axum::extract::{OriginalUri, State};
use axum::http::header;
use axum::response::IntoResponse;
use ctxfab_server::adapters::outbound::http::HttpTransport;
use ctxfab_server::domain::sync::{Reply, RequestKind};
use http_body_util::{BodyExt, Full};
use tokio::sync::Mutex;

type Hits = Arc<Mutex<Vec<(String, i64)>>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_repositories_share_edited_rate_and_cooldown_across_restart()
-> Result<(), Box<dyn Error>> {
    let clock = Clock::system();
    let hits = Hits::default();
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("pacing.sqlite3");
    let store = Arc::new(SqliteStore::open(&path)?);
    let fixture = fixture(hits.clone(), path.clone());
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let endpoint = format!("http://{}", listener.local_addr()?);
    let server = tokio::spawn(async move { axum::serve(listener, fixture).await });
    let policy = QuotaPolicy {
        concurrency: NonZeroU16::MIN,
        spacing: Duration::from_millis(8),
    };
    let gate = OutboundPool::new(store.clone(), clock).gate(BUCKET, policy)?;
    let transport = Arc::new(HttpTransport::new(gate.clone())?);
    assert!(
        matches!(hop(&transport, RequestKind::Scheduled, format!("{endpoint}/alpha/limited")).await?, Reply::Throttled(d) if d == Duration::from_secs(3))
    );
    let cooldown = store.traffic(BUCKET)?.cooldown_until;
    let mut tasks = Vec::new();
    for (kind, repo) in [
        (RequestKind::Scheduled, "alpha"),
        (RequestKind::Scheduled, "beta"),
    ] {
        let transport = transport.clone();
        let url = format!("{endpoint}/{repo}/queued");
        tasks.push(tokio::spawn(
            async move { hop(&transport, kind, url).await },
        ));
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        while gate.queued() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert_eq!(store.traffic(BUCKET)?.attempts, 1);
    store.set_rate(0, 60)?;
    assert_eq!(store.traffic(BUCKET)?.cooldown_until, cooldown);
    assert_eq!(hits.lock().await.len(), 1);
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
    drop(transport);
    drop(gate);
    drop(store);

    let store = Arc::new(SqliteStore::open(&path)?);
    let persisted = store.traffic(BUCKET)?;
    assert_eq!(persisted.requests_per_minute, Some(60));
    assert_eq!(persisted.cooldown_until, cooldown);
    let gate = OutboundPool::new(store.clone(), clock).gate(BUCKET, policy)?;
    let transport = HttpTransport::new(gate)?;
    let (alpha, beta) = tokio::join!(
        hop(
            &transport,
            RequestKind::Manual,
            format!("{endpoint}/alpha/force")
        ),
        hop(
            &transport,
            RequestKind::Scheduled,
            format!("{endpoint}/beta/scheduled")
        ),
    );
    assert!(matches!(alpha?, Reply::Success(StatusCode::OK)));
    assert!(matches!(beta?, Reply::Success(StatusCode::OK)));
    let hits = hits.lock().await;
    assert_eq!(hits.len(), 3);
    assert!(hits[1].1 >= cooldown);
    assert!(hits[2].1 - hits[1].1 >= 1_000);
    let mut paths = [hits[1].0.as_str(), hits[2].0.as_str()];
    paths.sort_unstable();
    assert_eq!(paths, ["/alpha/force", "/beta/scheduled"]);
    let traffic = store.traffic(BUCKET)?;
    assert_eq!(
        (
            traffic.attempts,
            traffic.failures,
            traffic.pending,
            traffic.uncertain
        ),
        (3, 1, 0, 0)
    );
    server.abort();
    Ok(())
}

fn fixture(hits: Hits, path: std::path::PathBuf) -> Router {
    Router::new()
        .fallback(
            |State((hits, path)): State<(Hits, std::path::PathBuf)>,
             OriginalUri(uri): OriginalUri| async move {
                // The response holds the single in-flight permit, so no later request can
                // overwrite this bucket's admission timestamp before it is recorded.
                let admission = rusqlite::Connection::open(path).and_then(|connection| {
                    connection.query_row(
                        "SELECT last_handoff FROM outbound_buckets WHERE bucket = ?1",
                        [BUCKET],
                        |row| row.get::<_, i64>(0),
                    )
                });
                let Ok(admission) = admission else {
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                };
                hits.lock().await.push((uri.path().to_owned(), admission));
                if uri.path() == "/alpha/limited" {
                    (StatusCode::TOO_MANY_REQUESTS, [(header::RETRY_AFTER, "3")]).into_response()
                } else {
                    StatusCode::OK.into_response()
                }
            },
        )
        .with_state((hits, path))
}

async fn hop(
    transport: &HttpTransport,
    kind: RequestKind,
    url: String,
) -> Result<Reply<StatusCode>, String> {
    let request = hyper::Request::get(url)
        .body(Full::default())
        .map_err(|error| error.to_string())?;
    transport
        .send(kind, request, Duration::from_secs(3), async |response| {
            let status = response.status();
            match response.body_mut().collect().await {
                Ok(_) => Reply::Success(status),
                Err(_) => Reply::Temporary,
            }
        })
        .await
        .map_err(|error| error.to_string())
}
