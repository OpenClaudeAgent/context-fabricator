//! Authenticated, session-owned first-run setup with tiny local artifact adapters.
use std::{
    error::Error,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
};
use ctxfab_inference::{
    ALTERNATE_EMBEDDING_PROFILE, ALTERNATE_EMBEDDING_SHA256, DIMENSIONS, Embedding, PROFILE,
    generation::{
        ALTERNATE_FILE, ALTERNATE_PROFILE, ALTERNATE_SHA256, GENERATION_FILE, GENERATION_PROFILE,
        GENERATION_SHA256, SelectionStore,
    },
    supervisor::{ALTERNATE_EMBEDDING_FILE, EMBEDDING_FILE, Runtime},
};
use ctxfab_server::{
    adapters::{
        inbound::http,
        outbound::{
            artifact_storage::{DesktopArtifactBudget, DesktopArtifactCapacityProbe},
            sqlite::SqliteStore,
        },
    },
    application::{
        inference_setup::InferenceSetup,
        model_management::{
            ArtifactResult, ArtifactStore, DownloadJobStore, LocalArtifactState, ModelInventory,
            ModelManagement,
        },
        ports::CorpusStore,
        retrieval::{Mode, VectorIndex},
        runtime_setup::{Package, PackageDownload, PackageIdentity, RuntimeSetup},
    },
    domain::{
        corpus::{Batch, Commit, Document, DocumentKind},
        model::{
            Artifact, ArtifactKind, DownloadError, ModelSource,
            discovery::managed_embedding_profile,
        },
    },
    runtime::inference::Inference,
};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use tokio::{
    sync::{Notify, watch},
    time::timeout,
};
use tower::ServiceExt;

const SETUP: &str = "/v1/inference/setup";
const PLAN: &str = "/v1/inference/setup/plan";
const EMBEDDING_MODELS: &str = "/v1/retrieval/models";
const FIRST: &str = "00000000-0000-4000-8000-000000000001";
const SECOND: &str = "00000000-0000-4000-8000-000000000002";
const WAIT: Duration = Duration::from_secs(8);
const WORKER_EMBEDDING_PAYLOAD: &[u8] = b"formulation fixture";
const IMPORTED_PROFILE: &str =
    "hf-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const IMPORTED_PIN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Default)]
struct Transfers {
    packages: AtomicUsize,
    weights: AtomicUsize,
    block_weights: AtomicBool,
    weight_entered: Notify,
    weight_proceed: Notify,
    interrupted_import: Mutex<Option<mpsc::Receiver<()>>>,
    import_entered: Notify,
    import_ended: Notify,
    entered: Notify,
    proceed: Notify,
}

struct Release {
    worker: PathBuf,
    transfers: Arc<Transfers>,
}

impl Package for Release {
    fn archive_bytes(&self) -> u64 {
        4
    }
    fn identity(&self) -> Option<PackageIdentity> {
        Some(PackageIdentity {
            archive_sha256: "a".repeat(64),
            archive_bytes: 4,
            expanded_bytes: 6,
            bundle_bytes: 6,
            bundle_manifest_sha256: "b".repeat(64),
            apple_team_id: "FIXTURE123".into(),
            hosted_release: Some("fixture-release".into()),
        })
    }
    fn storage_peaks(&self) -> Option<(u64, Option<u64>)> {
        Some((10, Some(14)))
    }

    fn can_download(&self) -> bool {
        true
    }
    fn download(&self) -> PackageDownload<'_> {
        Box::pin(async {
            self.transfers.packages.fetch_add(1, Ordering::SeqCst);
            self.transfers.entered.notify_one();
            self.transfers.proceed.notified().await;
            self.publish()
        })
    }
    fn install(&self, archive: &Path) -> std::result::Result<PathBuf, String> {
        let interrupted = self
            .transfers
            .interrupted_import
            .lock()
            .map_err(|_| "fixture lock")?
            .take();
        if let Some(interrupted) = interrupted {
            self.transfers.import_entered.notify_one();
            let _ = interrupted.recv_timeout(WAIT);
            self.transfers.import_ended.notify_one();
            return Err("Fixture import interrupted before publication".into());
        }
        if std::fs::read(archive).is_ok_and(|data| data == b"pinned runtime ZIP") {
            self.publish()
        } else {
            Err("Fixture archive does not match the pin".into())
        }
    }
}

impl Release {
    fn publish(&self) -> std::result::Result<PathBuf, String> {
        std::fs::write(self.worker.with_extension("published"), b"fixture-release")
            .map_err(|error| error.to_string())?;
        Ok(self.worker.clone())
    }
}

struct Weights {
    directory: PathBuf,
    transfers: Arc<Transfers>,
}

