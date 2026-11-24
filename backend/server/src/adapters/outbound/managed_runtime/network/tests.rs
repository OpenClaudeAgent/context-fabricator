use super::*;
use crate::application::sync::TrafficLedger;
use crate::{
    adapters::outbound::sqlite::SqliteStore,
    runtime::{
        clock::Clock,
        outbound::{OutboundPool, QuotaPolicy},
    },
};
use std::{num::NonZeroU16, os::unix::fs::PermissionsExt};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn run_script(responses: &'static [&'static str], expected_size: u64, expect_success: bool) {
    let temp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = SocketAddrV4::new(
        std::net::Ipv4Addr::LOCALHOST,
        listener.local_addr().unwrap().port(),
    );
    let server = tokio::spawn(async move {
        let mut observed = Vec::with_capacity(responses.len());
        for response in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            observed.push(tokio::time::Instant::now());
            let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
            assert!(request.starts_with("get /"));
            assert!(!request.contains("authorization:"));
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        }
        observed
    });
    let store = Arc::new(SqliteStore::open(&temp.path().join("state.sqlite")).unwrap());
    let pool = OutboundPool::new(store.clone(), Clock::system());
    let gate = pool
        .gate(
            BUCKET,
            QuotaPolicy {
                concurrency: NonZeroU16::new(1).unwrap(),
                spacing: Duration::from_millis(80),
            },
        )
        .unwrap();
    let downloader = RuntimeDownload::with_scope(gate, Scope::Loopback(address)).unwrap();
    let result = downloader
        .download(
            &format!("http://{address}/release"),
            expected_size,
            temp.path(),
        )
        .await;
    assert_eq!(result.is_ok(), expect_success);
    if let Ok(archive) = result {
        assert_eq!(std::fs::read(archive.path()).unwrap(), b"data");
        assert_eq!(
            archive.file().metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        let staged = archive.path().to_path_buf();
        drop(archive);
        assert!(!staged.exists());
    }
    let observed = timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    assert!(
        observed
            .windows(2)
            .all(|pair| pair[1] - pair[0] >= Duration::from_millis(50))
    );
    assert_eq!(
        store.traffic(BUCKET).unwrap().attempts,
        i64::try_from(responses.len()).unwrap()
    );
    assert_eq!(store.traffic(BUCKET).unwrap().pending, 0);
}
fn contention_server(
    listener: TcpListener,
    requests_tx: tokio::sync::mpsc::UnboundedSender<()>,
    release_rx: tokio::sync::oneshot::Receiver<()>,
    pause_weight: bool,
    cancel_first: bool,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let mut buf = [0; 1024];
        let _ = first.read(&mut buf).await.unwrap();
        requests_tx.send(()).unwrap();
        tokio::select! {
            _ = release_rx => {}
            incoming = listener.accept() => {
                let _ = incoming.unwrap();
                requests_tx.send(()).unwrap();
                return;
            }
        }
        if !cancel_first {
            first
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ndata")
                .await
                .unwrap();
            first.shutdown().await.unwrap();
        }
        if !pause_weight {
            let (mut second, _) = listener.accept().await.unwrap();
            let _ = second.read(&mut buf).await.unwrap();
            requests_tx.send(()).unwrap();
            second
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ndata")
                .await
                .unwrap();
            second.shutdown().await.unwrap();
        }
    })
}

fn contention_model() -> crate::domain::model::Artifact {
    crate::domain::model::Artifact {
        profile: "test".into(),
        label: None,
        kind: None,
        source: None,
        file: "test.gguf".into(),
        remote_file: None,
        bytes: 4,
        sha256: "3a6eb0790f39ac87c94f3856b2dd2c5d110e6811602261a9a923d3bb23adc8b7".into(),
    }
}

fn immediate_policy() -> QuotaPolicy {
    QuotaPolicy {
        concurrency: NonZeroU16::MIN,
        spacing: Duration::ZERO,
    }
}

fn contention_adapters(
    root: &Path,
    pool: &OutboundPool,
    address: SocketAddrV4,
) -> (
    Arc<RuntimeDownload>,
    Arc<crate::adapters::outbound::hugging_face::HuggingFace>,
) {
    use crate::adapters::outbound::hugging_face::{self, HuggingFace};
    let slot = Arc::new(tokio::sync::Semaphore::new(1));
    let runtime = Arc::new(
        RuntimeDownload::with_scope(
            pool.gate(BUCKET, immediate_policy()).unwrap(),
            Scope::Loopback(address),
        )
        .unwrap()
        .with_transfer_slot(slot.clone()),
    );
    let weights = Arc::new(
        HuggingFace::fixture(
            root.join("inference"),
            pool.gate(hugging_face::BUCKET, immediate_policy()).unwrap(),
            address,
            [("test", format!("http://{address}/model").parse().unwrap())]
                .into_iter()
                .collect(),
        )
        .unwrap()
        .with_transfer_slot(slot),
    );
    (runtime, weights)
}

