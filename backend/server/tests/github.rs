#![cfg(debug_assertions)]

use std::error::Error;
use std::fs;
use std::io::{self, ErrorKind};
use std::num::NonZeroU16;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use ctxfab_inference::{DIMENSIONS, Embedding};
use ctxfab_server::adapters::{
    inbound::http::{Services, router_with_services},
    outbound::{
        github::{BUCKET, GitHubRest},
        sqlite::SqliteStore,
    },
};
use ctxfab_server::application::{
    SearchQuery,
    answers::EvidenceStore,
    credentials::CredentialStore,
    github::{GitHub, Store, current::Store as CurrentStore},
    ports::Repository,
    retrieval::VectorIndex,
    sync::TrafficLedger,
};
use ctxfab_server::domain::github;
use ctxfab_server::runtime::{
    clock::Clock,
    entropy::random_uuid,
    outbound::{OutboundPool, QuotaPolicy},
    scheduler::Scheduler,
};
use serde_json::{Value, json};
use tower::ServiceExt;

#[path = "../src/qualification/github_fixture.rs"]
mod memory;
#[path = "github/scheduled.rs"]
mod scheduled;

const FIRST: &str = "github_pat_fixture_primary";
const REPLACEMENT: &str = "github_pat_fixture_replacement";

struct Origin {
    process: Child,
    directory: tempfile::TempDir,
    port: u16,
}

impl Drop for Origin {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

impl Origin {
    async fn start() -> Result<Self, Box<dyn Error>> {
        let directory = tempfile::tempdir()?;
        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../quality/m07/github_fixture.py");
        let log = directory.path().join("fixture.log");
        let output = fs::File::create(&log)?;
        let mut origin = Self {
            process: Command::new("python3")
                .arg(script)
                .arg(directory.path())
                .env("CTXFAB_GITHUB_FIXTURE_STARTUP_DIAGNOSTICS", "1")
                .stdin(Stdio::null())
                .stdout(output.try_clone()?)
                .stderr(output)
                .spawn()?,
            directory,
            port: 0,
        };
        let port = origin.directory.path().join("port");
        let ready = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match fs::read_to_string(&port) {
                    Ok(contents) => return contents.parse::<u16>().map_err(io::Error::other),
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                if let Some(status) = origin.process.try_wait()? {
                    return Err(io::Error::other(format!(
                        "fixture process exited with {status}"
                    )));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        origin.port = match ready {
            Ok(Ok(port)) => port,
            Ok(Err(error)) => return Err(fixture_start_error(&error, &log).into()),
            Err(_) => {
                let error =
                    io::Error::new(ErrorKind::TimedOut, "port file missing after 5 seconds");
                return Err(fixture_start_error(&error, &log).into());
            }
        };
        Ok(origin)
    }

    fn app(
        &self,
        store: &Arc<SqliteStore>,
        credentials: Option<Arc<memory::Credentials>>,
    ) -> Result<Router, Box<dyn Error>> {
        Ok(app_with_github(store, self.github(store, credentials)?))
    }

    fn github(
        &self,
        store: &Arc<SqliteStore>,
        credentials: Option<Arc<memory::Credentials>>,
    ) -> Result<Arc<GitHub>, Box<dyn Error>> {
        self.github_at(store, credentials, Clock::system())
    }

    fn github_at(
        &self,
        store: &Arc<SqliteStore>,
        credentials: Option<Arc<memory::Credentials>>,
        clock: Clock,
    ) -> Result<Arc<GitHub>, Box<dyn Error>> {
        let gate = OutboundPool::new(store.clone(), clock).gate(
            BUCKET,
            QuotaPolicy {
                concurrency: NonZeroU16::MIN,
                spacing: Duration::from_millis(8),
            },
        )?;
        let provider =
            Arc::new(GitHubRest::fixture(gate, self.port)?.with_scope_authority(store.clone()));
        Ok(Arc::new(
            GitHub::new(
                provider.clone(),
                store.clone(),
                credentials.map(|store| store as Arc<dyn CredentialStore + Send + Sync>),
                || {
                    random_uuid()
                        .map(|id| format!("test/{id}"))
                        .map_err(|_| github::Error::Storage)
                },
            )
            .with_current_code(provider, store.clone(), move || clock.now()),
        ))
    }

    fn hits(&self) -> std::io::Result<String> {
        fs::read_to_string(self.directory.path().join("hits.log"))
    }

    fn expire(&self) -> std::io::Result<()> {
        fs::write(self.directory.path().join("expire"), [])
    }
}

fn fixture_start_error(error: &io::Error, log: &Path) -> io::Error {
    let output = fs::read_to_string(log).unwrap_or_default();
    io::Error::new(
        error.kind(),
        format!("GitHub fixture did not publish its listening port: {error}\n{output}"),
    )
}

fn app_with_github(store: &Arc<SqliteStore>, github: Arc<GitHub>) -> Router {
    router_with_services(
        store.clone(),
        "local-session-fixture",
        Services {
            github: Some(github),
            ..Services::default()
        },
    )
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Value,
    status: u16,
) -> Result<Value, Box<dyn Error>> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("Authorization", "Bearer local-session-fixture")
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&body)?))?,
        )
        .await?;
    assert_eq!(response.status().as_u16(), status, "{method} {uri}");
    Ok(serde_json::from_slice(
        &to_bytes(response.into_body(), 128 * 1024).await?,
    )?)
}

