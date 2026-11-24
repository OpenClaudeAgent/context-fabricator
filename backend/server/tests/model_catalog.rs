use std::error::Error;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::num::NonZeroU16;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::Query;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use ctxfab_server::adapters::{
    inbound::http,
    outbound::{
        hugging_face::{BUCKET, HuggingFace},
        sqlite::SqliteStore,
    },
};
use ctxfab_server::application::model_management::{ModelInventory, ModelManagement};
use ctxfab_server::application::sync::TrafficLedger;
use ctxfab_server::domain::model::{ArtifactKind, State, discovery::managed_embedding_profile};
use ctxfab_server::runtime::{
    clock::Clock,
    outbound::{OutboundPool, QuotaPolicy},
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tower::ServiceExt;

const REPOSITORY: &str = "fixture/catalog";
const REVISION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FILE: &str = "Qwen3-4B-Instruct-2507-Q4_K_M.gguf";
const BYTES: &[u8] = b"GGUF catalog fixture";
const SHA256: &str = "865643e9af19e4d1697faad47c8732d96bba5885bbaa0ad9e91de3aa41c840b8";

const EMBEDDING_REPO: &str = "another-publisher/embedding";
const EMBEDDING_FILE: &str = "Publisher-Embedding-Q8_0.gguf";

const INVALID_FILE: &str = "Broken-Q5_K_M.gguf";
const INVALID_FORMAT: &[u8] = b"Not a GGUF file";
const INVALID_FORMAT_SHA256: &str =
    "9541bf965ca64bb76612fdfb04b83b55b1430291c02fb31a9baf9cce9824d068";

fn embedding_metadata() -> Value {
    json!({
        "id": EMBEDDING_REPO, "sha": REVISION, "private": false, "gated": false,
        "pipeline_tag": "feature-extraction",
        "gguf": {"architecture": "qwen3", "pooling_type": 3},
        "baseModels": {"relation": "quantized", "models": [{"id": "Qwen/Qwen3-Embedding-0.6B"}]},
        "cardData": {"license": "apache-2.0"},
        "tags": ["license:apache-2.0"],
    })
}

fn embedding_search_results() -> Value {
    let valid = embedding_metadata();
    let mut wrong_pooling = valid.clone();
    wrong_pooling["id"] = json!("test/wrong-pooling");
    wrong_pooling["gguf"]["pooling_type"] = json!(1);
    let mut wrong_family = valid.clone();
    wrong_family["id"] = json!("test/wrong-family");
    wrong_family["baseModels"]["models"][0]["id"] = json!("Qwen/Qwen3-Embedding-4B");
    let mut wrong_license = valid.clone();
    wrong_license["id"] = json!("test/wrong-license");
    wrong_license["cardData"]["license"] = json!("cc-by-nc-4.0");
    json!([valid, wrong_pooling, wrong_family, wrong_license])
}

fn embedding_routes() -> Router {
    Router::new()
        .route(
            &format!("/api/models/{EMBEDDING_REPO}"),
            get(|| async { Json(embedding_metadata()) }),
        )
        .route(
            &format!("/api/models/{EMBEDDING_REPO}/revision/{REVISION}"),
            get(|| async { Json(embedding_metadata()) }),
        )
        .route(
            &format!("/api/models/{EMBEDDING_REPO}/tree/{REVISION}"),
            get(|| async {
                Json(json!([{
                    "type": "file", "path": EMBEDDING_FILE, "size": BYTES.len(),
                    "lfs": {"size": BYTES.len(), "oid": SHA256}
                }]))
            }),
        )
        .route(
            &format!("/{EMBEDDING_REPO}/resolve/{REVISION}/{EMBEDDING_FILE}"),
            get(|| async { BYTES }),
        )
}

fn hub_fixture() -> Router {
    let supported = json!({
        "id": REPOSITORY, "sha": REVISION, "private": false, "gated": false,
        "pipeline_tag": "text-generation", "gguf": {"architecture": "qwen3"},
        "baseModels": {"relation": "quantized", "models": [{"id": "Qwen/Qwen3-4B-Instruct-2507"}]}
    });
    let mut unsupported = supported.clone();
    unsupported["id"] = json!("fixture/gated");
    unsupported["gated"] = json!("manual");
    Router::new()
        .route(
            "/api/models",
            get({
                let supported = supported.clone();
                move |Query(query): Query<std::collections::HashMap<String, String>>| async move {
                    match query.get("search").map(String::as_str) {
                        Some("embedding") => Json(embedding_search_results()).into_response(),
                        Some("limit") => (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "1")])
                            .into_response(),
                        Some("invalid-json") => String::from("broken").into_response(),
                        Some("too-large") => "x".repeat(3 * 1024 * 1024).into_response(),
                        Some("unavailable") => StatusCode::SERVICE_UNAVAILABLE.into_response(),
                        _ => Json(json!([supported, unsupported])).into_response(),
                    }
                }
            }),
        )
        .route(
            &format!("/api/models/{REPOSITORY}/revision/{REVISION}"),
            get(move || async move { Json(supported) }),
        )
        .route(
            &format!("/api/models/{REPOSITORY}/tree/{REVISION}"),
            get(|| async {
                Json(json!([
                    {"type": "file", "path": FILE, "size": BYTES.len(), "lfs": {"size": BYTES.len(), "oid": SHA256}},
                    {"type": "file", "path": INVALID_FILE, "size": INVALID_FORMAT.len(), "lfs": {
                        "size": INVALID_FORMAT.len(), "oid": INVALID_FORMAT_SHA256,
                    }},
                    {"type": "file", "path": "Model-Q5_K_M.gguf", "size": 11},
                    {"type": "file", "path": "Mismatched-Q8_0.gguf", "size": 12, "lfs": {"size": 11, "oid": SHA256}},
                    {"type": "file", "path": "Model.gguf", "size": 11, "lfs": {"size": 11, "oid": SHA256}},
                    {"type": "file", "path": "subdir/Unsafe-Q4_K_M.gguf", "size": 11, "lfs": {"size": 11, "oid": SHA256}},
                    {"type": "file", "path": "Unknown-Q8_0.gguf", "size": 11, "lfs": {"size": 11, "oid": "random"}},
                ]))
            }),
        )
        .route(
            &format!("/{REPOSITORY}/resolve/{REVISION}/{FILE}"),
            get(|| async { BYTES }),
        )
        .route(
            &format!("/{REPOSITORY}/resolve/{REVISION}/{INVALID_FILE}"),
            get(|| async { INVALID_FORMAT }),
        )
        .merge(embedding_routes())
}

