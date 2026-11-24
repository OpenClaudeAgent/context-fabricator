use std::error::Error;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::num::NonZeroU16;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ctxfab_server::adapters::{
    inbound::http,
    outbound::{
        hugging_face::{BUCKET, HuggingFace},
        sqlite::SqliteStore,
    },
};
use ctxfab_server::application::model_management::{
    ArtifactResult, ArtifactStore, DownloadJobStore, LocalArtifactState, ModelManagement,
};
use ctxfab_server::application::sync::TrafficLedger;
use ctxfab_server::domain::model::{Artifact, DownloadError, InstallationStage, Snapshot, State};
use ctxfab_server::runtime::{
    clock::Clock,
    outbound::{OutboundGate, OutboundPool, QuotaPolicy},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tower::ServiceExt;

const PROFILE: &str = "fixture-model";
const BYTES: &[u8] = b"hello world";
const SHA256: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

#[tokio::test]
async fn interrupted_artifact_resumes_and_every_hop_uses_the_ledger() -> Result<(), Box<dyn Error>>
{
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let server = tokio::spawn(async move {
        let script = [
            (
                "GET /model ",
                "HTTP/1.1 302 Found\r\nLocation: /blob\r\nContent-Length: 0\r\n\r\n",
                None,
            ),
            (
                "GET /blob ",
                "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 1\r\nContent-Length: 0\r\n\r\n",
                None,
            ),
            (
                "GET /model ",
                "HTTP/1.1 200 OK\r\nETag: \"fixture-v1\"\r\nContent-Length: 11\r\n\r\nhello",
                None,
            ),
            (
                "GET /model ",
                "HTTP/1.1 206 Partial Content\r\nETag: \"fixture-v1\"\r\nContent-Length: 6\r\nContent-Range: bytes 5-10/11\r\n\r\n world",
                Some("bytes=5-"),
            ),
        ];
        let mut observed = Vec::new();
        for (prefix, response, range) in script {
            let (mut socket, _) = listener.accept().await?;
            let request = read_request(&mut socket).await?;
            assert!(request.starts_with(prefix), "{request}");
            assert_eq!(
                range,
                request
                    .lines()
                    .find_map(|line| line.strip_prefix("range: "))
            );
            if range.is_some() {
                assert!(request.contains("if-range: \"fixture-v1\"\r\n"));
            }
            observed.push(Instant::now());
            socket.write_all(response.as_bytes()).await?;
            socket.shutdown().await?;
        }
        Ok::<_, std::io::Error>(observed)
    });

    let store = Arc::new(SqliteStore::open(
        &directory.path().join("context.sqlite3"),
    )?);
    let downloads = fixture(directory.path(), &store, addr)?;
    downloads.request(PROFILE)?;
    downloads.request(PROFILE)?;
    let status = terminal(&downloads).await?;
    assert_eq!(status.state, State::Installed);
    assert_eq!(status.downloaded_bytes, 11);
    assert_eq!(
        tokio::fs::read(directory.path().join("inference/fixture.gguf")).await?,
        BYTES
    );

    let observed = tokio::time::timeout(Duration::from_secs(2), server).await???;
    assert!(
        observed
            .windows(2)
            .all(|pair| pair[1].duration_since(pair[0]) >= Duration::from_millis(70))
    );
    assert!(observed[2].duration_since(observed[1]) >= Duration::from_millis(950));
    let traffic = store.traffic(BUCKET)?;
    assert_eq!(traffic.attempts, 4);
    assert_eq!(traffic.failures, 2);
    assert_eq!(traffic.pending, 0);
    Ok(())
}

async fn read_request(socket: &mut TcpStream) -> std::io::Result<String> {
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") && request.len() < 4096 {
        request.push(socket.read_u8().await?);
    }
    String::from_utf8(request).map_err(std::io::Error::other)
}

#[tokio::test]
async fn stop_and_restart_resume_a_staged_file_with_durable_accounting()
-> Result<(), Box<dyn Error>> {
    use ctxfab_server::application::model_management::DownloadJobStore;

    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await?;
        assert!(read_request(&mut first).await?.starts_with("GET /model "));
        first
            .write_all(
                b"HTTP/1.1 200 OK\r\nETag: \"fixture-v1\"\r\nContent-Length: 11\r\n\r\nhello",
            )
            .await?;
        let mut eof = [0];
        assert_eq!(first.read(&mut eof).await?, 0);

        let (mut second, _) = listener.accept().await?;
        assert!(
            read_request(&mut second)
                .await?
                .contains("range: bytes=5-\r\n")
        );
        second.write_all(b"HTTP/1.1 206 Partial Content\r\nETag: \"fixture-v1\"\r\nContent-Range: bytes 5-10/11\r\nContent-Length: 6\r\n\r\n world").await?;
        second.shutdown().await
    });

    let database = directory.path().join("context.sqlite3");
    let store = Arc::new(SqliteStore::open(&database)?);
    let first = durable_fixture(directory.path(), store.clone(), addr)?;
    first.request(PROFILE)?;
    let running = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let snapshot = first.snapshot().await?.remove(0);
            if snapshot.downloaded_bytes == 5 {
                return Ok::<_, DownloadError>(snapshot);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await??;
    assert_eq!(running.state, State::Installing);
    assert_eq!(
        running.installation_stage,
        Some(InstallationStage::Transferring)
    );
    let operation = running.operation_id.ok_or("operation ID")?;
    assert_eq!(store.traffic(BUCKET)?.pending, 1);
    first.stop().await;
    let interrupted = first.snapshot().await?.remove(0);
    assert_eq!(interrupted.state, State::Interrupted);
    assert_eq!(interrupted.installation_stage, None);
    assert_eq!(
        interrupted.operation_id.as_deref(),
        Some(operation.as_str())
    );
    assert_eq!(interrupted.downloaded_bytes, 5);
    let saved = store.load_download_jobs()?;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].phase, "interrupted");
    assert_eq!(saved[0].operation_id, operation);
    assert_eq!(store.traffic(BUCKET)?.pending, 0);
    assert_eq!(store.traffic(BUCKET)?.uncertain, 1);
    assert!(first.request(PROFILE).is_err());
    assert_eq!(
        tokio::fs::read(directory.path().join("inference/.fixture.gguf.part")).await?,
        b"hello"
    );
    drop(first);
    drop(store);

    let store = Arc::new(SqliteStore::open(&database)?);
    store.recover_traffic(Clock::system().now())?;
    let resumed = durable_fixture(directory.path(), store.clone(), addr)?;
    let restored = resumed.snapshot().await?.remove(0);
    assert_eq!(restored.state, State::Interrupted);
    assert_eq!(restored.operation_id.as_deref(), Some(operation.as_str()));
    assert_eq!(store.traffic(BUCKET)?.attempts, 1);
    resumed.resume(PROFILE)?;
    assert_eq!(terminal(&resumed).await?.state, State::Installed);
    assert!(store.load_download_jobs()?.is_empty());
    tokio::time::timeout(Duration::from_secs(2), server).await???;
    assert_eq!(store.traffic(BUCKET)?.attempts, 2);
    assert_eq!(store.traffic(BUCKET)?.uncertain, 1);
    assert_eq!(store.traffic(BUCKET)?.pending, 0);
    Ok(())
}