fn assert_waiting_traffic(store: &SqliteStore, runtime_first: bool) {
    let bucket = if runtime_first {
        crate::adapters::outbound::hugging_face::BUCKET
    } else {
        BUCKET
    };
    let traffic = store.traffic(bucket).unwrap();
    assert_eq!(traffic.attempts, 0);
    assert_eq!(traffic.pending, 0);
}

async fn contention(runtime_first: bool, pause_weight: bool, cancel_first: bool) {
    use crate::adapters::outbound::hugging_face;
    use crate::application::model_management::ArtifactStore;
    use crate::domain::model::DownloadError;
    let temp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = SocketAddrV4::new(
        std::net::Ipv4Addr::LOCALHOST,
        listener.local_addr().unwrap().port(),
    );
    let (requests_tx, mut requests_rx) = tokio::sync::mpsc::unbounded_channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = contention_server(
        listener,
        requests_tx,
        release_rx,
        pause_weight,
        cancel_first,
    );
    let store = Arc::new(SqliteStore::open(&temp.path().join("state.sqlite")).unwrap());
    let pool = OutboundPool::new(store.clone(), Clock::system());
    let (runtime, weights) = contention_adapters(temp.path(), &pool, address);
    let model = contention_model();
    let (pause_tx, pause_rx) = tokio::sync::watch::channel(false);
    let runtime_task = || {
        let runtime = runtime.clone();
        let root = temp.path().to_owned();
        tokio::spawn(async move {
            runtime
                .download(&format!("http://{address}/release"), 4, &root)
                .await
                .map(|_| ())
        })
    };
    let weight_task = || {
        let weights = weights.clone();
        let model = model.clone();
        let pause = pause_rx.clone();
        tokio::spawn(async move { weights.install_controlled(&model, pause).await })
    };
    let first_runtime = runtime_first.then(runtime_task);
    let first_weight = (!runtime_first).then(weight_task);
    timeout(Duration::from_secs(3), requests_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let mut second_weight = runtime_first.then(weight_task);
    let second_runtime = (!runtime_first).then(runtime_task);
    assert!(
        timeout(Duration::from_millis(150), requests_rx.recv())
            .await
            .is_err()
    );
    assert_waiting_traffic(&store, runtime_first);
    if pause_weight {
        pause_tx.send(true).unwrap();
        assert_eq!(
            second_weight.take().unwrap().await.unwrap(),
            Err(DownloadError::Paused)
        );
    }
    if cancel_first {
        first_runtime.as_ref().unwrap().abort();
    }
    release_tx.send(()).unwrap();
    if let Some(task) = first_runtime {
        if cancel_first {
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            task.await.unwrap().unwrap();
        }
    }
    if let Some(task) = first_weight {
        task.await.unwrap().unwrap();
    }
    if !pause_weight {
        timeout(Duration::from_secs(3), requests_rx.recv())
            .await
            .unwrap()
            .unwrap();
        if let Some(task) = second_runtime {
            task.await.unwrap().unwrap();
        }
        if let Some(task) = second_weight {
            task.await.unwrap().unwrap();
        }
    }
    server.await.unwrap();
    assert_eq!(
        store.traffic(hugging_face::BUCKET).unwrap().attempts,
        i64::from(!pause_weight)
    );
    assert_eq!(store.traffic(BUCKET).unwrap().attempts, 1);
}
#[tokio::test]
async fn shared_slot_serializes_runtime_and_weights_in_both_orders() {
    contention(true, false, false).await;
    contention(false, false, false).await;
}

#[tokio::test]
async fn cancellation_releases_slot_for_waiting_weight() {
    contention(true, false, true).await;
}

#[tokio::test]
async fn queued_weight_pause_does_not_send_a_request() {
    contention(true, true, false).await;
}

fn prime_bucket(root: &Path, cooled: &str) {
    let database = rusqlite::Connection::open(root.join("state.sqlite")).unwrap();
    database
        .execute(
            "INSERT OR IGNORE INTO outbound_buckets (bucket) VALUES (?1)",
            [cooled],
        )
        .unwrap();
    drop(database);
}

fn cooldown_server(listener: TcpListener) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut data = [0; 1024];
        assert!(socket.read(&mut data).await.unwrap() > 0);
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ndata")
            .await
            .unwrap();
    })
}