async fn authorize(
    app: &Router,
    revision: u32,
    token: &str,
    status: u16,
) -> Result<Value, Box<dyn Error>> {
    let result = call(
        app,
        "PUT",
        "/v1/github/credential",
        json!({"revision": revision, "token": token}),
        status,
    )
    .await?;
    assert!(!result.to_string().contains(token));
    Ok(result)
}

fn selection(revision: u32, name: &str, id: i64) -> Value {
    json!({
        "revision": revision, "repository": name, "expectedId": id, "intervalMinutes": 90,
        "features": {"currentCode": true, "history": true, "actions": true, "codeGraph": false}
    })
}

#[tokio::test]
async fn desktop_refresh_queues_a_durable_job_without_waiting_for_github()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/alpha", 41),
        200,
    )
    .await?;
    let before = origin.hits()?;
    let path = "/v1/github/repositories/41/refresh?background=true";
    let state = call(&app, "POST", path, Value::Null, 202).await?;
    assert_eq!(
        state["repositories"][0]["codeCollection"]["phase"],
        "queued"
    );
    let again = call(&app, "POST", path, Value::Null, 202).await?;
    assert_eq!(
        again["repositories"][0]["codeCollection"],
        state["repositories"][0]["codeCollection"]
    );
    assert_eq!(origin.hits()?, before);
    assert_eq!(
        call(&app, "GET", "/v1/github", Value::Null, 200).await?["repositories"][0]["codeCollection"],
        state["repositories"][0]["codeCollection"],
    );
    Ok(())
}

#[tokio::test]
async fn current_code_search_selects_the_repository_and_branch() -> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    for (revision, name, id) in [(1, "fixture/alpha", 41), (2, "fixture/beta", 42)] {
        call(
            &app,
            "PUT",
            "/v1/github/repositories",
            selection(revision, name, id),
            200,
        )
        .await?;
        call(
            &app,
            "POST",
            &format!("/v1/github/repositories/{id}/refresh"),
            Value::Null,
            200,
        )
        .await?;
    }
    let path = "/v1/search?query=paced";
    assert_eq!(
        call(&app, "GET", path, Value::Null, 200)
            .await?
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    let beta = call(
        &app,
        "GET",
        &format!("{path}&repositoryId=42&branch=trunk"),
        Value::Null,
        200,
    )
    .await?;
    assert_eq!(beta.as_array().map(Vec::len), Some(1));
    assert_eq!(beta[0]["source"], "github:42:old.rs");
    assert_eq!(beta[0]["trackedRef"], "refs/heads/trunk");
    assert_eq!(beta[0]["revision"], "c".repeat(40));
    let selected = SearchQuery::parse_scoped("paced", Some(42), Some("trunk".into()))?;
    let lexical = store.search(&selected)?;
    let mut values = vec![0.; DIMENSIONS];
    values[0] = 1.;
    let vector = Embedding::new(values)?;
    for passage in store.pending("scoped-fixture", 20)? {
        if passage.evidence.source == "github:42:old.rs" {
            assert!(store.save_vector("scoped-fixture", &passage, &vector)?);
        }
    }
    assert_eq!(store.progress("scoped-fixture", None)?.state, "incomplete");
    assert_eq!(
        store.progress("scoped-fixture", selected.scope())?.state,
        "ready"
    );
    let (dense, hybrid_lexical) =
        store.rank("scoped-fixture", &vector, lexical, selected.scope())?;
    assert_eq!(dense.len(), 1);
    assert_eq!(hybrid_lexical.len(), 1);
    assert!(dense[0].same_evidence(&hybrid_lexical[0]));
    let global = store.search(&SearchQuery::parse("paced")?)?;
    let excerpts = store.excerpts(&global, selected.scope())?;
    assert_eq!(excerpts.len(), 1);
    assert_eq!(
        excerpts[0].text,
        "fn paced_source_fixture() { assert!(true); }\n"
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{path}&repositoryId=42&branch=main"),
            Value::Null,
            200
        )
        .await?,
        json!([])
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{path}&repositoryId=42"),
            Value::Null,
            400
        )
        .await?["code"],
        "invalid_search_scope"
    );
    for scope in [
        json!({"repositoryId": 42}),
        json!({"repositoryId": null, "branch": null}),
    ] {
        let mut question = scope;
        question["query"] = json!("paced");
        call(&app, "POST", "/v1/answers", question, 400).await?;
    }
    Ok(())
}