impl ArtifactStore for Weights {
    fn install<'a>(&'a self, artifact: &'a Artifact) -> ArtifactResult<'a, ()> {
        Box::pin(async move {
            let (_sender, pause) = watch::channel(false);
            self.install_controlled(artifact, pause).await
        })
    }
    fn install_controlled<'a>(
        &'a self,
        artifact: &'a Artifact,
        mut pause: watch::Receiver<bool>,
    ) -> ArtifactResult<'a, ()> {
        Box::pin(async move {
            self.transfers.weights.fetch_add(1, Ordering::SeqCst);
            if self.transfers.block_weights.load(Ordering::SeqCst) {
                self.transfers.weight_entered.notify_one();
                tokio::select! {
                    _ = pause.wait_for(|value| *value) => return Err(DownloadError::Paused),
                    () = self.transfers.weight_proceed.notified() => {}
                }
            }
            let bytes = if artifact.profile == ALTERNATE_EMBEDDING_PROFILE
                || artifact.profile.starts_with("hf-emb-")
            {
                WORKER_EMBEDDING_PAYLOAD
            } else {
                b"tiny"
            };
            std::fs::write(self.directory.join(artifact.file.as_ref()), bytes)?;
            Ok(())
        })
    }
    fn local_state<'a>(&'a self, artifact: &'a Artifact) -> ArtifactResult<'a, LocalArtifactState> {
        Box::pin(async move {
            let partial = self.directory.join(format!(".{}.part", artifact.file));
            let staged_bytes = match std::fs::metadata(partial) {
                Ok(metadata) => metadata.len(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
                Err(error) => return Err(error.into()),
            };
            Ok(LocalArtifactState {
                installed: self.directory.join(artifact.file.as_ref()).is_file(),
                staged_bytes,
            })
        })
    }
    fn remove(&self, _: &Artifact) -> std::result::Result<(), DownloadError> {
        Err(DownloadError::Unavailable)
    }
    fn discard_partial(&self, artifact: &Artifact) -> std::result::Result<(), DownloadError> {
        match std::fs::remove_file(self.directory.join(format!(".{}.part", artifact.file))) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

struct Session {
    app: Router,
    store: Arc<SqliteStore>,
    setup: Arc<InferenceSetup>,
    release: Arc<RuntimeSetup>,
    downloads: Arc<ModelManagement>,
    inference: Arc<Inference>,
}

fn setup_catalog() -> Vec<Artifact> {
    let artifact = Artifact {
        file: GENERATION_FILE.into(),
        profile: GENERATION_PROFILE.into(),
        sha256: GENERATION_SHA256.into(),
        label: Some("Tiny answer model".into()),
        kind: Some(ArtifactKind::Generation),
        bytes: 4,
        source: None,
        remote_file: None,
    };
    let embedding = Artifact {
        file: EMBEDDING_FILE.into(),
        profile: PROFILE.into(),
        sha256: "06507c7b42688469c4e7298b0a1e16deff06caf291cf0a5b278c308249c3e439".into(),
        label: Some("Tiny embeddings".into()),
        kind: Some(ArtifactKind::Embedding),
        ..artifact.clone()
    };
    let alternate_embedding = Artifact {
        file: ALTERNATE_EMBEDDING_FILE.into(),
        profile: ALTERNATE_EMBEDDING_PROFILE.into(),
        sha256: ALTERNATE_EMBEDDING_SHA256.into(),
        bytes: WORKER_EMBEDDING_PAYLOAD.len() as u64,
        ..embedding.clone()
    };
    let imported = Artifact {
        profile: IMPORTED_PROFILE.into(),
        sha256: IMPORTED_PIN.into(),
        file: format!("{IMPORTED_PROFILE}.gguf").into(),
        remote_file: Some("fixture-Q8_0.gguf".into()),
        ..artifact.clone()
    };
    vec![artifact, embedding, alternate_embedding, imported]
}

impl Session {
    fn open(root: &Path, transfers: &Arc<Transfers>) -> Result<Self> {
        Self::with_worker(root, transfers, false)
    }

    fn with_worker(root: &Path, transfers: &Arc<Transfers>, responding: bool) -> Result<Self> {
        let directory = root.join("inference");
        std::fs::create_dir_all(&directory)?;
        let worker = root.join("fixture-worker");
        if responding {
            std::fs::write(
                &worker,
                include_bytes!("../../../quality/answers/mock_worker.py"),
            )?;
            std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o700))?;
        } else {
            std::fs::write(&worker, b"fixture requires warmup to fail")?;
        }
        let store = Arc::new(SqliteStore::open(&root.join("context.sqlite3"))?);
        let runtime = Arc::new(
            Runtime::default().with_guardian(PathBuf::from(env!("CARGO_BIN_EXE_ctxfab-server"))),
        );
        if std::fs::read(worker.with_extension("published"))
            .is_ok_and(|bytes| bytes == b"fixture-release")
        {
            runtime
                .publish(worker.clone())
                .map_err(|_| "fixture runtime could not reopen")?;
        }
        let imported = directory
            .join(format!("{IMPORTED_PROFILE}.gguf"))
            .is_file()
            .then(|| {
                ctxfab_inference::generation::Model::imported(
                    IMPORTED_PROFILE.into(),
                    "Imported fixture".into(),
                    IMPORTED_PIN.into(),
                )
            })
            .flatten();
        let inference = Arc::new(Inference::managed(
            &directory,
            &store,
            imported.into_iter().collect(),
            runtime.clone(),
        ));
        let activated = inference.clone();
        let release = Arc::new(RuntimeSetup::new(
            Box::new(Release {
                worker,
                transfers: transfers.clone(),
            }),
            runtime,
            move || activated.runtime_published(),
        ));
        let adapter = Arc::new(Weights {
            directory,
            transfers: transfers.clone(),
        });
        let mut catalog = setup_catalog();
        catalog.extend(store.load_models()?.into_iter().map(|model| model.artifact));
        let downloads = Arc::new(
            ModelManagement::new(catalog, adapter)
                .with_usage(inference.clone())
                .with_jobs(store.clone())?,
        );
        let capacity = Arc::new(DesktopArtifactBudget::new(
            DesktopArtifactCapacityProbe::new(root.to_path_buf(), 1_000_000, 0, 16),
        ));
        let setup = Arc::new(InferenceSetup::open(
            capacity,
            release.clone(),
            downloads.clone(),
            inference.answers.clone().ok_or("answers")?,
            inference.retrieval.clone().ok_or("retrieval")?,
            store.clone(),
        )?);
        let app = http::router_with_services(
            store.clone(),
            "fixture-token",
            http::Services {
                inference_setup: Some(setup.clone()),
                runtime_setup: Some(release.clone()),
                answers: inference.answers.clone(),
                retrieval: inference.retrieval.clone(),
                downloads: Some(downloads.clone()),
                ..http::Services::default()
            },
        );
        Ok(Self {
            app,
            store,
            setup,
            release,
            downloads,
            inference,
        })
    }

    async fn reopen(root: &Path, transfers: &Arc<Transfers>) -> Result<Self> {
        let session = Self::with_worker(root, transfers, true)?;
        session.setup.resume_after_startup().await?;
        Ok(session)
    }

    async fn stop(self) {
        self.setup.stop().await;
        self.release.stop();
        self.downloads.stop().await;
        self.inference.shutdown();
    }

    async fn request(
        &self,
        path: &str,
        input: Option<Value>,
        authorized: bool,
    ) -> Result<(StatusCode, Value)> {
        let method = if input.is_some() {
            Method::POST
        } else {
            Method::GET
        };
        self.call(method, path, input, authorized).await
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        input: Option<Value>,
        authorized: bool,
    ) -> Result<(StatusCode, Value)> {
        let mut request = Request::builder().method(method).uri(path);
        if authorized {
            request = request.header("Authorization", "Bearer fixture-token");
        }
        let body = if let Some(input) = input {
            request = request.header("Content-Type", "application/json");
            Body::from(input.to_string())
        } else {
            Body::empty()
        };
        let response = self.app.clone().oneshot(request.body(body)?).await?;
        let code = response.status();
        let raw = to_bytes(response.into_body(), 64 * 1024).await?;
        Ok((code, serde_json::from_slice(&raw)?))
    }

    async fn review(&self) -> Result<Value> {
        let (code, plan) = self.request(PLAN, Some(selection()), true).await?;
        assert_eq!(code, StatusCode::OK);
        assert_eq!(plan["canStart"], true);
        Ok(plan)
    }

    async fn await_phase(&self, phase: &str) -> Result<Value> {
        timeout(WAIT, async {
            loop {
                let (code, status) = self.request(SETUP, None, true).await?;
                assert_eq!(code, StatusCode::OK);
                if status["phase"] == phase {
                    return Ok(status);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?
    }
}

fn selection() -> Value {
    json!({"answerProfile": GENERATION_PROFILE, "embedding": false, "runtimeSource": "hosted"})
}
fn confirmation(plan: &Value, id: &str) -> Value {
    json!({"selection": plan["selection"], "fingerprint": plan["fingerprint"], "operationId": id})
}

#[tokio::test]
async fn confirmed_setup_is_idempotent_and_warm_failure_preserves_prior_selection() -> Result<()> {
    let root = tempfile::tempdir()?;
    let dir = root.path().join("inference");
    std::fs::create_dir(&dir)?;
    std::fs::write(dir.join(ALTERNATE_FILE), b"previous tiny weights")?;
    SqliteStore::open(&root.path().join("context.sqlite3"))?
        .save(ALTERNATE_PROFILE, ALTERNATE_SHA256)
        .map_err(|_| "could not seed selection")?;
    let transfers = Arc::new(Transfers::default());
    let session = Session::open(root.path(), &transfers)?;
    assert_eq!(
        session.request(PLAN, Some(selection()), false).await?.0,
        StatusCode::UNAUTHORIZED
    );
    let plan = session.review().await?;
    assert_eq!(plan["maximumTransferBytes"], 8);
    assert_eq!(plan["peakAdditionalBytes"], 14);
    let request = confirmation(&plan, FIRST);
    assert_eq!(
        session
            .request(SETUP, Some(request.clone()), false)
            .await?
            .0,
        StatusCode::UNAUTHORIZED
    );
    let mut stale = request.clone();
    stale["fingerprint"] = json!("0".repeat(64));
    assert_eq!(
        session.request(SETUP, Some(stale), true).await?.0,
        StatusCode::CONFLICT
    );
    let mut invalid = request.clone();
    invalid["operationId"] = json!("bad-id");
    assert_eq!(
        session.request(SETUP, Some(invalid), true).await?.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        session.request(SETUP, None, true).await?.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(transfers.packages.load(Ordering::SeqCst), 0);
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 0);
    assert_eq!(
        session.request(SETUP, Some(request.clone()), true).await?.0,
        StatusCode::ACCEPTED
    );
    timeout(WAIT, transfers.entered.notified()).await?;
    let (code, duplicate) = session.request(SETUP, Some(request.clone()), true).await?;
    assert_eq!(code, StatusCode::ACCEPTED);
    assert_eq!(duplicate["operationId"], FIRST);
    assert_eq!(
        session
            .request(SETUP, Some(confirmation(&plan, SECOND)), true)
            .await?
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 0);
    transfers.proceed.notify_one();
    let failed = session.await_phase("failed").await?;
    assert_eq!(failed["answerReady"], false);
    assert!(
        failed["error"]
            .as_str()
            .ok_or("missing activation error")?
            .contains("could not start")
    );
    assert_eq!(std::fs::read(dir.join(GENERATION_FILE))?, b"tiny");
    assert_eq!(
        session.store.load().map_err(|_| "selection")?,
        Some((ALTERNATE_PROFILE.into(), ALTERNATE_SHA256.into()))
    );
    assert_eq!(session.request(SETUP, Some(request), true).await?.1, failed);
    assert_eq!(transfers.packages.load(Ordering::SeqCst), 1);
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 1);
    session.stop().await;
    Ok(())
}

#[tokio::test]
async fn shutdown_and_reopen_resume_hosted_setup_under_the_same_confirmation() -> Result<()> {
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    let session = Session::with_worker(root.path(), &transfers, true)?;
    let first = confirmation(&session.review().await?, FIRST);
    let selection = first["selection"].clone();
    let (code, started) = session.request(SETUP, Some(first.clone()), true).await?;
    assert_eq!(code, StatusCode::ACCEPTED);
    assert_eq!(started["selection"], selection);
    timeout(WAIT, transfers.entered.notified()).await?;
    session.stop().await;
    let reopened = Session::reopen(root.path(), &transfers).await?;
    timeout(WAIT, transfers.entered.notified()).await?;
    let (code, replayed) = reopened.request(SETUP, Some(first), true).await?;
    assert_eq!(code, StatusCode::ACCEPTED);
    assert_eq!(replayed["operationId"], FIRST);
    assert_eq!(replayed["phase"], "runtime");
    assert_eq!(replayed["selection"], selection);
    transfers.proceed.notify_one();
    let saved = reopened.await_phase("completed").await?;
    assert_eq!(saved["answerReady"], true);
    assert_eq!(saved["operationId"], FIRST);
    assert_eq!(saved["selection"], selection);
    assert_eq!(transfers.packages.load(Ordering::SeqCst), 2);
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 1);
    reopened.stop().await;
    Ok(())
}

const QUOTE: &str = "The calibration interval is exactly 37 seconds.";

fn seed_source(store: &SqliteStore) -> Result<()> {
    let source = Document {
        source: "src/calibration.rs",
        revision: "setup-fixture-r1",
        kind: DocumentKind::Current {
            tracked_ref: "refs/heads/main",
        },
        title: "Calibration interval",
        content: QUOTE,
    };
    assert_eq!(
        store.commit_batch(&Batch {
            collection: "fixture",
            generation: 0,
            previous_cursor: None,
            next_cursor: "first",
            documents: &[source]
        })?,
        Commit::Indexed
    );
    Ok(())
}

fn seed_primary_vector(store: &SqliteStore, dimension: usize) -> Result<()> {
    seed_source(store)?;
    let passage = store.pending(PROFILE, 1)?.pop().ok_or("missing passage")?;
    let mut values = vec![0.0; DIMENSIONS];
    values[dimension] = 1.0;
    assert!(store.save_vector(PROFILE, &passage, &Embedding::new(values)?)?);
    Ok(())
}

async fn verify_first_answer(session: &Session) -> Result<()> {
    let (code, answer) = session
        .request(
            "/v1/answers",
            Some(json!({"query":"What is the calibration interval?"})),
            true,
        )
        .await?;
    assert_eq!(code, StatusCode::OK, "{answer}");
    assert_eq!(answer["outcome"], "answered");
    assert_eq!(answer["mode"], "bm25");
    assert_eq!(answer["model"], GENERATION_PROFILE);
    assert_eq!(answer["text"], QUOTE);
    let citations = answer["citations"].as_array().ok_or("missing citations")?;
    assert_eq!(citations.len(), 1);
    assert_eq!(citations[0]["quote"], QUOTE);
    assert_eq!(citations[0]["evidence"]["source"], "src/calibration.rs");
    assert_eq!(citations[0]["evidence"]["revision"], "setup-fixture-r1");
    Ok(())
}

#[tokio::test]
async fn confirmed_setup_warms_answer_and_returns_verified_citation_in_the_same_session()
-> Result<()> {
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    let session = Session::with_worker(root.path(), &transfers, true)?;
    let retrieval = session
        .inference
        .retrieval
        .as_ref()
        .ok_or("missing retrieval")?;
    let before = retrieval.set_index_enabled(false)?;
    seed_source(&session.store)?;
    let plan = session.review().await?;
    let (code, _) = session
        .request(SETUP, Some(confirmation(&plan, FIRST)), true)
        .await?;
    assert_eq!(code, StatusCode::ACCEPTED);
    transfers.proceed.notify_one();
    let ready = session.await_phase("completed").await?;
    assert_eq!(ready["answerReady"], true);
    assert_eq!(
        session.store.load().map_err(|_| "selection")?,
        Some((GENERATION_PROFILE.into(), GENERATION_SHA256.into()))
    );
    verify_first_answer(&session).await?;
    let after = retrieval.index_state()?;
    assert_eq!(after.epoch, before.epoch);
    assert!(!after.enabled);
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 1);
    session.stop().await;
    Ok(())
}

#[tokio::test]
async fn setup_activates_registered_generation_and_switches_between_isolated_embedding_profiles()
-> Result<()> {
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    let directory = root.path().join("inference");
    std::fs::create_dir(&directory)?;
    std::fs::write(directory.join(EMBEDDING_FILE), WORKER_EMBEDDING_PAYLOAD)?;
    let session = Session::with_worker(root.path(), &transfers, true)?;
    session.store.set_enabled(PROFILE, false)?;
    assert!(session.store.set_enabled(PROFILE, true)?.epoch > 0);
    seed_primary_vector(&session.store, 1)?;

    let mut choice = selection();
    choice["answerProfile"] = json!(IMPORTED_PROFILE);
    choice["embeddingProfile"] = json!(ALTERNATE_EMBEDDING_PROFILE);
    assert_eq!(
        session.request(PLAN, Some(choice.clone()), true).await?.0,
        StatusCode::BAD_REQUEST,
    );
    choice["embedding"] = json!(true);
    let (code, plan) = session.request(PLAN, Some(choice.clone()), true).await?;
    assert_eq!(code, StatusCode::OK, "{plan}");
    assert_eq!(plan["selection"], choice);
    assert_eq!(plan["embeddingInstalled"], false);
    assert_eq!(
        plan["maximumTransferBytes"],
        8 + WORKER_EMBEDDING_PAYLOAD.len()
    );
    assert_eq!(
        session
            .request(SETUP, Some(confirmation(&plan, FIRST)), true)
            .await?
            .0,
        StatusCode::ACCEPTED
    );
    transfers.proceed.notify_one();
    let status = session.await_phase("completed").await?;
    assert_eq!(status["answerReady"], true);
    assert_eq!(
        session.store.selected_embedding()?,
        ALTERNATE_EMBEDDING_PROFILE
    );
    assert_eq!(
        session.store.load().map_err(|_| "selection")?,
        Some((IMPORTED_PROFILE.into(), IMPORTED_PIN.into()))
    );
    let retrieval = session
        .inference
        .retrieval
        .as_ref()
        .ok_or("missing retrieval")?;
    assert_eq!(
        retrieval.progress()?.profile.as_deref(),
        Some(ALTERNATE_EMBEDDING_PROFILE)
    );
    assert_eq!(session.store.progress(PROFILE, None)?.indexed, 1);
    assert_eq!(retrieval.rebuild(1)?.indexed, 1);
    let query = ctxfab_server::application::SearchQuery::parse("calibration")?;
    assert_eq!(
        retrieval.search(&query, Mode::Semantic)?[0].source,
        "src/calibration.rs"
    );
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 2);
    session.stop().await;

    verify_switch_back(root.path(), &transfers, choice).await
}