#[tokio::test]
async fn failed_download_journal_write_leaves_no_operation_and_sends_no_request()
-> Result<(), Box<dyn Error>> {
    use ctxfab_server::application::model_management::DownloadJobStore;

    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let database = directory.path().join("context.sqlite3");
    let store = Arc::new(SqliteStore::open(&database)?);
    let downloads = durable_fixture(directory.path(), store.clone(), address)?;
    rusqlite::Connection::open(database)?.execute_batch(
        "CREATE TRIGGER reject_download BEFORE INSERT ON model_download_jobs
         BEGIN SELECT RAISE(ABORT, 'fixture journal failure'); END;",
    )?;

    assert_eq!(downloads.request(PROFILE), Err(DownloadError::Unavailable));
    let snapshot = downloads.snapshot().await?.remove(0);
    assert_eq!(snapshot.state, State::NotInstalled);
    assert_eq!(snapshot.operation_id, None);
    assert!(store.load_download_jobs()?.is_empty());
    assert_eq!(store.traffic(BUCKET)?.attempts, 0);
    Ok(())
}

#[tokio::test]
async fn refuses_invalid_weights_and_off_scope_redirects() -> Result<(), Box<dyn Error>> {
    for (response, code) in [
        (
            "HTTP/1.1 200 OK\r\nETag: \"fixture-v1\"\r\nContent-Length: 11\r\n\r\nHELLO WORLD",
            "model_integrity_failed",
        ),
        (
            "HTTP/1.1 302 Found\r\nLocation: https://example.invalid/model\r\nContent-Length: 0\r\n\r\n",
            "model_transfer_failed",
        ),
        (
            "HTTP/1.1 206 Partial Content\r\nETag: \"fixture-v1\"\r\nContent-Length: 6\r\nContent-Range: bytes 5-10/11\r\n\r\n world",
            "model_transfer_failed",
        ),
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 12\r\n\r\nhello worlds",
            "model_transfer_failed",
        ),
    ] {
        let directory = tempfile::tempdir()?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            read_request(&mut socket).await?;
            socket.write_all(response.as_bytes()).await?;
            socket.shutdown().await
        });
        let store = Arc::new(SqliteStore::open(
            &directory.path().join("context.sqlite3"),
        )?);
        let downloads = fixture(directory.path(), &store, addr)?;
        downloads.request(PROFILE)?;
        let failed = terminal(&downloads).await?;
        assert_eq!(failed.state, State::Failed);
        assert_eq!(failed.error, Some(code));
        if code == "model_integrity_failed" {
            assert!(
                !directory
                    .path()
                    .join("inference/.fixture.gguf.resume")
                    .exists()
            );
        }
        assert_eq!(failed.downloaded_bytes, 0);
        assert!(!tokio::fs::try_exists(directory.path().join("inference/fixture.gguf")).await?);
        tokio::time::timeout(Duration::from_secs(2), server).await???;
        assert_eq!(store.traffic(BUCKET)?.attempts, 1);
    }
    Ok(())
}

#[tokio::test]
async fn authenticated_model_command_installs_and_reuses_a_verified_artifact()
-> Result<(), Box<dyn Error>> {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use serde_json::{Value, json};

    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        read_request(&mut socket).await?;
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nETag: \"fixture-v1\"\r\nContent-Length: 11\r\n\r\nhello world",
            )
            .await?;
        socket.shutdown().await
    });

    let store = Arc::new(SqliteStore::open(
        &directory.path().join("context.sqlite3"),
    )?);
    let downloads = fixture(directory.path(), &store, addr)?;
    let app = http::router_with_services(
        store.clone(),
        "fixture-token",
        http::Services {
            downloads: Some(downloads.clone()),
            ..http::Services::default()
        },
    );
    let send = |method, body: Option<Value>, authorized| {
        let mut builder = Request::builder().method(method).uri("/v1/model-downloads");
        if authorized {
            builder = builder.header("Authorization", "Bearer fixture-token");
        }
        if body.is_some() {
            builder = builder.header("Content-Type", "application/json");
        }
        Ok::<_, axum::http::Error>(app.clone().oneshot(
            builder.body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))?,
        ))
    };
    assert_eq!(
        send("POST", Some(json!({"profile":PROFILE})), false)?
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            "POST",
            Some(json!({"profile":PROFILE,"url":"https://example.invalid/"})),
            true
        )?
        .await?
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send("POST", Some(json!({"profile":""})), true)?
            .await?
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        send("POST", Some(json!({"profile":"unknown"})), true)?
            .await?
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(store.traffic(BUCKET)?.attempts, 0);
    for _ in 0..2 {
        let accepted = send("POST", Some(json!({"profile":PROFILE})), true)?.await?;
        assert_eq!(accepted.status(), StatusCode::ACCEPTED);
        assert_eq!(
            serde_json::from_slice::<Value>(&to_bytes(accepted.into_body(), 4096).await?)?,
            json!({"profile":PROFILE})
        );
        assert_eq!(terminal(&downloads).await?.state, State::Installed);
    }
    let status = send("GET", None, true)?.await?;
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(
        serde_json::from_slice::<Value>(&to_bytes(status.into_body(), 4096).await?)?,
        json!([{"profile":PROFILE,"state":"installed","downloadedBytes":11,"totalBytes":11}])
    );
    tokio::time::timeout(Duration::from_secs(2), server).await???;
    assert_eq!(store.traffic(BUCKET)?.attempts, 1);
    remove_fixture_weights(app).await?;
    assert_eq!(terminal(&downloads).await?.state, State::NotInstalled);
    assert!(!tokio::fs::try_exists(directory.path().join("inference/fixture.gguf")).await?);
    assert_eq!(store.traffic(BUCKET)?.attempts, 1);
    Ok(())
}

