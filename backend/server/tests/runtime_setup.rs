use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use ctxfab_inference::{
    generation::{GENERATION_FILE, GENERATION_PROFILE},
    supervisor::Runtime,
};
use ctxfab_server::{
    adapters::{
        inbound::http,
        outbound::{
            artifact_storage::{DesktopArtifactBudget, DesktopArtifactCapacityProbe},
            sqlite::SqliteStore,
        },
    },
    application::runtime_setup::{Package, PackageDownload, RuntimeSetup},
    runtime::inference::Inference,
};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tower::ServiceExt;

struct FixturePackage {
    archive: PathBuf,
    binary: PathBuf,
    hosted: Option<Arc<Hosted>>,
}

#[derive(Default)]
struct Hosted {
    attempts: AtomicUsize,
    entered: Notify,
    proceed: Notify,
    ended: Notify,
}

struct DownloadEnd<'a>(&'a Notify);

impl Drop for DownloadEnd<'_> {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

impl Package for FixturePackage {
    fn archive_bytes(&self) -> u64 {
        123
    }

    fn can_download(&self) -> bool {
        self.hosted.is_some()
    }

    fn storage_peaks(&self) -> Option<(u64, Option<u64>)> {
        Some((246, self.can_download().then_some(369)))
    }

    fn download(&self) -> PackageDownload<'_> {
        Box::pin(async {
            let hosted = self.hosted.as_ref().ok_or("No test release")?;
            let _end = DownloadEnd(&hosted.ended);
            let attempt = hosted.attempts.fetch_add(1, Ordering::SeqCst);
            hosted.entered.notify_one();
            hosted.proceed.notified().await;
            if attempt == 0 {
                Err("private download failure".into())
            } else {
                Ok(self.binary.clone())
            }
        })
    }

    fn install(&self, archive: &Path) -> Result<PathBuf, String> {
        if archive == self.archive {
            Ok(self.binary.clone())
        } else {
            Err("private verifier detail".into())
        }
    }
}

async fn request(
    app: &Router,
    path: &str,
    body: Option<Value>,
    authenticated: bool,
) -> Result<(StatusCode, Value), Box<dyn Error>> {
    let mut request = Request::builder().uri(path);
    if authenticated {
        request = request.header("authorization", "Bearer session-capability");
    }
    let body = if let Some(value) = body {
        request = request
            .method("POST")
            .header("content-type", "application/json");
        Body::from(value.to_string())
    } else {
        Body::empty()
    };
    send(app, request.body(body)?).await
}

async fn send(app: &Router, request: Request<Body>) -> Result<(StatusCode, Value), Box<dyn Error>> {
    let response = app.clone().oneshot(request).await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}

struct Fixture {
    app: Router,
    setup: Arc<RuntimeSetup>,
    archive: PathBuf,
    runtime: Arc<Runtime>,
    activations: Arc<AtomicUsize>,
    budget: DesktopArtifactBudget,
}

fn fixture(root: &Path, hosted: Option<Arc<Hosted>>) -> Result<Fixture, Box<dyn Error>> {
    let assets = root.join("inference");
    std::fs::create_dir(&assets)?;
    std::fs::write(assets.join(GENERATION_FILE), b"fixture weights")?;
    let archive = root.join("candidate.zip");
    let binary = root.join("llama-server");
    std::fs::write(&binary, b"fixture executable")?;
    let store = Arc::new(SqliteStore::open(&root.join("context.sqlite3"))?);
    let runtime = Arc::new(Runtime::default());
    let inference = Arc::new(Inference::managed(
        &assets,
        &store,
        Vec::new(),
        runtime.clone(),
    ));
    let activations = Arc::new(AtomicUsize::new(0));
    let activation_count = activations.clone();
    let running = inference.clone();
    let setup = Arc::new(RuntimeSetup::new(
        Box::new(FixturePackage {
            archive: archive.clone(),
            binary,
            hosted,
        }),
        runtime.clone(),
        move || {
            activation_count.fetch_add(1, Ordering::SeqCst);
            running.runtime_published();
        },
    ));
    let budget = DesktopArtifactBudget::new(DesktopArtifactCapacityProbe::new(
        root.to_owned(),
        400,
        0,
        64,
    ));
    let app = http::router_with_services(
        store,
        "session-capability",
        http::Services {
            answers: inference.answers.clone(),
            retrieval: inference.retrieval.clone(),
            runtime_setup: Some(setup.clone()),
            artifact_storage: Some(Arc::new(budget.clone())),
            ..http::Services::default()
        },
    );
    Ok(Fixture {
        app,
        setup,
        archive,
        runtime,
        activations,
        budget,
    })
}

#[tokio::test]
async fn storage_plan_tracks_live_reservations_and_requires_session_authentication()
-> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let Fixture { app, budget, .. } = fixture(root.path(), Some(Arc::new(Hosted::default())))?;
    let route = "/v1/inference/storage";
    assert_eq!(
        request(&app, route, None, false).await?.0,
        StatusCode::UNAUTHORIZED
    );
    let reservation = budget
        .try_reserve(50)
        .map_err(|_| "fixture capacity missing")?;
    let (code, plan) = request(&app, route, None, true).await?;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(plan["capacity"]["limitBytes"], 400);
    assert_eq!(plan["capacity"]["usedBytes"], 15);
    assert_eq!(plan["capacity"]["reservedBytes"], 50);
    assert_eq!(plan["capacity"]["availableBytes"], 335);
    assert_eq!(
        plan["runtime"]["localImport"],
        json!({"peakBytes":246,"fits":true})
    );
    assert_eq!(
        plan["runtime"]["hostedDownload"],
        json!({"peakBytes":369,"fits":false})
    );
    drop(reservation);
    assert_eq!(
        request(&app, route, None, true).await?.1["runtime"]["hostedDownload"]["fits"],
        true
    );
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let preview = http::router(store, "session-capability");
    assert_eq!(
        request(&preview, route, None, true).await?,
        (StatusCode::OK, json!({"models":[]}))
    );
    Ok(())
}