#[tokio::test]
async fn current_code_refresh_resumes_and_counts_read_only_hops() -> Result<(), Box<dyn Error>> {
    use ctxfab_server::application::ports::SnapshotStore;

    let origin = Origin::start().await?;
    let data = tempfile::tempdir()?;
    let database = data.path().join("context.sqlite3");
    let credentials = Arc::new(memory::Credentials::default());
    let store = Arc::new(SqliteStore::open(&database)?);
    let app = origin.app(&store, Some(credentials.clone()))?;
    authorize(&app, 0, FIRST, 200).await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/alpha", 41),
        200,
    )
    .await?;
    let route = "/v1/github/repositories/41/refresh";
    let first = call(&app, "POST", route, Value::Null, 200).await?;
    assert_eq!(first["commit"], "a".repeat(40));
    let first_hit = store.search(&ctxfab_server::application::SearchQuery::parse(
        "paced_source_fixture",
    )?)?;
    assert_eq!(
        first_hit[0]
            .location
            .as_ref()
            .ok_or("missing location")?
            .path,
        "src/old.rs"
    );
    fs::write(origin.directory.path().join("advance-code"), [])?;
    fs::write(origin.directory.path().join("fail-code"), [])?;
    call(&app, "POST", route, Value::Null, 503).await?;
    let throttled = call(&app, "GET", "/v1/github", Value::Null, 200).await?;
    let retry_at = throttled["repositories"][0]["codeCollection"]["nextAtMs"]
        .as_i64()
        .ok_or("retry due")?;
    let observed = origin.hits()?;
    call(&app, "POST", route, Value::Null, 503).await?;
    assert_eq!(origin.hits()?, observed);
    assert_eq!(
        store
            .pending_snapshot("github/current/41/main")?
            .ok_or("interrupted collection")?
            .sequence,
        3
    );
    assert!(
        !store
            .search(&ctxfab_server::application::SearchQuery::parse(
                "removed_source_fixture"
            )?)?
            .is_empty()
    );
    drop(app);
    drop(store);
    fs::remove_file(origin.directory.path().join("fail-code"))?;
    let store = Arc::new(SqliteStore::open(&database)?);
    let app = origin.app(&store, Some(credentials))?;
    Clock::system().wait_until(retry_at).await;
    let second = call(&app, "POST", route, Value::Null, 200).await?;
    assert_eq!(second["commit"], "b".repeat(40));
    let hits = store.search(&ctxfab_server::application::SearchQuery::parse(
        "paced_source_fixture",
    )?)?;
    assert_eq!(
        hits[0].location.as_ref().ok_or("missing location")?.path,
        "src/renamed.rs"
    );
    assert!(
        store
            .search(&ctxfab_server::application::SearchQuery::parse(
                "removed_source_fixture"
            )?)?
            .iter()
            .all(|hit| hit
                .location
                .as_ref()
                .is_none_or(|location| location.path != "src/removed.rs"))
    );
    let log = origin.hits()?;
    assert_counted_code_hops(&store, &log)?;
    call(
        &app,
        "DELETE",
        "/v1/github/credential?revision=2",
        Value::Null,
        200,
    )
    .await?;
    let retained = store.search(&ctxfab_server::application::SearchQuery::parse(
        "newly_collected_fixture",
    )?)?;
    assert_eq!(retained[0].access, "retained");
    call(&app, "POST", route, Value::Null, 403).await?;
    assert_eq!(origin.hits()?, log);
    Ok(())
}