struct Fixture {
    app: Router,
    downloads: Arc<ModelManagement>,
    store: Arc<SqliteStore>,
    server: tokio::task::AbortHandle,
}

impl Fixture {
    async fn start(directory: &std::path::Path) -> Result<Self, Box<dyn Error>> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener.local_addr()?.port());
        let server = tokio::spawn(async move { axum::serve(listener, hub_fixture()).await });
        let store = Arc::new(SqliteStore::open(&directory.join("context.sqlite3"))?);
        let pool = OutboundPool::new(store.clone(), Clock::system());
        let gate = pool.gate(
            BUCKET,
            QuotaPolicy {
                concurrency: NonZeroU16::MIN,
                spacing: Duration::from_millis(1),
            },
        )?;
        let (catalog, adapter) =
            HuggingFace::discovery_fixture(directory.join("inference"), gate, address)?;
        let adapter = Arc::new(adapter);
        let downloads = Arc::new(
            ModelManagement::new(catalog, adapter.clone()).with_discovery(
                adapter,
                store.clone(),
                store.load_models()?,
            )?,
        );
        let app = http::router_with_services(
            store.clone(),
            "fixture",
            http::Services {
                downloads: Some(downloads.clone()),
                ..http::Services::default()
            },
        );
        Ok(Self {
            app,
            downloads,
            store,
            server: server.abort_handle(),
        })
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(StatusCode, Value), Box<dyn Error>> {
        let request = Request::builder()
            .uri(path)
            .method(method)
            .header("authorization", "Bearer fixture")
            .header("content-type", "application/json")
            .body(body.map_or_else(Body::empty, |value| Body::from(value.to_string())))?;
        body_json(self.app.clone().oneshot(request).await?).await
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn body_json(response: Response) -> Result<(StatusCode, Value), Box<dyn Error>> {
    let status = response.status();
    let body = to_bytes(response.into_body(), 65_536).await?;
    Ok((
        status,
        if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)?
        },
    ))
}