async fn remove_fixture_weights(app: axum::Router) -> Result<(), Box<dyn Error>> {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};

    for (authorized, expected) in [
        (false, StatusCode::UNAUTHORIZED),
        (true, StatusCode::NO_CONTENT),
        (true, StatusCode::NO_CONTENT),
    ] {
        let mut request = Request::builder()
            .method("DELETE")
            .uri("/v1/model-downloads/fixture-model");
        if authorized {
            request = request.header("Authorization", "Bearer fixture-token");
        }
        let response = app.clone().oneshot(request.body(Body::empty())?).await?;
        assert_eq!(response.status(), expected);
    }
    Ok(())
}

fn fixture(
    directory: &std::path::Path,
    store: &Arc<SqliteStore>,
    addr: SocketAddrV4,
) -> Result<Arc<ModelManagement>, Box<dyn Error>> {
    let gate = OutboundPool::new(store.clone(), Clock::system()).gate(
        BUCKET,
        QuotaPolicy {
            concurrency: NonZeroU16::MIN,
            spacing: Duration::from_millis(80),
        },
    )?;
    fixture_models(directory, gate, addr, &[(PROFILE, "fixture.gguf")])
}

fn fixture_models(
    directory: &std::path::Path,
    gate: Arc<OutboundGate>,
    addr: SocketAddrV4,
    entries: &[(&'static str, &'static str)],
) -> Result<Arc<ModelManagement>, Box<dyn Error>> {
    let url: hyper::Uri = format!("http://{addr}/model").parse()?;
    let urls = entries
        .iter()
        .map(|(profile, _)| (*profile, url.clone()))
        .collect();
    let catalog = entries
        .iter()
        .map(|(profile, file)| Artifact {
            profile: (*profile).into(),
            label: None,
            kind: None,
            source: None,
            file: (*file).into(),
            remote_file: None,
            bytes: 11,
            sha256: SHA256.into(),
        })
        .collect();
    let adapter = HuggingFace::fixture(directory.join("inference"), gate, addr, urls)?;
    Ok(Arc::new(ModelManagement::new(catalog, Arc::new(adapter))))
}

async fn terminal(downloads: &ModelManagement) -> Result<Snapshot, Box<dyn Error>> {
    Ok(tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let status = downloads.snapshot().await?.remove(0);
            if status.state != State::Installing {
                return Ok::<_, DownloadError>(status);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await??)
}

#[tokio::test]
async fn separate_profiles_share_one_inflight_slot_through_the_body() -> Result<(), Box<dyn Error>>
{
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let store = Arc::new(SqliteStore::open(
        &directory.path().join("context.sqlite3"),
    )?);
    let gate = OutboundPool::new(store.clone(), Clock::system()).gate(
        BUCKET,
        QuotaPolicy {
            concurrency: NonZeroU16::MIN,
            spacing: Duration::from_millis(20),
        },
    )?;
    let downloads = fixture_models(
        directory.path(),
        gate,
        addr,
        &[("first", "first.gguf"), ("second", "second.gguf")],
    )?;
    downloads.request("first")?;
    downloads.request("second")?;
    let (mut first, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept()).await??;
    read_request(&mut first).await?;
    first
        .write_all(b"HTTP/1.1 200 OK\r\nETag: \"fixture-v1\"\r\nContent-Length: 11\r\n\r\nhello")
        .await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(150), listener.accept())
            .await
            .is_err()
    );
    assert_eq!(store.traffic(BUCKET)?.attempts, 1);
    assert_eq!(store.traffic(BUCKET)?.pending, 1);
    assert_eq!(downloads.remove("first"), Err(DownloadError::InUse));
    first.write_all(b" world").await?;
    first.shutdown().await?;
    let (mut second, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept()).await??;
    read_request(&mut second).await?;
    second
        .write_all(
            b"HTTP/1.1 200 OK\r\nETag: \"fixture-v1\"\r\nContent-Length: 11\r\n\r\nhello world",
        )
        .await?;
    second.shutdown().await?;
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let states = downloads.snapshot().await?;
            if states.iter().all(|model| model.state == State::Installed) {
                return Ok::<_, DownloadError>(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await??;
    assert_eq!(store.traffic(BUCKET)?.attempts, 2);
    assert_eq!(store.traffic(BUCKET)?.pending, 0);
    Ok(())
}

#[tokio::test]
async fn removal_protects_active_inference_and_updates_the_generation_catalog()
-> Result<(), Box<dyn Error>> {
    use ctxfab_inference::generation::{
        ALTERNATE_FILE, ALTERNATE_PROFILE, GENERATION_FILE, GENERATION_PROFILE,
    };
    use ctxfab_server::runtime::inference::Inference;

    let directory = tempfile::tempdir()?;
    let assets = directory.path().join("inference");
    std::fs::create_dir(&assets)?;
    for name in ["llama-server", GENERATION_FILE, ALTERNATE_FILE] {
        std::fs::write(assets.join(name), BYTES)?;
    }
    let db = Arc::new(SqliteStore::open(
        &directory.path().join("context.sqlite3"),
    )?);
    let inference = Arc::new(Inference::configured(&assets, &db));
    let outbound = OutboundPool::new(db.clone(), Clock::system());
    let gate = outbound.gate(
        BUCKET,
        QuotaPolicy {
            concurrency: NonZeroU16::MIN,
            spacing: Duration::from_secs(1),
        },
    )?;
    let (catalog, source) = HuggingFace::production(assets.clone(), gate)?;
    let downloads = ModelManagement::new(catalog, Arc::new(source)).with_usage(inference.clone());
    let current = downloads.snapshot().await?;
    let active = current
        .iter()
        .find(|item| item.profile == GENERATION_PROFILE)
        .ok_or("active profile")?;
    assert!(active.deletion_block.is_some());
    assert_eq!(
        downloads.remove(GENERATION_PROFILE),
        Err(DownloadError::InUse)
    );
    assert!(assets.join(GENERATION_FILE).is_file());
    downloads.remove(ALTERNATE_PROFILE)?;
    assert!(!assets.join(ALTERNATE_FILE).exists());
    let generation = inference.answers.as_ref().ok_or("answers")?.models()?;
    assert_eq!(
        generation.active_profile.as_deref(),
        Some(GENERATION_PROFILE)
    );
    assert_eq!(generation.profiles.len(), 1);
    assert_eq!(db.traffic(BUCKET)?.attempts, 0);
    Ok(())
}

#[tokio::test]
async fn hub_catalog_is_exposed_only_to_authenticated_clients() -> Result<(), Box<dyn Error>> {
    use axum::{body::Body, http::Request};
    let database = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let app = http::router_with_services(database, "fixture-token", http::Services::default());
    let request = Request::builder()
        .uri("/v1/model-catalog?query=Qwen3")
        .body(Body::empty())?;
    assert_eq!(app.oneshot(request).await?.status(), 401);
    Ok(())
}

#[tokio::test]
async fn verified_embedding_install_registers_without_restarting_the_service()
-> Result<(), Box<dyn Error>> {
    use ctxfab_inference::{PROFILE as EMBEDDING_PROFILE, supervisor::EMBEDDING_FILE};
    use ctxfab_server::domain::model::ArtifactKind;
    use ctxfab_server::runtime::inference::Inference;
    let directory = tempfile::tempdir()?;
    let assets = directory.path().join("inference");
    std::fs::create_dir(&assets)?;
    std::fs::write(assets.join("llama-server"), b"fixture runtime")?;
    let store = Arc::new(SqliteStore::open(
        &directory.path().join("context.sqlite3"),
    )?);
    let inference = Arc::new(Inference::configured(&assets, &store));
    let retrieval = inference.retrieval.as_ref().ok_or("stable retrieval")?;
    assert_eq!(retrieval.progress()?.state, "notConfigured");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let origin = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let _ = read_request(&mut socket).await?;
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nETag: \"fixture-v1\"\r\nContent-Length: 11\r\n\r\nhello world",
            )
            .await?;
        socket.shutdown().await
    });
    let gate = OutboundPool::new(store, Clock::system()).gate(
        BUCKET,
        QuotaPolicy {
            concurrency: NonZeroU16::MIN,
            spacing: Duration::from_millis(1),
        },
    )?;
    let artifact = Artifact {
        profile: EMBEDDING_PROFILE.into(),
        label: None,
        kind: Some(ArtifactKind::Embedding),
        source: None,
        file: EMBEDDING_FILE.into(),
        remote_file: None,
        bytes: 11,
        sha256: SHA256.into(),
    };
    let adapter = HuggingFace::fixture(
        assets.clone(),
        gate,
        addr,
        [(EMBEDDING_PROFILE, format!("http://{addr}/model").parse()?)]
            .into_iter()
            .collect(),
    )?;
    let downloads = Arc::new(
        ModelManagement::new(vec![artifact], Arc::new(adapter)).with_usage(inference.clone()),
    );
    downloads.request(EMBEDDING_PROFILE)?;
    assert_eq!(terminal(&downloads).await?.state, State::Installed);
    origin.await??;
    assert_eq!(std::fs::read(assets.join(EMBEDDING_FILE))?, BYTES);
    assert_eq!(
        retrieval.progress()?.state,
        "notConfigured",
        "unwarmed worker must not be reported ready"
    );
    downloads.remove(EMBEDDING_PROFILE)?;
    assert!(!assets.join(EMBEDDING_FILE).exists());
    assert_eq!(retrieval.progress()?.state, "notConfigured");
    downloads.stop().await;
    inference.shutdown();
    Ok(())
}