async fn verify_switch_back(
    root: &Path,
    transfers: &Arc<Transfers>,
    mut choice: Value,
) -> Result<()> {
    let session = Session::reopen(root, transfers).await?;
    let retrieval = session
        .inference
        .retrieval
        .as_ref()
        .ok_or("missing retrieval")?;
    assert_eq!(
        retrieval.index_state()?.profile,
        ALTERNATE_EMBEDDING_PROFILE
    );
    choice
        .as_object_mut()
        .ok_or("missing choice")?
        .remove("embeddingProfile");
    let (code, plan) = session.request(PLAN, Some(choice), true).await?;
    assert_eq!(code, StatusCode::OK, "{plan}");
    assert_eq!(plan["embeddingInstalled"], true);
    assert_eq!(plan["selection"]["embeddingProfile"], PROFILE);
    let (code, explicit_plan) = session
        .request(PLAN, Some(plan["selection"].clone()), true)
        .await?;
    assert_eq!(code, StatusCode::OK, "{explicit_plan}");
    assert_eq!(explicit_plan["fingerprint"], plan["fingerprint"]);
    assert_eq!(
        session
            .request(SETUP, Some(confirmation(&plan, SECOND)), true)
            .await?
            .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(session.await_phase("completed").await?["answerReady"], true);
    assert_eq!(session.store.selected_embedding()?, PROFILE);
    assert_eq!(retrieval.progress()?.indexed, 1);
    assert_eq!(
        session
            .store
            .progress(ALTERNATE_EMBEDDING_PROFILE, None)?
            .indexed,
        1
    );
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 2);
    session.stop().await;
    Ok(())
}

