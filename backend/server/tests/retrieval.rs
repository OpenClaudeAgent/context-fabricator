use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ctxfab_inference::{
    DIMENSIONS, Embedder, Embedding, EmbeddingError, Input, PROFILE, llama_cpp::LocalWorker,
};
use ctxfab_server::adapters::outbound::sqlite::SqliteStore;
use ctxfab_server::application::{
    SearchQuery,
    ports::{CorpusStore, Repository},
    retrieval::{Mode, Retrieval, RetrievalError, VectorIndex},
};
use ctxfab_server::domain::corpus::{Batch, CollectionAccess, Commit, Document, DocumentKind};
use serde::Deserialize;
use serde_json::{Value, json};

#[path = "retrieval/api.rs"]
mod api;

const DATASET: &str = include_str!("../../../quality/retrieval/corpus.json");

#[derive(Deserialize)]
struct Corpus {
    documents: Vec<Record>,
    queries: Vec<Judgment>,
}

#[derive(Deserialize)]
struct Record {
    id: String,
    source: String,
    revision: String,
    kind: String,
    title: String,
    content: String,
}

#[derive(Deserialize)]
struct Judgment {
    id: String,
    family: String,
    query: String,
    relevant: String,
}

impl Record {
    fn document(&self) -> Result<Document<'_>, Box<dyn Error>> {
        let kind = match self.kind.as_str() {
            "current" => DocumentKind::Current {
                tracked_ref: "refs/heads/main",
            },
            "history" => DocumentKind::History,
            "workflow" => DocumentKind::Workflow,
            _ => return Err("unknown fixture source kind".into()),
        };
        Ok(Document {
            source: &self.source,
            revision: &self.revision,
            kind,
            title: &self.title,
            content: &self.content,
        })
    }

    fn matches(&self, hit: &Value) -> bool {
        hit["source"] == self.source && hit["revision"] == self.revision && hit["kind"] == self.kind
    }
}

struct FixtureEmbedder;

impl Embedder for FixtureEmbedder {
    fn embed(&self, input: Input<'_>) -> Result<Embedding, EmbeddingError> {
        let archive = match input {
            Input::Passage { title, content } => {
                title.contains("archive") || content.contains("archive")
            }
            Input::Query(query) => query.contains("archive"),
        };
        let mut vector = vec![0.0; DIMENSIONS];
        vector[usize::from(archive)] = 1.0;
        Embedding::new(vector)
    }
}

fn fixture_service(store: &Arc<SqliteStore>, profile: &'static str) -> Retrieval {
    Retrieval::new(
        store.clone(),
        store.clone(),
        Arc::new(FixtureEmbedder),
        profile,
    )
}