#[tokio::test]
async fn paused_download_survives_restart_and_resumes_only_on_request() -> Result<(), Box<dyn Error>>
{
    use tokio::sync::{Notify, oneshot};
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("context.sqlite3");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let (sent, received) = oneshot::channel();
    let release = Arc::new(Notify::new());
    let origin_release = release.clone();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await?;
        assert!(read_request(&mut first).await?.starts_with("GET /model "));
        first
            .write_all(
                b"HTTP/1.1 200 OK\r\nETag: \"fixture-v1\"\r\nContent-Length: 11\r\n\r\nhello",
            )
            .await?;
        let _ = sent.send(());
        origin_release.notified().await;
        drop(first);
        let (mut second, _) = listener.accept().await?;
        let request = read_request(&mut second).await?;
        assert!(request.lines().any(|line| line == "range: bytes=5-"));
        assert!(request.contains("if-range: \"fixture-v1\"\r\n"));
        second.write_all(b"HTTP/1.1 206 Partial Content\r\nETag: \"fixture-v1\"\r\nContent-Length: 6\r\nContent-Range: bytes 5-10/11\r\n\r\n world").await?;
        second.shutdown().await
    });
    let store = Arc::new(SqliteStore::open(&database)?);
    let downloads = durable_fixture(directory.path(), store.clone(), addr)?;
    downloads.request(PROFILE)?;
    received.await?;
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if downloads.snapshot().await?[0].downloaded_bytes == 5 {
                return Ok::<_, DownloadError>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    assert_eq!(
        downloads.discard_partial(PROFILE),
        Err(DownloadError::InUse)
    );
    assert_eq!(
        control_request(&store, &downloads, "pause").await?,
        axum::http::StatusCode::ACCEPTED
    );
    let paused = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let state = downloads.snapshot().await?.remove(0);
            if state.state == State::Paused {
                return Ok::<_, DownloadError>(state);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    assert_eq!(paused.downloaded_bytes, 5);
    assert!(
        directory
            .path()
            .join("inference/.fixture.gguf.resume")
            .exists()
    );
    let id = paused.operation_id.ok_or("operation ID")?;
    assert_eq!(downloads.remove(PROFILE), Err(DownloadError::InUse));
    release.notify_one();
    downloads.stop().await;
    drop(downloads);
    drop(store);
    let store = Arc::new(SqliteStore::open(&database)?);
    let restarted = durable_fixture(directory.path(), store.clone(), addr)?;
    let restored = restarted.snapshot().await?.remove(0);
    assert_eq!(restored.state, State::Paused);
    assert_eq!(restored.operation_id.as_deref(), Some(id.as_str()));
    assert_eq!(store.traffic(BUCKET)?.attempts, 1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(store.traffic(BUCKET)?.attempts, 1);
    assert_eq!(
        control_request(&store, &restarted, "resume").await?,
        axum::http::StatusCode::ACCEPTED
    );
    assert_eq!(terminal(&restarted).await?.state, State::Installed);
    assert!(
        !directory
            .path()
            .join("inference/.fixture.gguf.resume")
            .exists()
    );
    server.await??;
    assert_eq!(
        std::fs::read(directory.path().join("inference/fixture.gguf"))?,
        BYTES
    );
    assert_eq!(store.traffic(BUCKET)?.attempts, 2);
    restarted.stop().await;
    Ok(())
}

async fn control_request(
    store: &Arc<SqliteStore>,
    downloads: &Arc<ModelManagement>,
    command: &str,
) -> Result<axum::http::StatusCode, Box<dyn Error>> {
    let app = http::router_with_services(
        store.clone(),
        "token",
        http::Services {
            downloads: Some(downloads.clone()),
            ..http::Services::default()
        },
    );
    let request = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/v1/model-downloads/{PROFILE}/{command}"))
        .header("Authorization", "Bearer token")
        .body(axum::body::Body::empty())?;
    Ok(app.oneshot(request).await?.status())
}

fn durable_fixture(
    directory: &std::path::Path,
    store: Arc<SqliteStore>,
    addr: SocketAddrV4,
) -> Result<Arc<ModelManagement>, Box<dyn Error>> {
    let gate = OutboundPool::new(store.clone(), Clock::system()).gate(
        BUCKET,
        QuotaPolicy {
            concurrency: NonZeroU16::MIN,
            spacing: Duration::from_millis(1),
        },
    )?;
    let adapter = HuggingFace::fixture(
        directory.join("inference"),
        gate,
        addr,
        [(PROFILE, format!("http://{addr}/model").parse()?)]
            .into_iter()
            .collect(),
    )?;
    Ok(Arc::new(
        ModelManagement::new(vec![durable_artifact()], Arc::new(adapter)).with_jobs(store)?,
    ))
}

fn durable_artifact() -> Artifact {
    Artifact {
        profile: PROFILE.into(),
        label: None,
        kind: None,
        source: None,
        file: "fixture.gguf".into(),
        remote_file: None,
        bytes: 11,
        sha256: SHA256.into(),
    }
}

struct OutcomeOnPause(DownloadError);

impl ArtifactStore for OutcomeOnPause {
    fn install<'a>(&'a self, _artifact: &'a Artifact) -> ArtifactResult<'a, ()> {
        Box::pin(std::future::ready(Err(DownloadError::Unavailable)))
    }

    fn install_controlled<'a>(
        &'a self,
        _artifact: &'a Artifact,
        mut pause: tokio::sync::watch::Receiver<bool>,
    ) -> ArtifactResult<'a, ()> {
        Box::pin(async move {
            pause
                .wait_for(|requested| *requested)
                .await
                .map_err(|_| DownloadError::Unavailable)?;
            Err(self.0)
        })
    }

    fn local_state<'a>(
        &'a self,
        _artifact: &'a Artifact,
    ) -> ArtifactResult<'a, LocalArtifactState> {
        Box::pin(std::future::ready(Ok(LocalArtifactState {
            installed: false,
            staged_bytes: 0,
        })))
    }

    fn remove(&self, _artifact: &Artifact) -> Result<(), DownloadError> {
        Err(DownloadError::Unavailable)
    }

    fn discard_partial(&self, _artifact: &Artifact) -> Result<(), DownloadError> {
        Err(DownloadError::Unavailable)
    }
}