fn assert_counted_code_hops(store: &SqliteStore, log: &str) -> Result<(), Box<dyn Error>> {
    assert_eq!(
        log.lines()
            .filter(|line| line.ends_with(&format!("/git/trees/{}", "1".repeat(40))))
            .count(),
        1
    );
    let unique_blobs = log
        .lines()
        .filter(|line| line.contains("/git/blobs/"))
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(unique_blobs.len(), 3);
    assert_eq!(
        log.lines()
            .filter(|line| line.ends_with("/branches/main"))
            .count(),
        3
    );
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("/git/blobs/"))
            .count(),
        4
    );
    assert_eq!(
        usize::try_from(store.traffic(BUCKET)?.attempts)?,
        log.lines().count()
    );
    assert!(log.lines().all(|line| line.starts_with("GET ")));
    Ok(())
}

#[tokio::test]
async fn a_queued_refresh_does_not_send_a_disconnected_credential() -> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/alpha", 41),
        200,
    )
    .await?;
    call(
        &app,
        "PUT",
        "/v1/github/rate-limit",
        json!({"revision":2,"requestsPerMinute":60}),
        200,
    )
    .await?;
    let before = origin.hits()?;
    let refreshing = app.clone();
    let refresh = tokio::spawn(async move {
        call(
            &refreshing,
            "POST",
            "/v1/github/repositories/41/refresh",
            Value::Null,
            409,
        )
        .await
        .map_err(|error| error.to_string())
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let state = call(&app, "GET", "/v1/github", Value::Null, 200).await?;
            if state["traffic"]["queued"] == 1 {
                break Ok::<_, Box<dyn Error>>(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    call(
        &app,
        "DELETE",
        "/v1/github/credential?revision=3",
        Value::Null,
        200,
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(3), refresh).await???;
    assert_eq!(origin.hits()?, before);
    assert_eq!(
        usize::try_from(store.traffic(BUCKET)?.attempts)?,
        before.lines().count()
    );
    Ok(())
}

#[tokio::test]
async fn resuming_a_sealed_stage_checks_contents_permission() -> Result<(), Box<dyn Error>> {
    use ctxfab_server::application::{SearchQuery, ports::SnapshotStore};
    use ctxfab_server::domain::corpus::snapshot::Plan;

    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/alpha", 41),
        200,
    )
    .await?;
    let route = "/v1/github/repositories/41/refresh";
    call(&app, "POST", route, Value::Null, 200).await?;
    let scope = store.current_scope(41)?;
    let pinned = "b".repeat(40);
    let stage = store.begin_snapshot(&Plan {
        collection: &scope.collection,
        generation: scope.generation,
        predecessor: scope.predecessor,
        revision: &pinned,
        tracked_ref: "refs/heads/main",
    })?;
    store.append_snapshot(stage.id, 0, None, &[])?;
    fs::write(origin.directory.path().join("deny-contents"), [])?;
    call(&app, "POST", route, Value::Null, 403).await?;
    assert_eq!(
        store
            .active_snapshot(&scope.collection)?
            .ok_or("retained snapshot")?
            .id,
        scope.predecessor.ok_or("previous public ID")?
    );
    assert!(store.pending_snapshot(&scope.collection)?.is_none());
    let hits = store.search(&SearchQuery::parse("paced_source_fixture")?)?;
    assert_eq!(hits[0].access, "retained");
    let mut configured = store.snapshot()?;
    assert_eq!(configured.revision, 3);
    let mut obsolete_probe = configured.repositories.remove(0);
    obsolete_probe.contents = github::Access::Available;
    assert!(matches!(
        store.save(2, &obsolete_probe),
        Err(github::Error::Conflict)
    ));
    assert!(origin.hits()?.contains("/branches?per_page=1"));
    Ok(())
}

#[test]
fn current_authority_fences_replacement_and_preserves_rate_edits() -> Result<(), Box<dyn Error>> {
    use ctxfab_server::application::ports::SnapshotStore;
    use ctxfab_server::domain::corpus::snapshot::{Plan, SnapshotError};
    use ctxfab_server::domain::github::{
        Access, ConfiguredRepository, Defaults, Error, Repository,
    };

    let store = SqliteStore::open(Path::new(":memory:"))?;
    let config = ConfiguredRepository {
        repository: Repository {
            id: 41,
            full_name: "fixture/alpha".into(),
            default_branch: "main".into(),
            private: true,
        },
        branch: "main".into(),
        interval_minutes: 60,
        features: Defaults::default().features,
        contents: Access::Available,
        actions: Access::Unchecked,
        verified: true,
        code_collection: None,
    };
    store.bind(0, Some(("fixture/a", "test-a")))?;
    store.save(1, &config)?;
    let original = store.current_scope(41)?;
    let pending = store.begin_snapshot(&Plan {
        collection: &original.collection,
        generation: original.generation,
        predecessor: original.predecessor,
        revision: "fixture-commit",
        tracked_ref: "refs/heads/main",
    })?;
    store.append_snapshot(pending.id, 0, None, &[])?;
    store.set_rate(2, 30)?;
    store.check_scope(&original)?;
    assert_eq!(
        store
            .pending_snapshot(&original.collection)?
            .ok_or("retained stage")?
            .id,
        pending.id
    );

    store.bind(3, Some(("fixture/b", "test-b")))?;
    store.save(4, &config)?;
    let replacement = store.current_scope(41)?;
    assert_ne!(original.generation, replacement.generation);
    assert!(matches!(store.check_scope(&original), Err(Error::Conflict)));
    assert!(matches!(
        store.publish_snapshot(pending.id),
        Err(SnapshotError::Conflict)
    ));
    store.expire_scope_credential(&original)?;
    store.check_scope(&replacement)?;

    store.remove(5, 41)?;
    assert!(matches!(
        store.check_scope(&replacement),
        Err(Error::Conflict)
    ));
    store.save(6, &config)?;
    assert_ne!(store.current_scope(41)?.generation, replacement.generation);
    Ok(())
}

#[tokio::test]
async fn rate_updates_preserve_reserved_spacing_and_accounting() -> Result<(), Box<dyn Error>> {
    use ctxfab_server::domain::sync::{Admission, Outcome};

    let origin = Origin::start().await?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("context.sqlite3");
    let store = Arc::new(SqliteStore::open(&path)?);
    let app = origin.app(&store, None)?;
    assert_eq!(store.reserve(BUCKET, 10_000, 8)?, Admission::Granted);
    store.complete(BUCKET, Outcome::Failed, 13_000)?;

    let route = "/v1/github/rate-limit";
    let slow = call(
        &app,
        "PUT",
        route,
        json!({"revision": 0, "requestsPerMinute": 10}),
        200,
    )
    .await?;
    assert_eq!(slow["traffic"]["effectiveRequestsPerMinute"], 10);
    assert_eq!(slow["traffic"]["failures"], 1);
    assert_eq!(store.traffic(BUCKET)?.cooldown_until, 13_000);
    assert_eq!(
        store.reserve(BUCKET, 15_999, 8)?,
        Admission::WaitUntil(16_000)
    );
    assert_eq!(store.reserve(BUCKET, 16_000, 8)?, Admission::Granted);
    let fast = call(
        &app,
        "PUT",
        route,
        json!({"revision": 1, "requestsPerMinute": 60}),
        200,
    )
    .await?;
    assert_eq!(fast["traffic"]["effectiveRequestsPerMinute"], 60);
    assert_eq!(fast["traffic"]["attempts"], 2);
    assert_eq!(fast["traffic"]["pending"], 1);
    assert_eq!(
        store.reserve(BUCKET, 21_999, 8)?,
        Admission::WaitUntil(22_000)
    );
    store.complete(BUCKET, Outcome::Succeeded, 0)?;
    call(
        &app,
        "PUT",
        route,
        json!({"revision": 1, "requestsPerMinute": 30}),
        409,
    )
    .await?;
    call(
        &app,
        "PUT",
        route,
        json!({"revision": 2, "requestsPerMinute": 61}),
        400,
    )
    .await?;

    drop(app);
    drop(store);
    let reopened = Arc::new(SqliteStore::open(&path)?);
    let app = origin.app(&reopened, None)?;
    let state = call(&app, "GET", "/v1/github", Value::Null, 200).await?;
    assert_eq!(state["traffic"]["requestedRequestsPerMinute"], 60);
    assert_eq!(state["traffic"]["attempts"], 2);
    assert_eq!(state["traffic"]["pending"], 0);
    assert_eq!(
        reopened.reserve(BUCKET, 21_999, 8)?,
        Admission::WaitUntil(22_000)
    );
    assert_eq!(reopened.reserve(BUCKET, 22_000, 8)?, Admission::Granted);
    assert_eq!(
        reopened.reserve(BUCKET, 22_999, 8)?,
        Admission::WaitUntil(23_000)
    );
    Ok(())
}

#[tokio::test]
async fn read_only_onboarding_tracks_independent_repository_settings_after_restart()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("context.sqlite3");
    let store = Arc::new(SqliteStore::open(&path)?);
    let keychain = Arc::new(memory::Credentials::default());
    let app = origin.app(&store, Some(keychain.clone()))?;
    let denied = app
        .clone()
        .oneshot(Request::get("/v1/github/repositories").body(Body::empty())?)
        .await?;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let initial = call(&app, "GET", "/v1/github", Value::Null, 200).await?;
    assert_eq!(initial["credential"], "unconfigured");
    assert_eq!(initial["defaults"]["features"]["currentCode"], true);
    assert_eq!(store.traffic(BUCKET)?.attempts, 0);
    let connected = authorize(&app, 0, FIRST, 200).await?;
    assert_eq!(connected["credential"], "connected");
    assert_eq!(connected["login"], "fixture-user");

    let first = call(&app, "GET", "/v1/github/repositories", Value::Null, 200).await?;
    assert_eq!(first["repositories"][0]["id"], 41);
    assert_eq!(first["nextPage"], 2);
    let second = call(
        &app,
        "GET",
        "/v1/github/repositories?page=2",
        Value::Null,
        200,
    )
    .await?;
    assert_eq!(second["repositories"][0]["id"], 42);
    assert!(second["nextPage"].is_null());

    let alpha = selection(1, "https://github.com/fixture/alpha", 41);
    call(&app, "PUT", "/v1/github/repositories", alpha, 200).await?;
    let mut beta = selection(2, "fixture/beta", 42);
    beta["branch"] = json!("release/v1");
    beta["features"]["currentCode"] = json!(false);
    beta["features"]["history"] = json!(false);
    beta["features"]["codeGraph"] = json!(true);
    call(&app, "PUT", "/v1/github/repositories", beta, 200).await?;
    assert_eq!(store.status()?.connected_sources, 2);
    assert_eq!(store.status()?.indexed_documents, 0);
    let hits = origin.hits()?;
    assert!(hits.contains("GET /repos/fixture/beta/branches/release%2Fv1\n"));
    assert_eq!(
        store.traffic(BUCKET)?.attempts,
        i64::try_from(hits.lines().count())?
    );
    drop(app);
    drop(store);

    let store = Arc::new(SqliteStore::open(&path)?);
    let app = origin.app(&store, Some(keychain))?;
    let saved = call(&app, "GET", "/v1/github", Value::Null, 200).await?;
    assert_eq!(saved["revision"], 3);
    let a = &saved["repositories"][0];
    let b = &saved["repositories"][1];
    assert_eq!(
        a["features"],
        json!({"currentCode":true,"history":true,"actions":true,"codeGraph":false})
    );
    assert_eq!(a["actions"], "available");
    assert_eq!(
        b["features"],
        json!({"currentCode":false,"history":false,"actions":true,"codeGraph":true})
    );
    assert_eq!(b["contents"], "available");
    assert_eq!(b["actions"], "unavailable");
    assert_eq!(b["branch"], "release/v1");
    assert!(
        !fs::read(path)?
            .windows(FIRST.len())
            .any(|window| window == FIRST.as_bytes())
    );
    let remaining = call(
        &app,
        "DELETE",
        "/v1/github/repositories/41?revision=3",
        Value::Null,
        200,
    )
    .await?;
    assert_eq!(remaining["repositories"][0]["repository"]["id"], 42);
    println!(
        "Two repository profiles survived restart; 9 read-only GitHub requests matched the ledger."
    );
    Ok(())
}

#[tokio::test]
async fn a_missing_or_expired_credential_and_replacement_preserve_configuration()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let keychain = Arc::new(memory::Credentials::default());
    let app = origin.app(&store, Some(keychain.clone()))?;
    assert_eq!(
        authorize(&app, 0, "ghp_wrongkind", 400).await?["code"],
        "github_invalid_token"
    );
    assert_eq!(
        authorize(&app, 0, "github_pat_badfixture", 400).await?["code"],
        "github_invalid_token"
    );
    authorize(&app, 0, FIRST, 200).await?;
    let original = store
        .binding()?
        .reference
        .ok_or("fixture binding missing")?;
    assert_eq!(keychain.read(&original)?, FIRST.as_bytes());
    assert_eq!(
        authorize(&app, 0, REPLACEMENT, 409).await?["code"],
        "github_revision_conflict"
    );
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/alpha", 41),
        200,
    )
    .await?;
    origin.expire()?;
    assert_eq!(
        call(&app, "GET", "/v1/github/repositories", Value::Null, 409).await?["code"],
        "github_reconnect"
    );
    let expired = call(&app, "GET", "/v1/github", Value::Null, 200).await?;
    assert_eq!(expired["credential"], "reconnectRequired");
    assert_eq!(expired["repositories"][0]["verified"], false);
    assert_eq!(expired["repositories"][0]["contents"], "unchecked");
    assert_eq!(store.status()?.connected_sources, 0);
    let replaced = authorize(&app, 3, REPLACEMENT, 200).await?;
    let replacement = store
        .binding()?
        .reference
        .ok_or("replacement binding missing")?;
    assert!(keychain.read(&original).is_err());
    store.expire(&original)?;
    assert_eq!(replaced["repositories"][0]["features"]["history"], true);
    assert_eq!(
        store.snapshot()?.credential,
        github::CredentialState::Connected
    );
    keychain.delete(&replacement)?;
    call(&app, "GET", "/v1/github/repositories", Value::Null, 409).await?;
    let disconnected = call(
        &app,
        "DELETE",
        "/v1/github/credential?revision=5",
        Value::Null,
        200,
    )
    .await?;
    assert_eq!(disconnected["credential"], "unconfigured");
    assert_eq!(disconnected["repositories"][0]["repository"]["id"], 41);
    let unavailable = origin.app(&store, None)?;
    assert_eq!(
        authorize(&unavailable, 6, REPLACEMENT, 503).await?["code"],
        "github_keychain_unavailable"
    );
    Ok(())
}