#[tokio::test]
async fn discovery_download_and_deletion_use_one_paced_transport_and_a_durable_inventory()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let fixture = Fixture::start(directory.path()).await?;
    let (status, search) = fixture
        .call("GET", "/v1/model-catalog?query=Qwen%20fixture", None)
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        search,
        json!([{
            "repository": REPOSITORY, "revision": REVISION,
            "url": format!("https://huggingface.co/{REPOSITORY}/tree/{REVISION}")
        }])
    );
    let (status, variants) = fixture
        .call(
            "GET",
            &format!("/v1/model-catalog/variants?repository={REPOSITORY}&revision={REVISION}"),
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        variants,
        json!([
            {"file": INVALID_FILE, "quantization": "Q5_K_M", "bytes": INVALID_FORMAT.len()},
            {"file": FILE, "quantization": "Q4_K_M", "bytes": BYTES.len()},
        ])
    );
    let choice = json!({"repository": REPOSITORY, "revision": REVISION, "file": FILE});
    let (status, accepted) = fixture
        .call("POST", "/v1/model-catalog/downloads", Some(choice))
        .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    let profile = accepted["profile"].as_str().ok_or("no accepted profile")?;
    wait_for_install(&fixture, profile).await?;
    let file = directory
        .path()
        .join("inference")
        .join(format!("{profile}.gguf"));
    assert_eq!(tokio::fs::read(&file).await?, BYTES);
    assert!(fixture.store.load_models()?[0].verified);
    assert_eq!(fixture.store.traffic(BUCKET)?.attempts, 6);
    fixture.downloads.stop().await;
    drop(fixture);

    let reopened = Fixture::start(directory.path()).await?;
    wait_for_install(&reopened, profile).await?;
    let (status, fault) = reopened
        .call(
            "DELETE",
            &format!("/v1/model-downloads/{profile}"),
            Some(json!({"identity": 2})),
        )
        .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(fault["code"], "invalid_model_removal");
    assert!(file.exists());
    let (status, _) = reopened
        .call("DELETE", &format!("/v1/model-downloads/{profile}"), None)
        .await?;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!file.exists());
    let durable = reopened.store.load_models()?;
    assert!(!durable[0].verified);
    assert_eq!(durable[0].artifact.profile, profile);
    assert_eq!(reopened.store.traffic(BUCKET)?.attempts, 6);
    Ok(())
}

async fn wait_for_install(fixture: &Fixture, profile: &str) -> Result<(), Box<dyn Error>> {
    let model = installed_state(fixture, profile).await?;
    assert_eq!(model.state, State::Installed);
    assert_eq!(model.total_bytes, BYTES.len() as u64);
    Ok(())
}

async fn installed_state(
    fixture: &Fixture,
    profile: &str,
) -> Result<ctxfab_server::domain::model::Snapshot, Box<dyn Error>> {
    let snapshots = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let snapshots = fixture.downloads.snapshot().await?;
            if !snapshots
                .iter()
                .any(|model| model.profile == profile && model.state == State::Installing)
            {
                return Ok::<_, ctxfab_server::domain::model::DownloadError>(snapshots);
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await??;
    Ok(snapshots
        .into_iter()
        .find(|model| model.profile == profile)
        .ok_or("discovered model missing")?)
}

#[tokio::test]
async fn invalid_or_unsupported_selections_never_start_a_download() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let fixture = Fixture::start(directory.path()).await?;
    for (path, status) in [
        ("/v1/model-catalog", StatusCode::BAD_REQUEST),
        (
            "/v1/model-catalog?query=%0afoo%0abar",
            StatusCode::BAD_REQUEST,
        ),
        (
            "/v1/model-catalog/variants?repository=../../forge&revision=main",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(fixture.call("GET", path, None).await?.0, status);
    }
    assert_eq!(fixture.store.traffic(BUCKET)?.attempts, 0);
    for file in ["../Model-Q4_K_M.gguf", "Model-Q5_K_M.gguf"] {
        let choice = json!({"repository": REPOSITORY, "revision": REVISION, "file": file});
        let (status, error) = fixture
            .call("POST", "/v1/model-catalog/downloads", Some(choice))
            .await?;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(error["code"], "unsupported_model_artifact");
    }
    assert_eq!(fixture.store.traffic(BUCKET)?.attempts, 2);
    assert!(fixture.store.load_models()?.is_empty());
    assert!(!directory.path().join("inference").exists());
    Ok(())
}

#[tokio::test]
async fn catalog_distinguishes_throttling_upstream_failure_and_malformed_data()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let fixture = Fixture::start(directory.path()).await?;
    for (query, status) in [
        ("invalid-json", StatusCode::SERVICE_UNAVAILABLE),
        ("too-large", StatusCode::SERVICE_UNAVAILABLE),
        ("unavailable", StatusCode::SERVICE_UNAVAILABLE),
        ("limit", StatusCode::TOO_MANY_REQUESTS),
    ] {
        assert_eq!(
            fixture
                .call("GET", &format!("/v1/model-catalog?query={query}"), None)
                .await?
                .0,
            status
        );
    }
    let ledger = fixture.store.traffic(BUCKET)?;
    assert_eq!(ledger.attempts, 4);
    assert_eq!(ledger.failures, 4);
    assert!(ledger.cooldown_until > Clock::system().now());
    Ok(())
}