async fn cooldown_does_not_block_other_provider(cool_weight: bool) {
    use crate::adapters::outbound::hugging_face;
    use crate::application::model_management::ArtifactStore;
    use crate::domain::model::DownloadError;
    let temp = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = SocketAddrV4::new(
        std::net::Ipv4Addr::LOCALHOST,
        listener.local_addr().unwrap().port(),
    );
    let server = cooldown_server(listener);
    let store = Arc::new(SqliteStore::open(&temp.path().join("state.sqlite")).unwrap());
    let pool = OutboundPool::new(store.clone(), Clock::system());
    let cooled = if cool_weight {
        hugging_face::BUCKET
    } else {
        BUCKET
    };
    let gate = pool.gate(cooled, immediate_policy()).unwrap();
    prime_bucket(temp.path(), cooled);
    gate.pause(Duration::from_secs(60)).unwrap();
    let (runtime, weights) = contention_adapters(temp.path(), &pool, address);
    let (pause_tx, pause_rx) = tokio::sync::watch::channel(false);
    let root = temp.path().to_owned();
    let start_runtime = || {
        let runtime = runtime.clone();
        let root = root.clone();
        tokio::spawn(async move {
            runtime
                .download(&format!("http://{address}/release"), 4, &root)
                .await
                .map(|_| ())
        })
    };
    let start_weight = || {
        let weights = weights.clone();
        let pause_rx = pause_rx.clone();
        tokio::spawn(async move {
            weights
                .install_controlled(&contention_model(), pause_rx)
                .await
        })
    };
    let (mut runtime_task, mut weight_task) = if cool_weight {
        (None, Some(start_weight()))
    } else {
        (Some(start_runtime()), None)
    };
    timeout(Duration::from_secs(3), async {
        while gate.queued() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    if cool_weight {
        runtime_task = Some(start_runtime());
    } else {
        weight_task = Some(start_weight());
    }
    let runtime_task = runtime_task.unwrap();
    let weight_task = weight_task.unwrap();
    timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
    let traffic = store.traffic(cooled).unwrap();
    assert_eq!(traffic.attempts, 0);
    assert_eq!(traffic.pending, 0);
    if cool_weight {
        timeout(Duration::from_secs(3), runtime_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!weight_task.is_finished());
        pause_tx.send(true).unwrap();
        assert_eq!(
            timeout(Duration::from_secs(1), weight_task)
                .await
                .unwrap()
                .unwrap(),
            Err(DownloadError::Paused)
        );
    } else {
        timeout(Duration::from_secs(3), weight_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        runtime_task.abort();
        assert!(runtime_task.await.unwrap_err().is_cancelled());
    }
}

#[tokio::test]
async fn provider_cooldown_does_not_block_other_artifact_transfer() {
    cooldown_does_not_block_other_provider(true).await;
    cooldown_does_not_block_other_provider(false).await;
}

#[tokio::test]
async fn redirect_and_retry_are_each_counted() {
    run_script(
        &[
            "HTTP/1.1 302 Found\r\nLocation: /blob\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 302 Found\r\nLocation: /blob\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ndata",
        ],
        4,
        true,
    )
    .await;
}
#[tokio::test]
async fn rejects_wrong_length_and_off_scope_redirect() {
    run_script(
        &["HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ndata"],
        5,
        false,
    )
    .await;
    run_script(
        &["HTTP/1.1 302 Found\r\nLocation: https://example.com/asset\r\nContent-Length: 0\r\n\r\n"],
        4,
        false,
    )
    .await;
}
#[test]
fn release_url_validation_accepts_only_github_release_zip_paths() {
    assert!(
        validate_release_url("https://github.com/org/project/releases/download/v1.2/runtime.zip")
            .is_ok()
    );
    for invalid in [
        "http://github.com/org/project/releases/download/v1/runtime.zip",
        "https://github.com.evil.example/org/project/releases/download/v1/runtime.zip",
        "https://github.com/org/project/releases/download/latest/runtime.zip",
        "https://github.com/org/project/releases/download/v1/runtime.zip?token=secret",
        "https://github.com/org/project/releases/download/v1/%2e%2e.zip",
        "https://github.com/org/project/releases/download/v1/runtime.tar",
    ] {
        assert!(validate_release_url(invalid).is_err(), "{invalid}");
    }
}