fn imported_embedding_artifact() -> Artifact {
    const REPO: &str = "independent-publisher/embedding";
    const REV: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const FILE: &str = "Publisher-Embedding-Q8_0.gguf";
    const PIN: &str = "84486f4c2c4eb227557138964f1128e3ef8fbafd8c407d464f24f198000067fa";
    let profile = managed_embedding_profile(REPO, REV, FILE, PIN);
    Artifact {
        label: Some("Imported embedding fixture".into()),
        kind: Some(ArtifactKind::Embedding),
        source: Some(ModelSource {
            label: "Hugging Face · independent-publisher".into(),
            repository: REPO.into(),
            revision: REV.into(),
            url: format!("https://huggingface.co/{REPO}/tree/{REV}").into(),
            license: Some("apache-2.0".into()),
        }),
        file: format!("{profile}.gguf").into(),
        profile: profile.into(),
        remote_file: Some(FILE.into()),
        bytes: WORKER_EMBEDDING_PAYLOAD.len() as u64,
        sha256: PIN.into(),
    }
}

#[tokio::test]
async fn imported_embedding_can_be_activated_reopened_and_switched_back() -> Result<()> {
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    let directory = root.path().join("inference");
    std::fs::create_dir(&directory)?;
    std::fs::write(directory.join(EMBEDDING_FILE), WORKER_EMBEDDING_PAYLOAD)?;
    let model = imported_embedding_artifact();
    SqliteStore::open(&root.path().join("context.sqlite3"))?.retain_model(&model)?;

    let session = Session::with_worker(root.path(), &transfers, true)?;
    seed_primary_vector(&session.store, 1)?;
    let mut choice = selection();
    choice["embedding"] = json!(true);
    choice["embeddingProfile"] = json!(model.profile);
    let (code, plan) = session.request(PLAN, Some(choice.clone()), true).await?;
    assert_eq!(code, StatusCode::OK, "{plan}");
    assert_eq!(plan["selection"], choice);
    assert_eq!(plan["embeddingInstalled"], false);
    assert_eq!(
        session
            .request(SETUP, Some(confirmation(&plan, FIRST)), true)
            .await?
            .0,
        StatusCode::ACCEPTED
    );
    transfers.proceed.notify_one();
    assert_eq!(session.await_phase("completed").await?["answerReady"], true);

    let retrieval = session
        .inference
        .retrieval
        .as_ref()
        .ok_or("missing retrieval")?;
    assert_eq!(retrieval.index_state()?.profile, model.profile);
    assert_eq!(retrieval.rebuild(1)?.indexed, 1);
    let question = ctxfab_server::application::SearchQuery::parse("calibration")?;
    assert_eq!(
        retrieval.search(&question, Mode::Semantic)?[0].source,
        "src/calibration.rs"
    );
    assert_eq!(session.store.progress(PROFILE, None)?.indexed, 1);
    session.stop().await;

    let session = Session::reopen(root.path(), &transfers).await?;
    let retrieval = session
        .inference
        .retrieval
        .as_ref()
        .ok_or("missing retrieval")?;
    assert_eq!(retrieval.index_state()?.profile, model.profile);
    choice
        .as_object_mut()
        .ok_or("missing choice")?
        .remove("embeddingProfile");
    let (code, plan) = session.request(PLAN, Some(choice), true).await?;
    assert_eq!(code, StatusCode::OK, "{plan}");
    assert_eq!(plan["embeddingInstalled"], true);
    assert_eq!(
        session
            .request(SETUP, Some(confirmation(&plan, SECOND)), true)
            .await?
            .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(session.await_phase("completed").await?["answerReady"], true);
    assert_eq!(retrieval.index_state()?.profile, PROFILE);
    assert_eq!(session.store.progress(&model.profile, None)?.indexed, 1);
    assert_eq!(retrieval.progress()?.indexed, 1);
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 2);
    session.stop().await;
    Ok(())
}