fn add(
    store: &SqliteStore,
    collection: &str,
    documents: &[Document<'_>],
) -> Result<(), Box<dyn Error>> {
    assert_eq!(
        store.commit_batch(&Batch {
            collection,
            generation: 0,
            previous_cursor: None,
            next_cursor: "initial",
            documents,
        })?,
        Commit::Indexed
    );
    Ok(())
}

#[test]
fn profile_progress_is_durable_and_canonical_citations_respect_deletion()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("retrieval.sqlite3");
    let docs = [
        Document {
            source: "code",
            revision: "new",
            kind: DocumentKind::Current {
                tracked_ref: "refs/heads/main",
            },
            title: "retry_after_gate",
            content: "active",
        },
        Document {
            source: "code",
            revision: "old",
            kind: DocumentKind::History,
            title: "previous",
            content: "archive",
        },
    ];
    let store = Arc::new(SqliteStore::open(&path)?);
    add(&store, "scope", &docs)?;
    let service = fixture_service(&store, PROFILE);
    let query = SearchQuery::parse("archive")?;
    assert_eq!(service.rebuild(1)?.indexed, 1);
    assert!(matches!(
        service.search(&query, Mode::Semantic),
        Err(RetrievalError::Incomplete)
    ));
    drop(service);
    drop(store);

    let store = Arc::new(SqliteStore::open(&path)?);
    let service = fixture_service(&store, PROFILE);
    assert_eq!(service.progress()?.indexed, 1);
    assert_eq!(service.rebuild(32)?.state, "ready");
    let ranked = service.search(&query, Mode::Hybrid)?;
    assert_eq!(ranked[0].revision, "old");

    add(&store, "overlap", &docs[1..])?;
    assert_eq!(service.rebuild(32)?.total, 3);
    assert!(store.set_collection_access("scope", 0, CollectionAccess::Lost)?);
    let ranked = service.search(&query, Mode::Semantic)?;
    assert_eq!(ranked.iter().filter(|hit| hit.revision == "old").count(), 1);
    assert_eq!(ranked[0].access, "available");
    store.delete_collection("overlap")?;
    assert_eq!(
        service.search(&query, Mode::Semantic)?[0].access,
        "retained"
    );

    let version = fixture_service(&store, "fixture-changed-profile");
    assert_eq!(version.progress()?.indexed, 0);
    assert_eq!(version.rebuild(1)?.state, "incomplete");
    assert!(matches!(
        version.search(&query, Mode::Hybrid),
        Err(RetrievalError::Incomplete)
    ));
    assert_eq!(service.progress()?.state, "ready");
    assert_eq!(service.search(&query, Mode::Hybrid)?[0].revision, "old");
    assert_eq!(version.rebuild(32)?.state, "ready");
    store.delete_collection("scope")?;
    assert_eq!(service.progress()?.total, 0);
    assert_eq!(version.progress()?.total, 0);
    assert!(version.search(&query, Mode::Hybrid)?.is_empty());
    Ok(())
}

#[test]
fn changed_content_and_reused_rows_cannot_accept_stale_vectors() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let store = Arc::new(SqliteStore::open(
        &directory.path().join("retrieval.sqlite3"),
    )?);
    let docs = [Document {
        source: "code",
        revision: "head",
        kind: DocumentKind::History,
        title: "current",
        content: "active",
    }];
    add(&store, "scope", &docs)?;
    let service = fixture_service(&store, PROFILE);
    service.rebuild(32)?;
    let changed = [Document {
        content: "archive",
        ..docs[0]
    }];
    assert_eq!(
        store.commit_batch(&Batch {
            collection: "scope",
            generation: 0,
            previous_cursor: Some("initial"),
            next_cursor: "updated",
            documents: &changed,
        })?,
        Commit::Indexed
    );
    assert_eq!(service.progress()?.indexed, 0);
    let pending = store.pending(PROFILE, 1)?;
    let previous = pending.first().ok_or("missing pending record")?;
    store.delete_collection("scope")?;
    let vector = FixtureEmbedder.embed(Input::Query("archive"))?;
    assert!(!store.save_vector(PROFILE, previous, &vector)?);
    assert_eq!(
        store.commit_batch(&Batch {
            collection: "scope",
            generation: 1,
            previous_cursor: None,
            next_cursor: "reimported",
            documents: &changed,
        })?,
        Commit::Indexed
    );
    assert_eq!(store.pending(PROFILE, 1)?[0].id, previous.id);
    assert!(!store.save_vector(PROFILE, previous, &vector)?);
    assert_eq!(service.progress()?.indexed, 0);
    assert!(Embedding::new(vec![0.0; DIMENSIONS]).is_err());
    assert!(Embedding::new(vec![1.0; DIMENSIONS - 1]).is_err());
    assert!(Embedding::new(vec![f32::NAN; DIMENSIONS]).is_err());
    assert!(LocalWorker::connect("192.0.2.1:19001".parse()?, &"a".repeat(48)).is_err());
    assert_eq!(
        SearchQuery::parse("quota::新しい")?.original(),
        "quota::新しい"
    );
    Ok(())
}

struct Backend(Child);