#[tokio::test]
async fn rejects_invalid_local_requests_before_network_access() -> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    for invalid in [
        "https://example.invalid/owner/repo",
        "fixture/../alpha",
        "fixture/alpha?x=y",
    ] {
        call(
            &app,
            "PUT",
            "/v1/github/repositories",
            selection(1, invalid, 41),
            400,
        )
        .await?;
    }
    call(
        &app,
        "GET",
        "/v1/github/repositories?page=0",
        Value::Null,
        400,
    )
    .await?;
    call(
        &app,
        "GET",
        "/v1/github/repositories?page=oops",
        Value::Null,
        400,
    )
    .await?;
    call(
        &app,
        "DELETE",
        "/v1/github/repositories/0?revision=1",
        Value::Null,
        400,
    )
    .await?;
    call(
        &app,
        "DELETE",
        "/v1/github/repositories/alpha?revision=1",
        Value::Null,
        400,
    )
    .await?;
    call(&app, "DELETE", "/v1/github/credential", Value::Null, 400).await?;
    call(&app, "PUT", "/v1/github/credential", Value::Null, 400).await?;
    call(&app, "PUT", "/v1/github/repositories", Value::Null, 400).await?;
    assert_eq!(store.traffic(BUCKET)?.attempts, 1);
    Ok(())
}