#[tokio::test]
async fn pause_preserves_the_install_outcome_across_restart() -> Result<(), Box<dyn Error>> {
    for outcome in [
        DownloadError::Integrity,
        DownloadError::StorageLimit,
        DownloadError::Unavailable,
        DownloadError::Transfer,
        DownloadError::Paused,
    ] {
        let (state, phase, code) = if outcome == DownloadError::Paused {
            (State::Paused, "paused", None)
        } else {
            (State::Failed, "failed", Some(outcome.code()))
        };
        let directory = tempfile::tempdir()?;
        let database = directory.path().join("context.sqlite3");
        let adapter = Arc::new(OutcomeOnPause(outcome));
        let store = Arc::new(SqliteStore::open(&database)?);
        let downloads = Arc::new(
            ModelManagement::new(vec![durable_artifact()], adapter.clone())
                .with_jobs(store.clone())?,
        );
        downloads.request(PROFILE)?;
        let id = downloads.snapshot().await?[0]
            .operation_id
            .clone()
            .ok_or("operation ID")?;
        downloads.pause(PROFILE)?;
        let completed = pause_completion(&downloads).await?;
        assert_eq!(completed.state, state, "installer returned {outcome:?}");
        assert_eq!(completed.error, code);
        assert_eq!(completed.operation_id.as_deref(), Some(id.as_str()));

        downloads.stop().await;
        drop(downloads);
        drop(store);
        let reopened = Arc::new(SqliteStore::open(&database)?);
        let persisted = reopened.load_download_jobs()?.remove(0);
        assert_eq!(persisted.phase, phase);
        assert_eq!(persisted.error.as_deref(), code);
        let restored =
            ModelManagement::new(vec![durable_artifact()], adapter).with_jobs(reopened)?;
        let snapshot = restored.snapshot().await?.remove(0);
        assert_eq!(snapshot.state, state);
        assert_eq!(snapshot.error, code);
        assert_eq!(snapshot.operation_id.as_deref(), Some(id.as_str()));
    }
    Ok(())
}

async fn pause_completion(downloads: &ModelManagement) -> Result<Snapshot, Box<dyn Error>> {
    Ok(tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let snapshot = downloads.snapshot().await?.remove(0);
            if matches!(snapshot.state, State::Paused | State::Failed) {
                return Ok::<_, DownloadError>(snapshot);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??)
}

#[tokio::test]
async fn pause_after_dispatch_distinguishes_network_loss_from_provider_rejection()
-> Result<(), Box<dyn Error>> {
    for (reject, expected, code) in [
        (false, State::Paused, None),
        (true, State::Failed, Some(DownloadError::Transfer.code())),
    ] {
        let directory = tempfile::tempdir()?;
        let store = Arc::new(SqliteStore::open(
            &directory.path().join("context.sqlite3"),
        )?);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
        let (dispatched, request_seen) = tokio::sync::oneshot::channel();
        let (release, pause_requested) = tokio::sync::oneshot::channel();
        let origin = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await?;
                assert!(read_request(&mut socket).await?.starts_with("GET /model "));
                socket
                    .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await?;
                socket.shutdown().await?;
            }
            let (mut socket, _) = listener.accept().await?;
            assert!(read_request(&mut socket).await?.starts_with("GET /model "));
            assert!(dispatched.send(()).is_ok());
            assert!(pause_requested.await.is_ok());
            if reject {
                socket
                    .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                    .await?;
            }
            socket.shutdown().await
        });
        let downloads = durable_fixture(directory.path(), store.clone(), addr)?;
        downloads.request(PROFILE)?;
        tokio::time::timeout(Duration::from_secs(4), request_seen).await??;
        downloads.pause(PROFILE)?;
        assert_eq!(downloads.snapshot().await?[0].state, State::Pausing);
        assert!(release.send(()).is_ok());
        let completed = pause_completion(&downloads).await?;
        assert_eq!(completed.state, expected, "provider rejected: {reject}");
        assert_eq!(completed.error, code);
        origin.await??;
        let traffic = store.traffic(BUCKET)?;
        assert_eq!(traffic.attempts, 3);
        assert_eq!(traffic.failures, 3);
        assert_eq!(traffic.pending, 0);
    }
    Ok(())
}

