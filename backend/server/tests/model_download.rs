use std::error::Error;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::num::NonZeroU16;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ctxfab_server::adapters::{inbound::http, outbound::sqlite::SqliteStore};
use ctxfab_server::application::sync::TrafficLedger;
use ctxfab_server::runtime::{
    clock::Clock,
    model_downloads::{Artifact, BUCKET, ModelDownloads, Scope, Snapshot, State},
    outbound::{OutboundPool, QuotaPolicy},
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
                "HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello",
                None,
            ),
            (
                "GET /model ",
                "HTTP/1.1 206 Partial Content\r\nContent-Length: 6\r\nContent-Range: bytes 5-10/11\r\n\r\n world",
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
    let directory = tempfile::tempdir()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await?;
        assert!(read_request(&mut first).await?.starts_with("GET /model "));
        first
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello")
            .await?;
        let mut eof = [0];
        assert_eq!(first.read(&mut eof).await?, 0);

        let (mut second, _) = listener.accept().await?;
        assert!(
            read_request(&mut second)
                .await?
                .contains("range: bytes=5-\r\n")
        );
        second.write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 5-10/11\r\nContent-Length: 6\r\n\r\n world").await?;
        second.shutdown().await
    });

    let database = directory.path().join("context.sqlite3");
    let store = Arc::new(SqliteStore::open(&database)?);
    let first = fixture(directory.path(), &store, addr)?;
    first.request(PROFILE)?;
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if first.snapshot().await?.remove(0).downloaded_bytes == 5 {
                return Ok::<_, ctxfab_server::runtime::model_downloads::DownloadError>(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await??;
    assert_eq!(store.traffic(BUCKET)?.pending, 1);
    first.stop().await;
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
    let resumed = fixture(directory.path(), &store, addr)?;
    resumed.request(PROFILE)?;
    assert_eq!(terminal(&resumed).await?.state, State::Installed);
    tokio::time::timeout(Duration::from_secs(2), server).await???;
    assert_eq!(store.traffic(BUCKET)?.attempts, 2);
    assert_eq!(store.traffic(BUCKET)?.uncertain, 1);
    assert_eq!(store.traffic(BUCKET)?.pending, 0);
    Ok(())
}

#[tokio::test]
async fn refuses_invalid_weights_and_off_scope_redirects() -> Result<(), Box<dyn Error>> {
    for (response, code) in [
        (
            "HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nHELLO WORLD",
            "model_integrity_failed",
        ),
        (
            "HTTP/1.1 302 Found\r\nLocation: https://example.invalid/model\r\nContent-Length: 0\r\n\r\n",
            "model_transfer_failed",
        ),
        (
            "HTTP/1.1 206 Partial Content\r\nContent-Length: 6\r\nContent-Range: bytes 5-10/11\r\n\r\n world",
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
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world")
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
    Ok(())
}

fn fixture(
    directory: &std::path::Path,
    store: &Arc<SqliteStore>,
    addr: SocketAddrV4,
) -> Result<Arc<ModelDownloads>, Box<dyn Error>> {
    let gate = OutboundPool::new(store.clone(), Clock::system()).gate(
        BUCKET,
        QuotaPolicy {
            concurrency: NonZeroU16::MIN,
            spacing: Duration::from_millis(80),
        },
    )?;
    Ok(Arc::new(ModelDownloads::new(
        directory.join("inference"),
        gate,
        Scope::Loopback(addr),
        vec![Artifact {
            profile: PROFILE,
            file: "fixture.gguf",
            bytes: 11,
            sha256: SHA256,
            url: format!("http://{addr}/model").parse()?,
        }],
    )?))
}

async fn terminal(downloads: &ModelDownloads) -> Result<Snapshot, Box<dyn Error>> {
    Ok(tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let status = downloads.snapshot().await?.remove(0);
            if status.state != State::Installing {
                return Ok::<_, ctxfab_server::runtime::model_downloads::DownloadError>(status);
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
    let url: hyper::Uri = format!("http://{addr}/model").parse()?;
    let downloads = Arc::new(ModelDownloads::new(
        directory.path().join("inference"),
        gate,
        Scope::Loopback(addr),
        [("first", "first.gguf"), ("second", "second.gguf")]
            .into_iter()
            .map(|(profile, file)| Artifact {
                profile,
                file,
                bytes: 11,
                sha256: SHA256,
                url: url.clone(),
            })
            .collect(),
    )?);
    downloads.request("first")?;
    downloads.request("second")?;
    let (mut first, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept()).await??;
    read_request(&mut first).await?;
    first
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello")
        .await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(150), listener.accept())
            .await
            .is_err()
    );
    assert_eq!(store.traffic(BUCKET)?.attempts, 1);
    assert_eq!(store.traffic(BUCKET)?.pending, 1);
    first.write_all(b" world").await?;
    first.shutdown().await?;
    let (mut second, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept()).await??;
    read_request(&mut second).await?;
    second
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world")
        .await?;
    second.shutdown().await?;
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let states = downloads.snapshot().await?;
            if states.iter().all(|model| model.state == State::Installed) {
                return Ok::<_, ctxfab_server::runtime::model_downloads::DownloadError>(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await??;
    assert_eq!(store.traffic(BUCKET)?.attempts, 2);
    assert_eq!(store.traffic(BUCKET)?.pending, 0);
    Ok(())
}