fn provision_settings_models(root: &Path, alternate: &[u8]) -> Result<()> {
    let directory = root.join("inference");
    std::fs::create_dir(&directory)?;
    std::fs::write(root.join("fixture-worker.published"), b"fixture-release")?;
    std::fs::write(directory.join(EMBEDDING_FILE), WORKER_EMBEDDING_PAYLOAD)?;
    std::fs::write(directory.join(ALTERNATE_EMBEDDING_FILE), alternate)?;
    std::fs::write(directory.join(ALTERNATE_FILE), b"existing answer model")?;
    SqliteStore::open(&root.join("context.sqlite3"))?
        .save(ALTERNATE_PROFILE, ALTERNATE_SHA256)
        .map_err(|_| "unable to set prior answer selection")?;
    Ok(())
}

async fn select_in_settings(session: &Session, profile: &str) -> Result<(StatusCode, Value)> {
    session
        .call(
            Method::PUT,
            EMBEDDING_MODELS,
            Some(json!({ "profile": profile })),
            true,
        )
        .await
}

#[tokio::test]
async fn settings_switches_installed_embedding_without_setup_or_generation_changes() -> Result<()> {
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    provision_settings_models(root.path(), WORKER_EMBEDDING_PAYLOAD)?;
    let uninstalled = imported_embedding_artifact();
    SqliteStore::open(&root.path().join("context.sqlite3"))?.retain_model(&uninstalled)?;
    let session = Session::with_worker(root.path(), &transfers, true)?;
    seed_primary_vector(&session.store, 1)?;
    let retrieval = session
        .inference
        .retrieval
        .as_ref()
        .ok_or("missing retrieval")?;
    retrieval.set_index_enabled(false)?;

    let (code, catalog) = session.request(EMBEDDING_MODELS, None, true).await?;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(catalog["activeProfile"], PROFILE);
    assert_eq!(
        catalog["profiles"],
        json!([
            {"profile": PROFILE, "label": "Tiny embeddings"},
            {"profile": ALTERNATE_EMBEDDING_PROFILE, "label": "Tiny embeddings"},
        ])
    );
    assert_eq!(
        session.request(EMBEDDING_MODELS, None, false).await?.0,
        StatusCode::UNAUTHORIZED
    );
    for (profile, expected) in [
        ("../invalid", StatusCode::BAD_REQUEST),
        ("unknown-profile", StatusCode::NOT_FOUND),
        (uninstalled.profile.as_ref(), StatusCode::NOT_FOUND),
        (ALTERNATE_PROFILE, StatusCode::NOT_FOUND),
    ] {
        assert_eq!(select_in_settings(&session, profile).await?.0, expected);
    }

    let (code, selected) = select_in_settings(&session, ALTERNATE_EMBEDDING_PROFILE).await?;
    assert_eq!(code, StatusCode::OK, "{selected}");
    assert_eq!(selected["activeProfile"], ALTERNATE_EMBEDDING_PROFILE);
    assert_eq!(selected["profiles"], catalog["profiles"]);
    assert!(!retrieval.index_state()?.enabled);
    assert!(session.setup.status()?.is_none());
    assert_eq!(
        session.store.load().map_err(|_| "selection")?,
        Some((ALTERNATE_PROFILE.into(), ALTERNATE_SHA256.into(),))
    );
    assert_eq!(session.store.progress(PROFILE, None)?.indexed, 1);
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 0);
    session.stop().await;

    let reopened = Session::reopen(root.path(), &transfers).await?;
    assert_eq!(
        reopened.request(EMBEDDING_MODELS, None, true).await?.1,
        selected
    );
    let retrieval = reopened
        .inference
        .retrieval
        .as_ref()
        .ok_or("missing retrieval")?;
    assert_eq!(
        retrieval.index_state()?.profile,
        ALTERNATE_EMBEDDING_PROFILE
    );
    assert!(!retrieval.index_state()?.enabled);
    assert_eq!(
        select_in_settings(&reopened, PROFILE).await?.0,
        StatusCode::OK
    );
    assert_eq!(retrieval.index_state()?.profile, PROFILE);
    assert!(!retrieval.index_state()?.enabled);
    assert_eq!(reopened.store.progress(PROFILE, None)?.indexed, 1);
    reopened.stop().await;
    Ok(())
}