#[tokio::test]
async fn interrupted_download_recovers_without_implicit_network_and_checks_fingerprint()
-> Result<(), Box<dyn Error>> {
    use ctxfab_server::application::model_management::{DownloadJobStore, StoredDownload};
    let directory = tempfile::tempdir()?;
    let store = Arc::new(SqliteStore::open(&directory.path().join("jobs.sqlite3"))?);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let job = StoredDownload {
        profile: PROFILE.into(),
        fingerprint: SHA256.into(),
        operation_id: "00112233-4455-4677-8899-aabbccddeeff".into(),
        phase: "running".into(),
        error: None,
    };
    store.save_download_job(&job)?;
    let gate = OutboundPool::new(store.clone(), Clock::system()).gate(
        BUCKET,
        QuotaPolicy {
            concurrency: NonZeroU16::MIN,
            spacing: Duration::from_millis(1),
        },
    )?;
    let adapter = HuggingFace::fixture(
        directory.path().join("inference"),
        gate,
        addr,
        [(PROFILE, format!("http://{addr}/model").parse()?)]
            .into_iter()
            .collect(),
    )?;
    let artifact = Artifact {
        profile: PROFILE.into(),
        label: None,
        kind: None,
        source: None,
        file: "fixture.gguf".into(),
        remote_file: None,
        bytes: 11,
        sha256: SHA256.into(),
    };
    let downloads =
        ModelManagement::new(vec![artifact.clone()], Arc::new(adapter)).with_jobs(store.clone())?;
    let restored = downloads.snapshot().await?.remove(0);
    assert_eq!(restored.state, State::Interrupted);
    assert_eq!(
        restored.operation_id.as_deref(),
        Some(job.operation_id.as_str())
    );
    assert_eq!(store.load_download_jobs()?[0].phase, "interrupted");
    assert_eq!(store.traffic(BUCKET)?.attempts, 0);
    downloads.stop().await;
    let mismatched = StoredDownload {
        fingerprint: "0".repeat(64),
        ..job
    };
    store.save_download_job(&mismatched)?;
    let gate = OutboundPool::new(store.clone(), Clock::system()).gate(
        BUCKET,
        QuotaPolicy {
            concurrency: NonZeroU16::MIN,
            spacing: Duration::from_millis(1),
        },
    )?;
    let adapter = HuggingFace::fixture(
        directory.path().join("inference"),
        gate,
        addr,
        [(PROFILE, format!("http://{addr}/model").parse()?)]
            .into_iter()
            .collect(),
    )?;
    let stale = ModelManagement::new(vec![artifact], Arc::new(adapter)).with_jobs(store.clone())?;
    assert_eq!(stale.snapshot().await?[0].state, State::NotInstalled);
    assert_eq!(store.traffic(BUCKET)?.attempts, 0);
    stale.stop().await;
    Ok(())
}

#[tokio::test]
async fn discard_paused_partial_preserves_installed_weights_and_reconciles_intent()
-> Result<(), Box<dyn Error>> {
    use ctxfab_server::application::model_management::{DownloadJobStore, StoredDownload};
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir()?;
    let assets = directory.path().join("inference");
    std::fs::create_dir(&assets)?;
    let partial = assets.join(".fixture.gguf.part");
    let installed = assets.join("fixture.gguf");
    let unrelated = directory.path().join("unrelated");
    std::fs::write(&installed, BYTES)?;
    std::fs::write(&unrelated, b"preserve")?;
    symlink(&unrelated, &partial)?;
    let store = Arc::new(SqliteStore::open(&directory.path().join("jobs.sqlite3"))?);
    store.save_download_job(&StoredDownload {
        profile: PROFILE.into(),
        fingerprint: SHA256.into(),
        operation_id: "00112233-4455-4677-8899-aabbccddeeff".into(),
        phase: "paused".into(),
        error: None,
    })?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let downloads = durable_fixture(directory.path(), store.clone(), addr)?;
    let request = || {
        axum::http::Request::builder()
            .method("DELETE")
            .uri(format!("/v1/model-downloads/{PROFILE}/partial"))
            .header("Authorization", "Bearer token")
            .body(axum::body::Body::empty())
    };
    let app = || {
        http::router_with_services(
            store.clone(),
            "token",
            http::Services {
                downloads: Some(downloads.clone()),
                ..http::Services::default()
            },
        )
    };
    assert_eq!(
        app().oneshot(request()?).await?.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(std::fs::read(&unrelated)?, b"preserve");
    assert_eq!(store.load_download_jobs()?.len(), 1);
    std::fs::remove_file(&partial)?;
    std::fs::write(&partial, b"hello")?;
    let database = rusqlite::Connection::open(directory.path().join("jobs.sqlite3"))?;
    database.execute_batch("CREATE TRIGGER reject_job_delete BEFORE DELETE ON model_download_jobs BEGIN SELECT RAISE(FAIL, 'fixture'); END;")?;
    assert_eq!(
        app().oneshot(request()?).await?.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(!partial.exists());
    assert_eq!(std::fs::read(&installed)?, BYTES);
    assert_eq!(store.load_download_jobs()?.len(), 1);
    database.execute_batch("DROP TRIGGER reject_job_delete;")?;
    assert_eq!(
        app().oneshot(request()?).await?.status(),
        axum::http::StatusCode::NO_CONTENT
    );
    assert!(!partial.exists());
    assert_eq!(std::fs::read(&installed)?, BYTES);
    assert!(store.load_download_jobs()?.is_empty());
    assert_eq!(downloads.snapshot().await?[0].state, State::Installed);
    assert_eq!(store.traffic(BUCKET)?.attempts, 0);
    downloads.stop().await;
    Ok(())
}

#[tokio::test]
async fn discard_interrupted_partial_does_not_start_a_transfer() -> Result<(), Box<dyn Error>> {
    use ctxfab_server::application::model_management::{DownloadJobStore, StoredDownload};
    let directory = tempfile::tempdir()?;
    let assets = directory.path().join("inference");
    std::fs::create_dir(&assets)?;
    let partial = assets.join(".fixture.gguf.part");
    std::fs::write(&partial, b"hello")?;
    let marker = assets.join(".fixture.gguf.resume");
    std::fs::write(&marker, b"stale marker")?;
    let store = Arc::new(SqliteStore::open(&directory.path().join("jobs.sqlite3"))?);
    store.save_download_job(&StoredDownload {
        profile: PROFILE.into(),
        fingerprint: SHA256.into(),
        operation_id: "00112233-4455-4677-8899-aabbccddeeff".into(),
        phase: "interrupted".into(),
        error: None,
    })?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let downloads = durable_fixture(directory.path(), store.clone(), addr)?;
    assert_eq!(downloads.snapshot().await?[0].state, State::Interrupted);
    downloads.discard_partial(PROFILE)?;
    assert!(!partial.exists());
    assert!(!marker.exists());
    assert_eq!(downloads.snapshot().await?[0].state, State::NotInstalled);
    assert!(store.load_download_jobs()?.is_empty());
    assert_eq!(store.traffic(BUCKET)?.attempts, 0);
    downloads.stop().await;
    Ok(())
}

#[tokio::test]
async fn legacy_partial_is_restarted_without_a_range() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let assets = directory.path().join("inference");
    std::fs::create_dir_all(&assets)?;
    std::fs::write(assets.join(".fixture.gguf.part"), b"hello")?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await?;
        let request = read_request(&mut socket).await?;
        assert!(!request.contains("range:"), "{request}");
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world")
            .await?;
        socket.shutdown().await
    });
    let store = Arc::new(SqliteStore::open(
        &directory.path().join("context.sqlite3"),
    )?);
    let downloads = fixture(directory.path(), &store, addr)?;
    downloads.request(PROFILE)?;
    assert_eq!(terminal(&downloads).await?.state, State::Installed);
    server.await??;
    assert_eq!(std::fs::read(assets.join("fixture.gguf"))?, BYTES);
    Ok(())
}