#[tokio::test]
async fn verified_install_publishes_existing_model_in_the_same_session()
-> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let Fixture {
        app,
        archive,
        runtime,
        activations,
        ..
    } = fixture(root.path(), None)?;
    let status_path = "/v1/runtime";
    let install_path = "/v1/runtime/install";
    let models_path = "/v1/generation/models";
    let chosen = json!({"archivePath": archive});
    assert_eq!(
        request(&app, install_path, Some(chosen.clone()), false)
            .await?
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&app, status_path, None, true).await?,
        (
            StatusCode::OK,
            json!({"state":"availableForInstall","archiveBytes":123})
        )
    );
    assert_eq!(
        request(&app, models_path, None, true).await?.1["profiles"],
        json!([])
    );
    assert_eq!(
        request(
            &app,
            install_path,
            Some(json!({"archivePath":"relative.zip"})),
            true
        )
        .await?
        .0,
        StatusCode::BAD_REQUEST
    );
    let rejected = request(
        &app,
        install_path,
        Some(json!({"archivePath":"/wrong.zip"})),
        true,
    )
    .await?;
    assert_eq!(
        rejected,
        (
            StatusCode::OK,
            json!({
                "state": "failed", "archiveBytes": 123,
                "error": "The archive could not be installed. Choose the exact runtime package for this app release."
            })
        )
    );
    assert!(!runtime.is_available());

    let installed = request(&app, install_path, Some(chosen.clone()), true).await?;
    assert_eq!(
        installed,
        (StatusCode::OK, json!({"state":"ready","archiveBytes":123}))
    );
    assert_eq!(request(&app, status_path, None, true).await?, installed);
    assert_eq!(
        request(&app, install_path, Some(chosen), true).await?,
        installed
    );
    assert_eq!(activations.load(Ordering::SeqCst), 1);
    assert!(runtime.is_available());
    let (_, plan) = request(&app, "/v1/inference/storage", None, true).await?;
    assert!(plan.get("runtime").is_none());
    let (code, models) = request(&app, models_path, None, true).await?;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(models["activeProfile"], GENERATION_PROFILE);
    assert_eq!(models["profiles"][0]["id"], GENERATION_PROFILE);
    Ok(())
}

async fn request_hosted(
    app: &Router,
    authenticated: bool,
) -> Result<(StatusCode, Value), Box<dyn Error>> {
    let mut request = Request::post("/v1/runtime/download");
    if authenticated {
        request = request.header("authorization", "Bearer session-capability");
    }
    send(app, request.body(Body::empty())?).await
}

async fn wait_for_runtime(app: &Router, state: &str) -> Result<Value, Box<dyn Error>> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        let (code, runtime) = request(app, "/v1/runtime", None, true).await?;
        assert_eq!(code, StatusCode::OK);
        if runtime["state"] == state {
            return Ok(runtime);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("runtime did not reach {state}: {runtime}").into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn hosted_download_is_session_owned_and_retries_after_a_failed_package()
-> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let hosted = Arc::new(Hosted::default());
    let Fixture {
        app,
        archive,
        runtime,
        activations,
        ..
    } = fixture(root.path(), Some(hosted.clone()))?;
    assert_eq!(
        request_hosted(&app, false).await?.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        wait_for_runtime(&app, "availableForInstall").await?["canDownload"],
        true
    );
    let expected = (
        StatusCode::OK,
        json!({"state":"installing","archiveBytes":123,"canDownload":true}),
    );
    assert_eq!(request_hosted(&app, true).await?, expected);
    tokio::time::timeout(std::time::Duration::from_secs(3), hosted.entered.notified()).await?;
    assert_eq!(request_hosted(&app, true).await?, expected);
    assert_eq!(
        request(
            &app,
            "/v1/runtime/install",
            Some(json!({"archivePath":archive})),
            true
        )
        .await?,
        expected
    );
    assert_eq!(hosted.attempts.load(Ordering::SeqCst), 1);
    hosted.proceed.notify_one();
    let failure = wait_for_runtime(&app, "failed").await?;
    assert_eq!(
        failure["error"],
        "The runtime could not be downloaded or verified. Try again or choose the release ZIP."
    );
    assert!(!runtime.is_available());
    assert_eq!(request_hosted(&app, true).await?, expected);
    hosted.proceed.notify_one();
    assert_eq!(wait_for_runtime(&app, "ready").await?["archiveBytes"], 123);
    assert!(runtime.is_available());
    assert_eq!(activations.load(Ordering::SeqCst), 1);
    assert_eq!(hosted.attempts.load(Ordering::SeqCst), 2);
    let models = request(&app, "/v1/generation/models", None, true).await?.1;
    assert_eq!(models["activeProfile"], GENERATION_PROFILE);
    Ok(())
}

#[tokio::test]
async fn stopping_the_session_drops_a_pending_hosted_download() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let hosted = Arc::new(Hosted::default());
    let Fixture {
        app,
        setup,
        runtime,
        activations,
        ..
    } = fixture(root.path(), Some(hosted.clone()))?;
    assert_eq!(request_hosted(&app, true).await?.1["state"], "installing");
    let deadline = std::time::Duration::from_secs(3);
    tokio::time::timeout(deadline, hosted.entered.notified()).await?;
    setup.stop();
    tokio::time::timeout(deadline, hosted.ended.notified()).await?;
    assert!(!runtime.is_available());
    assert_eq!(activations.load(Ordering::SeqCst), 0);
    Ok(())
}