#[tokio::test]
async fn settings_rejects_unready_or_busy_embedding_without_switching() -> Result<()> {
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    provision_settings_models(root.path(), b"no embeddings response")?;
    let session = Session::with_worker(root.path(), &transfers, true)?;
    let (code, problem) = select_in_settings(&session, ALTERNATE_EMBEDDING_PROFILE).await?;
    assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(problem["code"], "embedding_model_unready");
    assert_eq!(session.store.selected_embedding()?, PROFILE);

    transfers.block_weights.store(true, Ordering::SeqCst);
    session.downloads.request(ALTERNATE_EMBEDDING_PROFILE)?;
    timeout(WAIT, transfers.weight_entered.notified()).await?;
    let (code, problem) = select_in_settings(&session, ALTERNATE_EMBEDDING_PROFILE).await?;
    assert_eq!(code, StatusCode::CONFLICT);
    assert_eq!(problem["code"], "embedding_selection_conflict");
    let (code, catalog) = session.request(EMBEDDING_MODELS, None, true).await?;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        catalog["profiles"]
            .as_array()
            .ok_or("missing profiles")?
            .len(),
        1
    );

    let plan = session.review().await?;
    let (code, _) = session
        .request(SETUP, Some(confirmation(&plan, FIRST)), true)
        .await?;
    assert_eq!(code, StatusCode::ACCEPTED);
    timeout(WAIT, transfers.weight_entered.notified()).await?;
    assert_eq!(
        select_in_settings(&session, PROFILE).await?.0,
        StatusCode::CONFLICT
    );
    assert_eq!(session.store.selected_embedding()?, PROFILE);
    session.stop().await;
    Ok(())
}

#[tokio::test]
async fn failed_embedding_warmup_preserves_the_previous_profile_and_index() -> Result<()> {
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    let directory = root.path().join("inference");
    std::fs::create_dir(&directory)?;
    std::fs::write(
        directory.join(ALTERNATE_EMBEDDING_FILE),
        b"no embeddings response",
    )?;
    let session = Session::with_worker(root.path(), &transfers, true)?;
    let before = session.store.state(PROFILE)?;
    seed_primary_vector(&session.store, 0)?;

    let mut choice = selection();
    choice["embedding"] = json!(true);
    choice["embeddingProfile"] = json!(ALTERNATE_EMBEDDING_PROFILE);
    let (code, plan) = session.request(PLAN, Some(choice), true).await?;
    assert_eq!(code, StatusCode::OK, "{plan}");
    assert_eq!(plan["embeddingInstalled"], true);
    assert_eq!(
        session
            .request(SETUP, Some(confirmation(&plan, FIRST)), true)
            .await?
            .0,
        StatusCode::ACCEPTED,
    );
    transfers.proceed.notify_one();
    let failed = session.await_phase("failed").await?;
    assert_eq!(failed["answerReady"], true);
    assert_eq!(
        failed["error"],
        "The selected embedding model failed its readiness check. Answers are ready."
    );
    assert_eq!(session.store.selected_embedding()?, PROFILE);
    let after = session.store.state(PROFILE)?;
    assert!(after.enabled);
    assert_eq!(after.epoch, before.epoch);
    assert_eq!(session.store.progress(PROFILE, None)?.indexed, 1);
    assert_eq!(
        session
            .store
            .progress(ALTERNATE_EMBEDDING_PROFILE, None)?
            .indexed,
        0
    );
    verify_first_answer(&session).await?;
    session.stop().await;
    Ok(())
}

#[tokio::test]
#[ignore = "loopback server launched by the Swift guided-setup contract test"]
async fn serve_swift_setup_fixture() -> Result<()> {
    let root = PathBuf::from(
        std::env::var_os("CTXFAB_TEST_SETUP_FIXTURE_ROOT").ok_or("missing fixture root")?,
    );
    let transfers = Arc::new(Transfers::default());
    let session = Session::with_worker(&root, &transfers, true)?;
    session
        .inference
        .retrieval
        .as_ref()
        .ok_or("missing retrieval")?
        .set_index_enabled(false)?;
    seed_source(&session.store)?;
    transfers.proceed.notify_one();

    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    std::fs::write(
        root.join("port.pending"),
        listener.local_addr()?.port().to_string(),
    )?;
    std::fs::rename(root.join("port.pending"), root.join("port"))?;
    let result = axum::serve(listener, session.app.clone())
        .with_graceful_shutdown(async move {
            let _ = timeout(Duration::from_secs(45), terminate.recv()).await;
        })
        .await;
    session.stop().await;
    Ok(result?)
}

async fn embedding_setup(change_policy: bool) -> Result<()> {
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    let session = Session::with_worker(root.path(), &transfers, true)?;
    let retrieval = session
        .inference
        .retrieval
        .as_ref()
        .ok_or("missing retrieval")?;
    retrieval.set_index_enabled(false)?;
    let mut choices = selection();
    choices["embedding"] = json!(true);
    let (code, plan) = session.request(PLAN, Some(choices), true).await?;
    assert_eq!(code, StatusCode::OK);
    let (code, _) = session
        .request(SETUP, Some(confirmation(&plan, FIRST)), true)
        .await?;
    assert_eq!(code, StatusCode::ACCEPTED);
    timeout(WAIT, transfers.entered.notified()).await?;
    if change_policy {
        retrieval.set_index_enabled(true)?;
        retrieval.set_index_enabled(false)?;
    }
    transfers.proceed.notify_one();
    let expected = if change_policy { "failed" } else { "completed" };
    let status = session.await_phase(expected).await?;
    assert_eq!(status["answerReady"], true);
    if change_policy {
        assert!(
            status["error"]
                .as_str()
                .ok_or("missing policy conflict")?
                .contains("embedding policy changed")
        );
    }
    assert_eq!(retrieval.index_state()?.enabled, !change_policy);
    let expected_weight_transfers = if change_policy { 1 } else { 2 };
    assert_eq!(
        transfers.weights.load(Ordering::SeqCst),
        expected_weight_transfers
    );
    session.stop().await;
    Ok(())
}

#[tokio::test]
async fn optional_embedding_enable_respects_changes_after_confirmation() -> Result<()> {
    embedding_setup(false).await?;
    embedding_setup(true).await
}