#[tokio::test]
async fn unsatisfied_resume_retries_one_counted_full_request() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let server = tokio::spawn(async move {
        for step in 0..3 {
            let (mut socket, _) = listener.accept().await?;
            let request = read_request(&mut socket).await?;
            if step == 1 {
                assert!(request.contains("range: bytes=5-\r\n"));
                assert!(request.contains("if-range: \"fixture-v1\"\r\n"));
            } else {
                assert!(!request.contains("range:"));
            }
            let response = match step {
                0 => "HTTP/1.1 200 OK\r\nETag: \"fixture-v1\"\r\nContent-Length: 11\r\n\r\nhello",
                1 => "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\r\n",
                _ => "HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world",
            };
            socket.write_all(response.as_bytes()).await?;
            socket.shutdown().await?;
        }
        Ok::<_, std::io::Error>(())
    });
    let store = Arc::new(SqliteStore::open(
        &directory.path().join("context.sqlite3"),
    )?);
    let downloads = fixture(directory.path(), &store, addr)?;
    downloads.request(PROFILE)?;
    assert_eq!(terminal(&downloads).await?.state, State::Installed);
    server.await??;
    assert_eq!(store.traffic(BUCKET)?.attempts, 3);
    Ok(())
}

#[tokio::test]
async fn resumed_response_with_changed_etag_retries_full() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let server = tokio::spawn(async move {
        for step in 0..3 {
            let (mut socket, _) = listener.accept().await?;
            let request = read_request(&mut socket).await?;
            let response = if step == 0 {
                "HTTP/1.1 200 OK\r\nETag: \"fixture-v1\"\r\nContent-Length: 11\r\n\r\nhello"
            } else if step == 1 {
                assert!(request.contains("if-range: \"fixture-v1\"\r\n"));
                "HTTP/1.1 206 Partial Content\r\nETag: \"fixture-v2\"\r\nContent-Range: bytes 5-10/11\r\nContent-Length: 6\r\n\r\n world"
            } else {
                assert!(!request.contains("range:"));
                assert!(!request.contains("if-range:"));
                "HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world"
            };
            socket.write_all(response.as_bytes()).await?;
            socket.shutdown().await?;
        }
        Ok::<_, std::io::Error>(())
    });
    let store = Arc::new(SqliteStore::open(
        &directory.path().join("context.sqlite3"),
    )?);
    let downloads = fixture(directory.path(), &store, addr)?;
    downloads.request(PROFILE)?;
    assert_eq!(terminal(&downloads).await?.state, State::Installed);
    server.await??;
    assert_eq!(
        std::fs::read(directory.path().join("inference/fixture.gguf"))?,
        BYTES
    );
    let traffic = store.traffic(BUCKET)?;
    assert_eq!(traffic.attempts, 3);
    assert_eq!(traffic.failures, 2);
    assert_eq!(traffic.pending, 0);
    Ok(())
}

#[tokio::test]
async fn retry_gets_a_fresh_redirect_budget() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let server = tokio::spawn(async move {
        for attempt in 0..2 {
            for hop in 0..5 {
                let (mut socket, _) = listener.accept().await?;
                let request = read_request(&mut socket).await?;
                let path = if hop == 0 {
                    "/model".to_owned()
                } else {
                    format!("/r{hop}")
                };
                assert!(request.starts_with(&format!("GET {path} ")), "{request}");
                let response = if hop < 4 {
                    format!(
                        "HTTP/1.1 302 Found\r\nLocation: /r{}\r\nContent-Length: 0\r\n\r\n",
                        hop + 1
                    )
                } else if attempt == 0 {
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n".to_owned()
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world".to_owned()
                };
                socket.write_all(response.as_bytes()).await?;
                socket.shutdown().await?;
            }
        }
        Ok::<_, std::io::Error>(())
    });
    let store = Arc::new(SqliteStore::open(
        &directory.path().join("context.sqlite3"),
    )?);
    let downloads = fixture(directory.path(), &store, addr)?;
    downloads.request(PROFILE)?;
    assert_eq!(terminal(&downloads).await?.state, State::Installed);
    server.await??;
    let traffic = store.traffic(BUCKET)?;
    assert_eq!(traffic.attempts, 10);
    assert_eq!(traffic.failures, 1);
    assert_eq!(traffic.pending, 0);
    Ok(())
}