#[tokio::test]
async fn paces_denials_and_allows_only_same_origin_redirects() -> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;

    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/alpha", 42),
        409,
    )
    .await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/missing", 41),
        403,
    )
    .await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/broken", 41),
        503,
    )
    .await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/upstream-unavailable", 41),
        503,
    )
    .await?;
    let before_redirect = store.traffic(BUCKET)?.attempts;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/escape", 41),
        503,
    )
    .await?;
    assert_eq!(store.traffic(BUCKET)?.attempts, before_redirect + 1);
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/redirect", 41),
        200,
    )
    .await?;
    assert!(origin.hits()?.contains("GET /repositories/41\n"));
    call(
        &app,
        "GET",
        "/v1/github/repositories?page=3",
        Value::Null,
        429,
    )
    .await?;
    let attempts = store.traffic(BUCKET)?.attempts;
    call(&app, "GET", "/v1/github/repositories", Value::Null, 429).await?;
    assert_eq!(store.traffic(BUCKET)?.attempts, attempts);
    tokio::time::sleep(Duration::from_millis(1_050)).await;
    call(
        &app,
        "GET",
        "/v1/github/repositories?page=4",
        Value::Null,
        429,
    )
    .await?;
    assert_eq!(store.traffic(BUCKET)?.attempts, attempts + 1);
    assert_eq!(
        store.snapshot()?.credential,
        github::CredentialState::Connected
    );
    Ok(())
}