#[tokio::test]
async fn removing_reviewed_partial_during_runtime_download_requires_fresh_estimate() -> Result<()> {
    let root = tempfile::tempdir()?;
    let directory = root.path().join("inference");
    std::fs::create_dir(&directory)?;
    let partial = directory.join(format!(".{GENERATION_FILE}.part"));
    std::fs::write(&partial, b"ti")?;
    std::fs::write(directory.join(ALTERNATE_FILE), b"previous tiny weights")?;
    SqliteStore::open(&root.path().join("context.sqlite3"))?
        .save(ALTERNATE_PROFILE, ALTERNATE_SHA256)
        .map_err(|_| "initial selection")?;
    let transfers = Arc::new(Transfers::default());
    let session = Session::with_worker(root.path(), &transfers, true)?;
    let prior = session.store.load().map_err(|_| "selection")?;
    let plan = session.review().await?;
    let (code, _) = session
        .request(SETUP, Some(confirmation(&plan, FIRST)), true)
        .await?;
    assert_eq!(code, StatusCode::ACCEPTED);
    timeout(WAIT, transfers.entered.notified()).await?;
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 0);
    std::fs::remove_file(partial)?;
    transfers.proceed.notify_one();
    let status = session.await_phase("failed").await?;
    assert_eq!(status["answerReady"], false);
    assert!(
        status["error"]
            .as_str()
            .ok_or("missing stale estimate error")?
            .contains("Review a new installation estimate")
    );
    assert_eq!(transfers.packages.load(Ordering::SeqCst), 1);
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 0);
    assert!(!directory.join(GENERATION_FILE).exists());
    assert_eq!(session.store.load().map_err(|_| "selection")?, prior);
    session.stop().await;
    Ok(())
}

#[tokio::test]
async fn local_zip_setup_reuses_verified_weights_and_requires_an_absolute_archive() -> Result<()> {
    let root = tempfile::tempdir()?;
    let directory = root.path().join("inference");
    std::fs::create_dir(&directory)?;
    std::fs::write(directory.join(GENERATION_FILE), b"tiny")?;
    let archive = root.path().join("reviewed-runtime.zip");
    std::fs::write(&archive, b"pinned runtime ZIP")?;
    let transfers = Arc::new(Transfers::default());
    let session = Session::with_worker(root.path(), &transfers, true)?;
    let choices = json!({"embedding":false,"runtimeSource":"localArchive"});
    let (status, plan) = session.request(PLAN, Some(choices.clone()), true).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plan["canStart"], true);
    assert_eq!(plan["answerInstalled"], true);
    assert_eq!(plan["maximumTransferBytes"], 0);
    assert_eq!(plan["peakAdditionalBytes"], 10);
    let mut confirmed = confirmation(&plan, FIRST);
    let (status, problem) = session
        .request(SETUP, Some(confirmed.clone()), true)
        .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(problem["code"], "invalid_inference_setup");
    confirmed["archivePath"] = json!("relative.zip");
    assert_eq!(
        session
            .request(SETUP, Some(confirmed.clone()), true)
            .await?
            .0,
        StatusCode::BAD_REQUEST
    );
    confirmed["archivePath"] = json!(root.path().join("wrong.zip"));
    assert_eq!(
        session.request(SETUP, Some(confirmed), true).await?.0,
        StatusCode::ACCEPTED
    );
    assert_eq!(session.await_phase("failed").await?["answerReady"], false);
    let (_, reviewed) = session.request(PLAN, Some(choices), true).await?;
    let mut accepted = confirmation(&reviewed, SECOND);
    accepted["archivePath"] = json!(archive);
    assert_eq!(
        session.request(SETUP, Some(accepted), true).await?.0,
        StatusCode::ACCEPTED
    );
    let completed = session.await_phase("completed").await?;
    assert_eq!(completed["operationId"], SECOND);
    assert_eq!(completed["answerReady"], true);
    assert_eq!(transfers.packages.load(Ordering::SeqCst), 0);
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 0);
    session.stop().await;
    Ok(())
}

async fn start_blocked_weight(root: &Path, transfers: &Arc<Transfers>) -> Result<Session> {
    transfers.block_weights.store(true, Ordering::SeqCst);
    let session = Session::with_worker(root, transfers, true)?;
    let reviewed = confirmation(&session.review().await?, FIRST);
    assert_eq!(
        session.request(SETUP, Some(reviewed), true).await?.0,
        StatusCode::ACCEPTED
    );
    timeout(WAIT, transfers.entered.notified()).await?;
    transfers.proceed.notify_one();
    timeout(WAIT, transfers.weight_entered.notified()).await?;
    let jobs = session.store.load_download_jobs()?;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].operation_id, FIRST);
    assert_eq!(jobs[0].phase, "running");
    Ok(session)
}

#[tokio::test]
async fn recovering_owned_weight_keeps_job_identity_and_reuses_published_runtime() -> Result<()> {
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    let first = start_blocked_weight(root.path(), &transfers).await?;
    first.stop().await;
    let session = Session::reopen(root.path(), &transfers).await?;
    timeout(WAIT, transfers.weight_entered.notified()).await?;
    let jobs = session.store.load_download_jobs()?;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].operation_id, FIRST);
    assert_eq!(transfers.packages.load(Ordering::SeqCst), 1);
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 2);
    transfers.weight_proceed.notify_one();
    let completed = session.await_phase("completed").await?;
    assert_eq!(completed["operationId"], FIRST);
    assert_eq!(completed["answerReady"], true);
    assert!(session.store.load_download_jobs()?.is_empty());
    session.stop().await;
    Ok(())
}

#[tokio::test]
async fn manually_paused_setup_weight_is_not_replaced_on_reopen() -> Result<()> {
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    let first = start_blocked_weight(root.path(), &transfers).await?;
    first.downloads.pause(GENERATION_PROFILE)?;
    first.stop().await;
    let session = Session::reopen(root.path(), &transfers).await?;
    let status = session.await_phase("failed").await?;
    assert_eq!(status["operationId"], FIRST);
    assert_eq!(status["answerReady"], false);
    let jobs = session.store.load_download_jobs()?;
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].operation_id, FIRST);
    assert_eq!(jobs[0].phase, "paused");
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 1);
    assert_eq!(transfers.packages.load(Ordering::SeqCst), 1);
    session.stop().await;
    Ok(())
}

async fn awaiting_archive(
    root: &Path,
    transfers: &Arc<Transfers>,
) -> Result<(Session, PathBuf, Value)> {
    let (interrupt, blocked_import) = mpsc::channel();
    *transfers
        .interrupted_import
        .lock()
        .map_err(|_| "fixture lock")? = Some(blocked_import);
    let first = Session::with_worker(root, transfers, true)?;
    let archive = root.join("runtime.zip");
    std::fs::write(&archive, b"pinned runtime ZIP")?;
    let choices = json!({"embedding": false, "runtimeSource":"localArchive"});
    let (code, plan) = first.request(PLAN, Some(choices), true).await?;
    assert_eq!(code, StatusCode::OK);
    let mut confirmed = confirmation(&plan, FIRST);
    confirmed["archivePath"] = json!(archive);
    assert_eq!(
        first.request(SETUP, Some(confirmed.clone()), true).await?.0,
        StatusCode::ACCEPTED
    );
    timeout(WAIT, transfers.import_entered.notified()).await?;
    first.stop().await;
    interrupt.send(())?;
    timeout(WAIT, transfers.import_ended.notified()).await?;
    let session = Session::reopen(root, transfers).await?;
    let waiting = session.await_phase("waitingForArchive").await?;
    assert_eq!(waiting["operationId"], FIRST);
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 0);
    assert_eq!(transfers.packages.load(Ordering::SeqCst), 0);
    Ok((session, archive, confirmed))
}

