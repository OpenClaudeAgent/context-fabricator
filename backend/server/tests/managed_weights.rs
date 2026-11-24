use std::borrow::Cow::Borrowed;
use std::error::Error;
use std::fs;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use ctxfab_inference::{
    generation::{GENERATION_FILE, GENERATION_PROFILE, Generation},
    supervisor::Runtime,
};
use ctxfab_server::adapters::inbound::http;
use ctxfab_server::adapters::outbound::{
    artifact_storage::{DesktopArtifactBudget, DesktopArtifactCapacityProbe},
    hugging_face::{BUCKET, HuggingFace},
    sqlite::{SqliteStore, verified_weights::guardian_weight},
};
use ctxfab_server::application::{
    model_management::{ArtifactStore, ModelManagement},
    sync::TrafficLedger,
};
use ctxfab_server::domain::model::{Artifact, ArtifactKind, DownloadError, InstallationStage};
use ctxfab_server::runtime::{
    clock::Clock,
    outbound::{OutboundPool, QuotaPolicy},
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tower::ServiceExt;

const WEIGHT: Artifact = Artifact {
    profile: Borrowed(GENERATION_PROFILE),
    label: None,
    kind: Some(ArtifactKind::Generation),
    source: None,
    file: Borrowed(GENERATION_FILE),
    remote_file: None,
    bytes: 11,
    sha256: Borrowed("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"),
};

fn adapter(
    support: &Path,
    store: &Arc<SqliteStore>,
    listener: &TcpListener,
    budget: DesktopArtifactBudget,
) -> Result<HuggingFace, Box<dyn Error>> {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
    let gate = OutboundPool::new(store.clone(), Clock::system()).gate(
        BUCKET,
        QuotaPolicy {
            concurrency: NonZeroU16::MIN,
            spacing: Duration::ZERO,
        },
    )?;
    Ok(HuggingFace::fixture(
        support.join("inference"),
        gate,
        address,
        [(
            GENERATION_PROFILE,
            format!("http://{address}/weight").parse()?,
        )]
        .into(),
    )?
    .with_managed_weights(store.clone(), budget))
}

#[tokio::test]
async fn installed_receipt_survives_restart_but_refuses_rewrites_and_symlinks()
-> Result<(), Box<dyn Error>> {
    let support = tempfile::tempdir()?;
    let assets = support.path().join("inference");
    let model = assets.join(GENERATION_FILE);
    fs::create_dir(&assets)?;
    fs::write(&model, b"hello world")?;
    let database = support.path().join("context.sqlite3");
    let store = Arc::new(SqliteStore::open(&database)?);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let budget = DesktopArtifactBudget::new(DesktopArtifactCapacityProbe::new(
        support.path().to_owned(),
        100,
        0,
        16,
    ));
    let weights = adapter(support.path(), &store, &listener, budget)?;
    assert!(!weights.local_state(&WEIGHT).await?.installed);
    assert!(!guardian_weight(&model, GENERATION_PROFILE)?);
    let (_pause, pause) = tokio::sync::watch::channel(false);
    let (stage, observed) = tokio::sync::watch::channel(InstallationStage::Preparing);
    weights.install_tracked(&WEIGHT, pause, stage).await?;
    assert_eq!(*observed.borrow(), InstallationStage::Verifying);
    assert_eq!(weights.local_state(&WEIGHT).await?.staged_bytes, 0);
    weights.install(&WEIGHT).await?;
    assert_eq!(store.traffic(BUCKET)?.attempts, 0);
    assert!(guardian_weight(&model, GENERATION_PROFILE)?);
    assert!(!store.accepts_verified_weight(&model, GENERATION_PROFILE, Some("different pin")));

    let reopened = Arc::new(SqliteStore::open(&database)?);
    let runtime = Arc::new(Runtime::with_weight_policy(move |path, profile| {
        reopened.accepts_verified_weight(path, profile, None)
    }));
    let binary = support.path().join("fixture-runtime");
    fs::write(&binary, [])?;
    runtime
        .publish(binary)
        .map_err(|_| "publish fixture failed")?;
    let engine = Generation::with_runtime(&assets, Vec::new(), None, runtime.clone());
    assert_eq!(engine.models()?.profiles.len(), 1);
    fs::write(&model, b"other bytes")?;
    assert!(!weights.local_state(&WEIGHT).await?.installed);
    assert!(!guardian_weight(&model, GENERATION_PROFILE)?);
    assert!(engine.models()?.profiles.is_empty());
    assert!(engine.models()?.active_profile.is_none());
    assert!(
        Generation::with_runtime(&assets, Vec::new(), None, runtime)
            .models()?
            .profiles
            .is_empty()
    );
    fs::write(&model, b"hello world")?;
    weights.install(&WEIGHT).await?;
    assert_eq!(
        engine.models()?.active_profile.as_deref(),
        Some(GENERATION_PROFILE)
    );

    let staged = support.path().join("unmanaged.gguf");
    fs::write(&staged, b"hello world")?;
    fs::remove_file(&model)?;
    std::os::unix::fs::symlink(staged, &model)?;
    assert!(!guardian_weight(&model, GENERATION_PROFILE)?);
    assert!(!weights.local_state(&WEIGHT).await?.installed);
    Ok(())
}

#[tokio::test]
async fn shared_reservation_refuses_weight_before_network_and_retry_installs()
-> Result<(), Box<dyn Error>> {
    let support = tempfile::tempdir()?;
    let database = support.path().join("context.sqlite3");
    let store = Arc::new(SqliteStore::open(&database)?);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let probe = DesktopArtifactCapacityProbe::new(support.path().to_owned(), 20, 0, 16);
    let budget = DesktopArtifactBudget::new(probe);
    let runtime_reservation = budget
        .try_reserve(10)
        .map_err(|_| "runtime admission failed")?;
    let weights = adapter(support.path(), &store, &listener, budget)?;
    assert_eq!(
        weights.install(&WEIGHT).await,
        Err(DownloadError::StorageLimit)
    );
    assert_eq!(store.traffic(BUCKET)?.attempts, 0);
    assert_eq!(weights.local_state(&WEIGHT).await?.staged_bytes, 0);

    drop(runtime_reservation);
    let server = tokio::spawn(async move {
        let (mut peer, _) = listener.accept().await?;
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(peer.read_u8().await?);
        }
        peer.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello world",
        )
        .await?;
        peer.shutdown().await
    });
    weights.install(&WEIGHT).await?;
    tokio::time::timeout(Duration::from_secs(3), server).await???;
    let path = support.path().join("inference").join(GENERATION_FILE);
    assert!(guardian_weight(&path, GENERATION_PROFILE)?);
    assert_eq!(store.traffic(BUCKET)?.attempts, 1);
    assert_eq!(fs::read(&path)?, b"hello world");
    Ok(())
}