#[tokio::test]
async fn late_verification_cannot_overwrite_a_newer_credential() -> Result<(), Box<dyn Error>> {
    use ctxfab_server::application::github::{GitHubFuture, Provider};
    use ctxfab_server::domain::github::{
        ConfiguredRepository, Discovery, RepositoryName, Selection,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    #[derive(Default)]
    struct PausedProvider {
        started: Notify,
        resume: Notify,
    }

    impl Provider for PausedProvider {
        fn verify<'a>(&'a self, secret: &'a [u8]) -> GitHubFuture<'a, String> {
            Box::pin(async move {
                if secret == FIRST.as_bytes() {
                    self.started.notify_one();
                    self.resume.notified().await;
                    Ok("old-fixture".into())
                } else {
                    Ok("current-fixture".into())
                }
            })
        }

        fn discover<'a>(&'a self, _: &'a [u8], _: u16) -> GitHubFuture<'a, Discovery> {
            Box::pin(std::future::ready(Err(github::Error::Upstream)))
        }

        fn inspect<'a>(
            &'a self,
            _: &'a [u8],
            _: RepositoryName<'a>,
            _: &'a Selection,
        ) -> GitHubFuture<'a, ConfiguredRepository> {
            Box::pin(std::future::ready(Err(github::Error::Upstream)))
        }
    }

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let keychain = Arc::new(memory::Credentials::default());
    let provider = Arc::new(PausedProvider::default());
    let github = Arc::new(GitHub::new(
        provider.clone(),
        store.clone(),
        Some(keychain.clone()),
        || Ok(format!("race/{}", NEXT.fetch_add(1, Ordering::Relaxed))),
    ));
    let earlier = {
        let github = github.clone();
        tokio::spawn(async move { github.authorize(0, FIRST).await })
    };
    tokio::time::timeout(Duration::from_secs(2), provider.started.notified()).await?;
    assert_eq!(
        github.authorize(0, REPLACEMENT).await?.login.as_deref(),
        Some("current-fixture")
    );
    provider.resume.notify_one();
    assert!(matches!(earlier.await?, Err(github::Error::Conflict)));
    assert_eq!(store.snapshot()?.login.as_deref(), Some("current-fixture"));
    assert_eq!(keychain.read("race/0")?, REPLACEMENT.as_bytes());
    assert!(keychain.read("race/1").is_err());
    Ok(())
}

#[tokio::test]
async fn a_successful_response_can_exhaust_the_shared_rate_budget() -> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    call(
        &app,
        "GET",
        "/v1/github/repositories?page=5",
        Value::Null,
        200,
    )
    .await?;
    let cooling = store.traffic(BUCKET)?;
    assert_eq!(
        (cooling.attempts, cooling.pending, cooling.failures),
        (2, 0, 0)
    );
    assert!(cooling.cooldown_until > Clock::system().now());
    call(&app, "GET", "/v1/github/repositories", Value::Null, 429).await?;
    assert_eq!(store.traffic(BUCKET)?.attempts, 2);
    Ok(())
}