#[tokio::test]
async fn worker_accepts_only_authenticated_profile_compatible_vectors() -> Result<(), Box<dyn Error>>
{
    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Redirect},
        routing::post,
    };
    use std::net::{Ipv4Addr, SocketAddr};

    let secret = "a".repeat(48);
    let expected = format!("Bearer {secret}");
    let routes = Router::new().route("/v1/embeddings", post(move |headers: HeaderMap, Json(body): Json<Value>| {
        let expected = expected.clone();
        async move {
            if headers.get("authorization").and_then(|header| header.to_str().ok()) != Some(expected.as_str())
                || body["model"] != PROFILE || body["encoding_format"] != "float"
            {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            let profile = match body["input"].as_str() {
                Some("Instruct: Given an engineering question, retrieve source passages that answer it.\nQuery: réessayer" | "Fixture\nquota::新しい") => PROFILE,
                Some("Fixture\nredirect") => return Redirect::temporary("/v1/embeddings").into_response(),
                _ => "other",
            };
            Json(json!({"model": profile, "data": [{"index": 0, "embedding": vec![1.0; DIMENSIONS]}]})).into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let SocketAddr::V4(address) = listener.local_addr()? else {
        return Err("expected IPv4 listener".into());
    };
    let server = tokio::spawn(async move { axum::serve(listener, routes).await });
    let result = tokio::task::spawn_blocking(move || {
        let worker = LocalWorker::connect(address, &secret)?;
        let passage = |content| Input::Passage {
            title: "Fixture",
            content,
        };
        let question = Input::Query("réessayer");
        assert_eq!(worker.embed(question)?.values().len(), DIMENSIONS);
        assert_eq!(
            worker.embed(passage("quota::新しい"))?.values().len(),
            DIMENSIONS
        );
        assert!(matches!(
            worker.embed(Input::Query("other")),
            Err(EmbeddingError::Vector)
        ));
        assert!(matches!(
            worker.embed(passage("redirect")),
            Err(EmbeddingError::Runtime)
        ));
        let invalid = LocalWorker::connect(address, &"b".repeat(48))?;
        assert!(matches!(
            invalid.embed(question),
            Err(EmbeddingError::Runtime)
        ));
        assert!(matches!(
            worker.embed(Input::Passage {
                title: "",
                content: &"é".repeat(1536)
            }),
            Err(EmbeddingError::PassageTooLarge)
        ));
        Ok::<_, EmbeddingError>(())
    })
    .await;
    server.abort();
    result??;
    Ok(())
}

impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Api {
    agent: ureq::Agent,
    endpoint: String,
    bearer: String,
}

impl Api {
    fn start(bundle: &Path, data: &Path) -> Result<(Backend, Self), Box<dyn Error>> {
        let backend = Backend(
            Command::new(bundle.join("Contents/MacOS/ctxfab-server"))
                .arg(data)
                .arg("--app-session")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()?,
        );
        let started = Instant::now();
        let discovery = data.join("endpoint.json");
        while !discovery.exists() && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let received: Value = serde_json::from_slice(&std::fs::read(discovery)?)?;
        assert_eq!(received["processId"], backend.0.id());
        let endpoint = received["url"]
            .as_str()
            .ok_or("missing local endpoint")?
            .to_owned();
        let bearer = format!(
            "Bearer {}",
            received["token"]
                .as_str()
                .ok_or("missing local credential")?
        );
        let agent = ureq::Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(70)))
            .build()
            .into();
        Ok((
            backend,
            Self {
                agent,
                endpoint,
                bearer,
            },
        ))
    }

    fn get(&self, path: &str) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
        self.agent
            .get(format!("{}{path}", self.endpoint))
            .header("Authorization", &self.bearer)
            .call()
    }

    fn rebuild(&self, limit: u16) -> Result<Value, Box<dyn Error>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut response = self
                .agent
                .post(format!(
                    "{}/v1/retrieval/rebuild?limit={limit}",
                    self.endpoint
                ))
                .header("Authorization", &self.bearer)
                .send_empty()?;
            if response.status() == 200 {
                return read(response);
            }
            let status = response.status();
            let problem: Value = serde_json::from_slice(&response.body_mut().read_to_vec()?)?;
            if status != 503 || problem["code"] != "embedding_busy" || Instant::now() >= deadline {
                return Err(format!("rebuild returned {status}: {problem}").into());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn enable_index(&self) -> Result<Value, Box<dyn Error>> {
        read(
            self.agent
                .put(format!("{}/v1/retrieval/index", self.endpoint))
                .header("Authorization", &self.bearer)
                .header("Content-Type", "application/json")
                .send(r#"{"enabled":true}"#)?,
        )
    }

    fn await_progress(&self, satisfied: impl Fn(&Value) -> bool) -> Result<Value, Box<dyn Error>> {
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            let progress = read(self.get("/v1/retrieval")?)?;
            if satisfied(&progress) {
                return Ok(progress);
            }
            if Instant::now() >= deadline {
                return Err(format!("embedding progress stayed at {progress}").into());
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn search(
        &self,
        question: &str,
        mode: &str,
    ) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
        self.agent
            .get(format!("{}/v1/search", self.endpoint))
            .header("Authorization", &self.bearer)
            .query("query", question)
            .query("mode", mode)
            .call()
    }
}

fn read(mut response: ureq::http::Response<ureq::Body>) -> Result<Value, Box<dyn Error>> {
    let status = response.status();
    let body = response.body_mut().read_to_vec()?;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    Ok(serde_json::from_slice(&body)?)
}

fn stop(mut backend: Backend) -> Result<(), Box<dyn Error>> {
    drop(backend.0.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(status) = backend.0.try_wait()? {
            assert!(status.success());
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err("helper did not stop with its owner".into())
}

fn evaluate(api: &Api, corpus: &Corpus) -> Result<Vec<Value>, Box<dyn Error>> {
    let mut results = Vec::new();
    for judgment in &corpus.queries {
        let relevant = corpus
            .documents
            .iter()
            .find(|document| document.id == judgment.relevant)
            .ok_or("judgment references an unknown document")?;
        for mode in ["bm25", "semantic", "hybrid"] {
            let start = Instant::now();
            let response = api.search(&judgment.query, mode)?;
            let hits = read(response)?;
            let elapsed = start.elapsed().as_micros();
            let hits = hits.as_array().ok_or("missing hit list")?;
            let rank = hits
                .iter()
                .position(|hit| relevant.matches(hit))
                .map(|offset| offset + 1);
            if matches!(judgment.family.as_str(), "exact" | "revision")
                && matches!(mode, "bm25" | "hybrid")
            {
                assert_eq!(
                    rank,
                    Some(1),
                    "exact provenance for {} in {mode}",
                    judgment.id
                );
            }
            results.push(json!({
                "id": judgment.id, "family": judgment.family, "mode": mode,
                "rank": rank, "micros": elapsed, "top": hits.iter().take(3).collect::<Vec<_>>(),
            }));
        }
    }
    Ok(results)
}

#[test]
#[ignore = "requires the packaged macOS app and pinned local llama.cpp worker; run make qualify-retrieval"]
fn packaged_multilingual_corpus_qualifies_qwen_and_bm25() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::DirBuilderExt;

    let bundle =
        PathBuf::from(std::env::var_os("CTXFAB_TEST_APP_BUNDLE").ok_or("missing app bundle")?);
    let work = tempfile::tempdir()?;
    let data = work.path().join("data");
    std::fs::DirBuilder::new().mode(0o700).create(&data)?;
    let database = data.join("context.sqlite3");
    let corpus: Corpus = serde_json::from_str(DATASET)?;
    let documents = corpus
        .documents
        .iter()
        .map(Record::document)
        .collect::<Result<Vec<_>, _>>()?;
    let store = SqliteStore::open(&database)?;
    add(&store, "qualification", &documents)?;
    store.set_enabled(PROFILE, false)?;
    drop(store);

    let (backend, api) = Api::start(&bundle, &data)?;
    let disabled = read(api.get("/v1/retrieval")?)?;
    assert_eq!(disabled["state"], "disabled");
    assert_eq!(disabled["indexed"], 0);
    assert_eq!(api.search("retry_after_gate", "hybrid")?.status(), 503);
    let rebuild_start = Instant::now();
    assert_eq!(api.enable_index()?["enabled"], true);
    let partial =
        api.await_progress(|progress| progress["indexed"].as_i64().is_some_and(|n| n > 0))?;
    assert_eq!(partial["state"], "incomplete");
    assert_eq!(api.search("retry_after_gate", "hybrid")?.status(), 503);
    stop(backend)?;

    let saved = SqliteStore::open(&database)?.progress(PROFILE, None)?;
    assert!(saved.indexed >= partial["indexed"].as_i64().ok_or("partial indexed count")?);
    assert!(saved.indexed < saved.total);
    let (backend, api) = Api::start(&bundle, &data)?;
    let progress = api.await_progress(|progress| progress["state"] == "ready")?;
    assert_eq!(progress["state"], "ready");
    assert_eq!(progress["profile"], PROFILE);
    assert_eq!(progress["dimensions"], DIMENSIONS);
    assert_eq!(progress["indexed"], saved.total);
    let rebuild_micros = rebuild_start.elapsed().as_micros();
    assert_eq!(api.rebuild(32)?, progress);
    assert_eq!(api.search("retry_after_gate", "hybrid")?.status(), 200);
    let traces = await_retrieval_telemetry(&api)?;
    let results = evaluate(&api, &corpus)?;
    improve_cross_language(&results)?;
    let status = read(api.get("/v1/status")?)?;
    assert_eq!(status["inference"], "notConfigured");
    assert_eq!(status["retrieval"]["state"], "ready");
    assert_eq!(status["connectedSources"], 0);
    assert_eq!(status["outboundAttempts"], 0);
    let cli = Command::new(bundle.join("Contents/MacOS/ctxfab"))
        .args(["search", "retry_after_gate", "--mode", "hybrid"])
        .env("CTXFAB_DATA_DIRECTORY", &data)
        .output()?;
    assert!(cli.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&cli.stdout)?[0]["revision"],
        "r2"
    );
    let store = SqliteStore::open(&database)?;
    let vector_bytes: i64 = rusqlite::Connection::open(&database)?.query_row(
        "SELECT coalesce(sum(length(embedding)), 0) FROM document_vectors",
        [],
        |row| row.get(0),
    )?;
    store.delete_collection("qualification")?;
    assert_eq!(read(api.search("retry_after_gate", "hybrid")?)?, json!([]));
    assert!(
        store
            .search(&SearchQuery::parse("retry_after_gate")?)?
            .is_empty()
    );
    stop(backend)?;
    println!(
        "RETRIEVAL_RESULT {}",
        json!({
            "documents": documents.len(), "rebuildMicrosIncludingRestart": rebuild_micros,
            "vectorBytes": vector_bytes, "profile": progress, "cases": results, "telemetry": traces,
        })
    );
    Ok(())
}

fn improve_cross_language(results: &[Value]) -> Result<(), Box<dyn Error>> {
    let hits = |mode: &str| {
        results
            .iter()
            .filter(|result| {
                result["family"] == "crossLanguage"
                    && result["mode"] == mode
                    && result["rank"].as_u64().is_some_and(|rank| rank <= 3)
            })
            .count()
    };
    let hybrid = hits("hybrid");
    let semantic = hits("semantic");
    let bm25 = hits("bm25");
    if hybrid < 6 || semantic < 6 || hybrid <= bm25 {
        return Err(format!(
            "Cross-language Recall@3 below the fixture gate: BM25 {bm25}, hybrid {hybrid}, semantic {semantic}"
        ).into());
    }
    Ok(())
}

fn await_retrieval_telemetry(api: &Api) -> Result<Value, Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        let report = read(api.get("/v1/telemetry")?)?;
        let samples = report["traces"]
            .as_array()
            .ok_or("missing retrieval traces")?;
        let evidence: Vec<_> = samples
            .iter()
            .filter(|span| {
                matches!(
                    span["name"].as_str(),
                    Some("vector_retrieval" | "embedding_rebuild")
                )
            })
            .cloned()
            .collect();
        if ["vector_retrieval", "embedding_rebuild"]
            .iter()
            .all(|name| {
                evidence
                    .iter()
                    .any(|span| span["name"] == *name && span["level"] == "ok")
            })
        {
            return Ok(json!(evidence));
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    Err("missing retrieval latency traces".into())
}

#[test]
fn disabled_index_fences_inflight_vectors_and_survives_restart() -> Result<(), Box<dyn Error>> {
    use std::sync::{Condvar, Mutex, mpsc};
    struct Blocking {
        started: mpsc::SyncSender<()>,
        gate: Arc<(Mutex<bool>, Condvar)>,
    }
    impl Embedder for Blocking {
        fn embed(&self, _: Input<'_>) -> Result<Embedding, EmbeddingError> {
            self.started.send(()).map_err(|_| EmbeddingError::Runtime)?;
            let (lock, changed) = &*self.gate;
            let mut released = lock.lock().map_err(|_| EmbeddingError::Runtime)?;
            while !*released {
                released = changed
                    .wait(released)
                    .map_err(|_| EmbeddingError::Runtime)?;
            }
            let mut vector = vec![0.0; DIMENSIONS];
            vector[0] = 1.0;
            Embedding::new(vector)
        }
    }
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("index.sqlite3");
    let store = Arc::new(SqliteStore::open(&database)?);
    let documents = [Document {
        source: "fixture",
        revision: "v1",
        kind: DocumentKind::History,
        title: "orchid",
        content: "orchid search remains available",
    }];
    add(&store, "fixture", &documents)?;
    let (started, receiver) = mpsc::sync_channel(0);
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let retrieval = Arc::new(Retrieval::new(
        store.clone(),
        store.clone(),
        Arc::new(Blocking {
            started,
            gate: gate.clone(),
        }),
        PROFILE,
    ));
    let worker = {
        let retrieval = retrieval.clone();
        std::thread::spawn(move || retrieval.rebuild_one())
    };
    receiver.recv_timeout(Duration::from_secs(5))?;
    let deleted = retrieval.delete_index()?;
    assert!(!deleted.state.enabled);
    assert_eq!(deleted.state.epoch, 1);
    assert_eq!(deleted.removed_vectors, 0);
    assert_eq!(deleted.removed_bytes, 0);
    let query = SearchQuery::parse("orchid")?;
    assert_eq!(retrieval.search(&query, Mode::Bm25)?.len(), 1);
    assert!(matches!(
        retrieval.search(&query, Mode::Semantic),
        Err(RetrievalError::Disabled)
    ));
    assert!(matches!(
        retrieval.search(&query, Mode::Hybrid),
        Err(RetrievalError::Disabled)
    ));
    assert_eq!(retrieval.set_index_enabled(true)?.epoch, 2);
    let (lock, changed) = &*gate;
    *lock.lock().map_err(|_| "lock poisoned")? = true;
    changed.notify_all();
    worker.join().map_err(|_| "worker panicked")??;
    assert_eq!(
        store.progress(PROFILE, None)?.indexed,
        0,
        "old epoch must not publish"
    );
    assert_eq!(retrieval.set_index_enabled(false)?.epoch, 3);
    drop(retrieval);
    drop(store);
    let reopened = Arc::new(SqliteStore::open(&database)?);
    let state = reopened.state(PROFILE)?;
    assert!(!state.enabled);
    assert_eq!(state.epoch, 3);
    assert_eq!(reopened.search(&query)?.len(), 1);
    let active = fixture_service(&reopened, PROFILE);
    assert_eq!(active.set_index_enabled(true)?.epoch, 4);
    active.rebuild(1)?;
    assert_eq!(reopened.progress(PROFILE, None)?.indexed, 1);
    let deletion = active.delete_index()?;
    assert_eq!(deletion.removed_vectors, 1);
    assert_eq!(deletion.removed_bytes, i64::try_from(DIMENSIONS * 4)?);
    assert_eq!(reopened.search(&query)?.len(), 1);
    Ok(())
}