#[tokio::test]
async fn interrupted_zip_import_waits_for_reselection_under_original_operation() -> Result<()> {
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    let (session, archive, _) = awaiting_archive(root.path(), &transfers).await?;
    let route = "/v1/inference/setup/archive";
    let other = json!({"operationId": SECOND, "archivePath": archive});
    assert_eq!(
        session.request(route, Some(other), true).await?.0,
        StatusCode::CONFLICT
    );
    let request = json!({"operationId": FIRST, "archivePath": archive});
    assert_eq!(
        session
            .request(route, Some(request.clone()), false)
            .await?
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        session.request(route, Some(request), true).await?.0,
        StatusCode::ACCEPTED
    );
    let ready = session.await_phase("completed").await?;
    assert_eq!(ready["operationId"], FIRST);
    assert_eq!(ready["answerReady"], true);
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 1);
    assert_eq!(transfers.packages.load(Ordering::SeqCst), 0);
    session.stop().await;
    Ok(())
}

#[tokio::test]
async fn discarded_owned_partial_is_not_recreated_by_startup_recovery() -> Result<()> {
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    let first = start_blocked_weight(root.path(), &transfers).await?;
    let partial = root
        .path()
        .join("inference")
        .join(format!(".{GENERATION_FILE}.part"));
    std::fs::write(&partial, b"t")?;
    first.stop().await;
    let session = Session::with_worker(root.path(), &transfers, true)?;
    session.downloads.discard_partial(GENERATION_PROFILE)?;
    assert!(!partial.exists());
    assert!(session.store.load_download_jobs()?.is_empty());
    session.setup.resume_after_startup().await?;
    let stopped = session.await_phase("failed").await?;
    assert_eq!(stopped["operationId"], FIRST);
    assert_eq!(stopped["answerReady"], false);
    assert!(session.store.load_download_jobs()?.is_empty());
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 1);
    assert_eq!(transfers.packages.load(Ordering::SeqCst), 1);
    session.stop().await;
    Ok(())
}

#[tokio::test]
async fn recovered_weight_waits_for_free_space_without_requesting_new_consent() -> Result<()> {
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    let first = start_blocked_weight(root.path(), &transfers).await?;
    first.stop().await;
    let fill = root.path().join("inference").join("other-installed-asset");
    std::fs::File::create(&fill)?.set_len(1_000_000)?;
    let session = Session::reopen(root.path(), &transfers).await?;
    timeout(WAIT, async {
        loop {
            let (code, state) = session.request(SETUP, None, true).await?;
            assert_eq!(code, StatusCode::OK);
            assert_eq!(state["phase"], "answerWeights");
            assert_eq!(state["operationId"], FIRST);
            if state["error"]
                .as_str()
                .is_some_and(|text| text.contains("Waiting for disk space"))
            {
                return Ok::<_, Box<dyn Error>>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 1);
    std::fs::remove_file(fill)?;
    timeout(WAIT, transfers.weight_entered.notified()).await?;
    transfers.weight_proceed.notify_one();
    let done = session.await_phase("completed").await?;
    assert_eq!(done["operationId"], FIRST);
    assert_eq!(transfers.packages.load(Ordering::SeqCst), 1);
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 2);
    session.stop().await;
    Ok(())
}

#[tokio::test]
async fn cancelling_archive_wait_fences_late_reselection_and_releases_the_setup_slot() -> Result<()>
{
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    let (session, archive, original) = awaiting_archive(root.path(), &transfers).await?;
    let route = format!("{SETUP}/archive");
    let cancel = format!("{route}/{FIRST}");
    assert_eq!(
        session.call(Method::DELETE, &cancel, None, false).await?.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        session
            .call(Method::DELETE, &format!("{route}/{SECOND}"), None, true)
            .await?
            .0,
        StatusCode::CONFLICT
    );
    let (code, cancelled) = session.call(Method::DELETE, &cancel, None, true).await?;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(cancelled["phase"], "interrupted");
    assert_eq!(cancelled["operationId"], FIRST);
    assert_eq!(
        session.request(SETUP, Some(original), true).await?.1,
        cancelled
    );
    let late = json!({"operationId": FIRST, "archivePath": archive});
    assert_eq!(
        session.request(&route, Some(late), true).await?.0,
        StatusCode::CONFLICT
    );
    let selection = json!({"embedding":false, "runtimeSource":"localArchive"});
    let (code, plan) = session.request(PLAN, Some(selection), true).await?;
    assert_eq!(code, StatusCode::OK);
    let mut next = confirmation(&plan, SECOND);
    next["archivePath"] = json!(root.path().join("unavailable.zip"));
    let (code, accepted) = session.request(SETUP, Some(next), true).await?;
    assert_eq!(code, StatusCode::ACCEPTED);
    assert_eq!(accepted["operationId"], SECOND);
    session.await_phase("failed").await?;
    assert_eq!(transfers.packages.load(Ordering::SeqCst), 0);
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 0);
    session.stop().await;
    Ok(())
}

#[tokio::test]
async fn manual_verified_runtime_wakes_waiting_archive_setup_under_original_id() -> Result<()> {
    let root = tempfile::tempdir()?;
    let transfers = Arc::new(Transfers::default());
    let (session, archive, _) = awaiting_archive(root.path(), &transfers).await?;
    let wrong = json!({"archivePath": root.path().join("incorrect.zip")});
    let (code, rejected) = session
        .request("/v1/runtime/install", Some(wrong), true)
        .await?;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(rejected["state"], "failed");
    assert_eq!(
        session.request(SETUP, None, true).await?.1["phase"],
        "waitingForArchive"
    );
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 0);
    timeout(WAIT, session.stop()).await?;
    let session = Session::reopen(root.path(), &transfers).await?;
    let waiting = session.await_phase("waitingForArchive").await?;
    assert_eq!(waiting["operationId"], FIRST);
    let (code, runtime) = session
        .request(
            "/v1/runtime/install",
            Some(json!({"archivePath": archive})),
            true,
        )
        .await?;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(runtime["state"], "ready");
    let completed = session.await_phase("completed").await?;
    assert_eq!(completed["operationId"], FIRST);
    assert_eq!(completed["answerReady"], true);
    assert_eq!(transfers.weights.load(Ordering::SeqCst), 1);
    assert_eq!(transfers.packages.load(Ordering::SeqCst), 0);
    timeout(WAIT, session.stop()).await?;
    Ok(())
}