async fn maintenance(
    app: &Router,
    method: &str,
    path: &str,
    payload: Option<Value>,
) -> Result<(StatusCode, Value), Box<dyn Error>> {
    let request = Request::builder()
        .uri(path)
        .method(method)
        .header("authorization", "Bearer fixture")
        .header("content-type", "application/json")
        .body(payload.map_or_else(Body::empty, |value| Body::from(value.to_string())))?;
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 65536).await?;
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)?
    };
    Ok((status, value))
}

#[tokio::test]
async fn model_plan_credits_partial_bytes_and_guarded_api_deletes_only_confirmed_weight()
-> Result<(), Box<dyn Error>> {
    let support = tempfile::tempdir()?;
    let store = Arc::new(SqliteStore::open(&support.path().join("context.sqlite3"))?);
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let assets = support.path().join("inference");
    fs::create_dir(&assets)?;
    fs::write(assets.join(format!(".{GENERATION_FILE}.part")), b"hell")?;
    let budget = DesktopArtifactBudget::new(DesktopArtifactCapacityProbe::new(
        support.path().to_owned(),
        100,
        0,
        16,
    ));
    let weights = Arc::new(adapter(support.path(), &store, &listener, budget.clone())?);
    let downloads = Arc::new(
        ModelManagement::new(vec![WEIGHT.clone()], weights.clone()).with_jobs(store.clone())?,
    );
    let app = http::router_with_services(
        store.clone(),
        "fixture",
        http::Services {
            downloads: Some(downloads),
            artifact_storage: Some(Arc::new(budget)),
            ..http::Services::default()
        },
    );
    let storage = "/v1/inference/storage";
    let file = assets.join(GENERATION_FILE);
    let model = format!("/v1/model-downloads/{GENERATION_PROFILE}");
    let plan = format!("{model}/removal-plan");

    assert_eq!(
        maintenance(&app, "GET", storage, None).await?.1["models"][0]["peakBytes"],
        7
    );
    assert_eq!(
        maintenance(&app, "GET", &plan, None).await?.0,
        StatusCode::CONFLICT
    );
    fs::write(&file, b"hello world")?;
    weights.install(&WEIGHT).await?;
    assert_eq!(
        maintenance(&app, "GET", storage, None).await?.1["models"][0]["peakBytes"],
        0
    );
    assert_eq!(
        maintenance(&app, "DELETE", &model, None).await?.0,
        StatusCode::CONFLICT
    );
    let (status, review) = maintenance(&app, "GET", &plan, None).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(review["bytes"], 11);
    assert_eq!(
        maintenance(&app, "DELETE", &model, Some(json!({"identity":"stale"})))
            .await?
            .0,
        StatusCode::CONFLICT
    );
    assert!(file.is_file());
    let confirmation = json!({"identity":review["identity"]});
    for _ in 0..2 {
        assert_eq!(
            maintenance(&app, "DELETE", &model, Some(confirmation.clone()))
                .await?
                .0,
            StatusCode::NO_CONTENT
        );
    }
    assert!(!file.exists());
    assert_eq!(store.traffic(BUCKET)?.attempts, 0);
    Ok(())
}

#[tokio::test]
async fn storage_report_preserves_probe_failure_as_a_typed_api_error() -> Result<(), Box<dyn Error>>
{
    let support = tempfile::tempdir()?;
    let capacity = DesktopArtifactBudget::new(DesktopArtifactCapacityProbe::new(
        support.path().join("missing"),
        100,
        0,
        16,
    ));
    let app = http::router_with_services(
        Arc::new(SqliteStore::open(Path::new(":memory:"))?),
        "fixture",
        http::Services {
            artifact_storage: Some(Arc::new(capacity)),
            ..http::Services::default()
        },
    );
    let (code, error) = maintenance(&app, "GET", "/v1/inference/storage", None).await?;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["code"], "inference_storage_unavailable");
    Ok(())
}