#[tokio::test]
async fn managed_removal_checks_identity_and_recovers_without_network() -> Result<(), Box<dyn Error>>
{
    use ctxfab_server::adapters::outbound::artifact_storage::DesktopArtifactCapacityProbe;
    use ctxfab_server::application::artifact_storage::ArtifactStorageBudget;
    use ctxfab_server::application::model_management::{DownloadJobStore, RemovalIntent};
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    std::fs::create_dir(root.join("inference"))?;
    let file = root.join("inference/fixture.gguf");
    std::fs::write(&file, BYTES)?;
    let store = Arc::new(SqliteStore::open(&root.join("jobs.sqlite3"))?);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let artifact = Artifact {
        profile: PROFILE.into(),
        label: None,
        kind: None,
        source: None,
        file: "fixture.gguf".into(),
        remote_file: None,
        bytes: 11,
        sha256: SHA256.into(),
    };
    let make = || -> Result<ModelManagement, Box<dyn Error>> {
        let gate = OutboundPool::new(store.clone(), Clock::system()).gate(
            BUCKET,
            QuotaPolicy {
                concurrency: NonZeroU16::MIN,
                spacing: Duration::from_millis(1),
            },
        )?;
        let budget = ArtifactStorageBudget::new(DesktopArtifactCapacityProbe::new(
            root.to_owned(),
            1_000_000,
            0,
            8,
        ));
        let adapter = HuggingFace::fixture(
            root.join("inference"),
            gate,
            addr,
            std::collections::HashMap::default(),
        )?
        .with_managed_weights(store.clone(), budget);
        Ok(
            ModelManagement::new(vec![artifact.clone()], Arc::new(adapter))
                .with_jobs(store.clone())?,
        )
    };
    let downloads = make()?;
    let stale = downloads.removal_plan(PROFILE)?;
    std::fs::write(&file, b"replacement")?;
    assert_eq!(
        downloads.remove_guarded(PROFILE, Some(&stale.identity)),
        Err(DownloadError::StaleRemoval)
    );
    assert!(file.exists());
    let current = downloads.removal_plan(PROFILE)?;
    assert_eq!(current.bytes, 11);
    let confirmed = current.identity.clone();
    store.save_removal(&RemovalIntent {
        profile: PROFILE.into(),
        fingerprint: SHA256.into(),
        identity: current.identity,
    })?;
    drop(downloads);
    let recovered = make()?;
    assert!(!file.exists());
    recovered.remove_guarded(PROFILE, Some(&confirmed))?;
    assert!(store.load_removals()?.is_empty());
    assert_eq!(store.traffic(BUCKET)?.attempts, 0);
    drop(recovered);
    std::fs::write(&file, BYTES)?;
    let plan = make()?.removal_plan(PROFILE)?;
    store.save_removal(&RemovalIntent {
        profile: PROFILE.into(),
        fingerprint: SHA256.into(),
        identity: plan.identity,
    })?;
    std::fs::remove_file(&file)?;
    std::fs::write(&file, b"new payload")?;
    let recovered = make()?;
    assert_eq!(std::fs::read(&file)?, b"new payload");
    assert!(store.load_removals()?.is_empty());
    assert_eq!(store.traffic(BUCKET)?.attempts, 0);
    drop(recovered);
    Ok(())
}

struct Active;
impl ctxfab_server::application::model_management::ModelUsage for Active {
    fn deletion_block(&self, _: &str) -> Result<Option<&'static str>, DownloadError> {
        Ok(Some("active"))
    }
    fn with_removal(
        &self,
        _: &str,
        _: &dyn Fn() -> Result<(), DownloadError>,
    ) -> Result<(), DownloadError> {
        Err(DownloadError::InUse)
    }
}

#[tokio::test]
async fn removal_recovery_defers_active_target_and_preview_checks_identity()
-> Result<(), Box<dyn Error>> {
    use ctxfab_server::adapters::outbound::artifact_storage::DesktopArtifactCapacityProbe;
    use ctxfab_server::application::artifact_storage::ArtifactStorageBudget;
    use ctxfab_server::application::model_management::{
        DownloadJobStore, RemovalIntent, StoredDownload,
    };
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    std::fs::create_dir(root.join("inference"))?;
    let file = root.join("inference/fixture.gguf");
    std::fs::write(&file, BYTES)?;
    let store = Arc::new(SqliteStore::open(&root.join("jobs.sqlite3"))?);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let preview = durable_fixture(root, store.clone(), addr)?;
    let stale = preview.removal_plan(PROFILE)?;
    std::fs::write(&file, b"replacement")?;
    assert_eq!(
        preview.remove_guarded(PROFILE, Some(&stale.identity)),
        Err(DownloadError::StaleRemoval)
    );
    assert!(file.exists());
    let plan = preview.removal_plan(PROFILE)?;
    store.save_removal(&RemovalIntent {
        profile: PROFILE.into(),
        fingerprint: SHA256.into(),
        identity: plan.identity,
    })?;
    store.save_download_job(&StoredDownload {
        profile: PROFILE.into(),
        fingerprint: SHA256.into(),
        operation_id: "00112233-4455-4677-8899-aabbccddeeff".into(),
        phase: "failed".into(),
        error: None,
    })?;
    let gate = OutboundPool::new(store.clone(), Clock::system()).gate(
        BUCKET,
        QuotaPolicy {
            concurrency: NonZeroU16::MIN,
            spacing: Duration::from_millis(1),
        },
    )?;
    let budget = ArtifactStorageBudget::new(DesktopArtifactCapacityProbe::new(
        root.to_owned(),
        1_000_000,
        0,
        8,
    ));
    let adapter = HuggingFace::fixture(
        root.join("inference"),
        gate,
        addr,
        std::collections::HashMap::new(),
    )?
    .with_managed_weights(store.clone(), budget);
    let artifact = Artifact {
        profile: PROFILE.into(),
        label: None,
        kind: None,
        source: None,
        file: "fixture.gguf".into(),
        remote_file: None,
        bytes: 11,
        sha256: SHA256.into(),
    };
    let recovered = ModelManagement::new(vec![artifact.clone()], Arc::new(adapter))
        .with_usage(Arc::new(Active))
        .with_jobs(store.clone())?;
    assert!(file.exists());
    assert_eq!(store.load_removals()?.len(), 1);
    assert_eq!(store.load_download_jobs()?.len(), 1);
    drop(recovered);
    std::fs::remove_file(file)?;
    let gate = OutboundPool::new(store.clone(), Clock::system()).gate(
        BUCKET,
        QuotaPolicy {
            concurrency: NonZeroU16::MIN,
            spacing: Duration::from_millis(1),
        },
    )?;
    let adapter = HuggingFace::fixture(
        root.join("inference"),
        gate,
        addr,
        std::collections::HashMap::new(),
    )?;
    let recovered = ModelManagement::new(vec![artifact], Arc::new(adapter))
        .with_usage(Arc::new(Active))
        .with_jobs(store.clone())?;
    assert!(store.load_removals()?.is_empty());
    assert!(store.load_download_jobs()?.is_empty());
    assert_eq!(store.traffic(BUCKET)?.attempts, 0);
    drop(recovered);
    Ok(())
}