#[tokio::test]
async fn matching_hash_without_gguf_magic_never_installs() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let fixture = Fixture::start(directory.path()).await?;
    let choice = json!({"repository": REPOSITORY, "revision": REVISION, "file": INVALID_FILE});
    let (status, accepted) = fixture
        .call("POST", "/v1/model-catalog/downloads", Some(choice))
        .await?;
    assert_eq!(status, StatusCode::ACCEPTED);
    let profile = accepted["profile"].as_str().ok_or("no accepted profile")?;
    let model = installed_state(&fixture, profile).await?;
    assert_eq!(model.state, State::Failed);
    assert_eq!(model.error, Some("model_integrity_failed"));
    assert!(!fixture.store.load_models()?[0].verified);
    assert_eq!(fixture.store.traffic(BUCKET)?.attempts, 3);
    assert_eq!(
        std::fs::read_dir(directory.path().join("inference"))?.count(),
        0
    );
    Ok(())
}

#[tokio::test]
async fn embedding_discovery_uses_reviewed_pin_and_preserves_licence_and_profile()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let fixture = Fixture::start(directory.path()).await?;
    let (status, search) = fixture
        .call(
            "GET",
            "/v1/model-catalog?kind=embedding&query=embedding",
            None,
        )
        .await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        search,
        json!([{
            "repository": EMBEDDING_REPO, "revision": REVISION, "kind": "embedding",
            "license": "apache-2.0",
            "url": format!("https://huggingface.co/{EMBEDDING_REPO}/tree/{REVISION}")
        }])
    );
    let exact = format!(
        "/v1/model-catalog?kind=embedding&query=https%3A%2F%2Fhuggingface.co%2F{EMBEDDING_REPO}"
    );
    assert_eq!(
        fixture.call("GET", &exact, None).await?,
        (StatusCode::OK, search)
    );
    let (status, files) = fixture.call(
        "GET",
        &format!("/v1/model-catalog/variants?kind=embedding&repository={EMBEDDING_REPO}&revision={REVISION}"),
        None,
    ).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        files,
        json!([{"file": EMBEDDING_FILE, "quantization": "Q8_0", "bytes": BYTES.len(), "sha256": SHA256}])
    );

    let route = "/v1/model-catalog/downloads";
    let mut selection = json!({
        "repository": EMBEDDING_REPO, "revision": REVISION, "file": EMBEDDING_FILE, "kind": "embedding",
    });
    assert_eq!(
        fixture
            .call("POST", route, Some(selection.clone()))
            .await?
            .0,
        StatusCode::BAD_REQUEST
    );
    selection["sha256"] = json!("b".repeat(64));
    let (status, problem) = fixture.call("POST", route, Some(selection.clone())).await?;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(problem["code"], "model_catalog_pin_changed");
    assert!(fixture.store.load_models()?.is_empty());
    assert!(!directory.path().join("inference").exists());

    selection["sha256"] = files[0]["sha256"].clone();
    let (status, accepted) = fixture.call("POST", route, Some(selection)).await?;
    let profile = managed_embedding_profile(EMBEDDING_REPO, REVISION, EMBEDDING_FILE, SHA256);
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(accepted["profile"], profile);
    wait_for_install(&fixture, &profile).await?;
    let saved = fixture
        .store
        .load_models()?
        .pop()
        .ok_or("missing retained model")?;
    assert!(saved.verified);
    assert_eq!(saved.artifact.kind, Some(ArtifactKind::Embedding));
    assert_eq!(saved.artifact.sha256, SHA256);
    assert_eq!(
        saved
            .artifact
            .source
            .ok_or("missing provenance")?
            .license
            .as_deref(),
        Some("apache-2.0")
    );
    assert_eq!(
        tokio::fs::read(
            directory
                .path()
                .join("inference")
                .join(format!("{profile}.gguf"))
        )
        .await?,
        BYTES
    );
    fixture.downloads.stop().await;
    drop(fixture);
    let reopened = Fixture::start(directory.path()).await?;
    wait_for_install(&reopened, &profile).await?;
    let (_, catalog) = reopened.call("GET", "/v1/model-downloads", None).await?;
    assert!(
        catalog
            .as_array()
            .ok_or("missing catalog")?
            .iter()
            .any(|row| row["profile"] == profile
                && row["setupRole"] == "optionalEmbedding"
                && row["kind"] == "embedding")
    );
    Ok(())
}
