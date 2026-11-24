use super::*;
use ctxfab_server::adapters::outbound::telemetry::Limits;
use ctxfab_server::application::{ports::SnapshotStore, sync::CollectionJobs};

const ALPHA_REFRESH: &str = "/v1/github/repositories/41/refresh?background=true";
const SYNC_ALPHA: &str = "/v1/github/repositories/41/refresh";
const ALPHA_ACCESS: &str = "/v1/github/repositories/41/access";

async fn collection(app: &Router, id: i64) -> Result<Value, Box<dyn Error>> {
    let state = call(app, "GET", "/v1/github", Value::Null, 200).await?;
    let repos = state["repositories"].as_array().ok_or("repository array")?;
    let configured = repos
        .iter()
        .find(|repo| repo["repository"]["id"] == id)
        .ok_or("saved repository")?;
    Ok(configured["codeCollection"].clone())
}

async fn phase(app: &Router, id: i64, expected: &str) -> Result<Value, Box<dyn Error>> {
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            let status = collection(app, id).await?;
            if status["phase"] == expected {
                return Ok::<_, Box<dyn Error>>(status);
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await?
}

#[tokio::test]
async fn check_access_preserves_settings_and_schedule_and_fences_permission_changes()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    let selection = selection(1, "fixture/alpha", 41);
    call(&app, "PUT", "/v1/github/repositories", selection, 200).await?;
    call(&app, "POST", SYNC_ALPHA, Value::Null, 200).await?;
    let original = call(&app, "GET", "/v1/github", Value::Null, 200).await?;
    assert!(original["repositories"][0]["codeCollection"]["nextAtMs"].is_number());
    let scope = store.current_scope(41)?;
    let access = |revision: u32| format!("{ALPHA_ACCESS}?revision={revision}");

    let before = origin.hits()?;
    let checked = call(&app, "POST", &access(2), Value::Null, 200).await?;
    assert_eq!(checked["revision"], original["revision"]);
    assert_eq!(checked["repositories"], original["repositories"]);
    assert_eq!(store.current_scope(41)?.generation, scope.generation);
    assert_eq!(
        origin.hits()?.strip_prefix(&before),
        Some(
            "GET /repos/fixture/alpha\nGET /repos/fixture/alpha/branches/main\n\
             GET /repos/fixture/alpha/actions/runs?per_page=1\n"
        )
    );

    let marker = origin.directory.path().join("deny-contents");
    fs::write(&marker, [])?;
    let denied = call(&app, "POST", &access(2), Value::Null, 200).await?;
    assert_eq!(denied["revision"], 3);
    let denied = &denied["repositories"][0];
    let previous = &original["repositories"][0];
    for field in ["repository", "branch", "intervalMinutes", "features"] {
        assert_eq!(denied[field], previous[field]);
    }
    assert_eq!(denied["contents"], "unavailable");
    assert!(matches!(
        store.check_scope(&scope),
        Err(github::Error::Conflict)
    ));

    let before = origin.hits()?;
    call(&app, "POST", &access(2), Value::Null, 409).await?;
    assert_eq!(origin.hits()?, before);
    fs::remove_file(marker)?;
    let restored = call(&app, "POST", &access(3), Value::Null, 200).await?;
    assert_eq!(restored["revision"], 4);
    assert_eq!(restored["repositories"][0]["contents"], "available");
    assert_eq!(
        restored["repositories"][0]["codeCollection"]["phase"],
        "queued"
    );
    assert_eq!(
        restored["repositories"][0]["codeCollection"]["commit"],
        previous["codeCollection"]["commit"]
    );
    assert_ne!(store.current_scope(41)?.generation, scope.generation);
    Ok(())
}

#[tokio::test]
async fn refresh_interval_reports_its_bounds_and_rejects_edits_before_contacting_github()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    let mut config = selection(1, "fixture/alpha", 41);
    let saved = call(&app, "PUT", "/v1/github/repositories", config.clone(), 200).await?;
    assert_eq!(
        saved["defaults"]["refreshInterval"],
        json!({"minimumMinutes":15,"maximumMinutes":1440})
    );
    let before = origin.hits()?;
    config["revision"] = saved["revision"].clone();
    config["intervalMinutes"] = json!(10);
    let failure = call(&app, "PUT", "/v1/github/repositories", config.clone(), 400).await?;
    assert_eq!(failure["code"], "github_invalid_interval");
    assert_eq!(failure["field"], "intervalMinutes");
    assert!(
        failure["message"]
            .as_str()
            .is_some_and(|message| message.contains("15"))
    );
    assert_eq!(origin.hits()?, before);
    assert_eq!(
        call(&app, "GET", "/v1/github", Value::Null, 200).await?["revision"],
        saved["revision"]
    );

    config["intervalMinutes"] = json!(15);
    let saved = call(&app, "PUT", "/v1/github/repositories", config, 200).await?;
    assert_eq!(saved["repositories"][0]["intervalMinutes"], 15);
    Ok(())
}

#[tokio::test]
async fn repository_identity_mismatch_stops_before_probing_or_writing() -> Result<(), Box<dyn Error>>
{
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    let before = origin.hits()?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/alpha", 42),
        409,
    )
    .await?;
    assert_eq!(
        origin.hits()?.strip_prefix(&before),
        Some("GET /repos/fixture/alpha\n")
    );
    let state = call(&app, "GET", "/v1/github", Value::Null, 200).await?;
    assert_eq!(state["revision"], 1);
    assert_eq!(state["repositories"], json!([]));
    Ok(())
}

#[tokio::test]
async fn timer_discovers_two_scopes_and_interval_edits_preserve_freshness()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let credentials = Arc::new(memory::Credentials::default());
    let github = origin.github(&store, Some(credentials.clone()))?;
    let app = app_with_github(&store, github.clone());
    let scheduler = Scheduler::default().with_github(github, Clock::system(), None);
    authorize(&app, 0, FIRST, 200).await?;
    let mut alpha = selection(1, "fixture/alpha", 41);
    call(&app, "PUT", "/v1/github/repositories", alpha.clone(), 200).await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(2, "fixture/beta", 42),
        200,
    )
    .await?;
    let first = phase(&app, 41, "succeeded").await?;
    assert_eq!(first["commit"], "a".repeat(40));
    assert_eq!(
        phase(&app, 42, "succeeded").await?["commit"],
        "c".repeat(40)
    );
    let scope = store.current_scope(41)?;
    alpha["revision"] = json!(3);
    alpha["intervalMinutes"] = json!(15);
    call(&app, "PUT", "/v1/github/repositories", alpha, 200).await?;
    let edited = collection(&app, 41).await?;
    assert_eq!(edited["lastCheckedAtMs"], first["lastCheckedAtMs"]);
    assert_eq!(edited["publishedAtMs"], first["publishedAtMs"]);
    assert_eq!(store.current_scope(41)?.generation, scope.generation);
    let due = edited["nextAtMs"].as_i64().ok_or("next check")?;
    assert_eq!(
        due,
        edited["lastCheckedAtMs"].as_i64().ok_or("last check")? + 900_000
    );
    assert_eq!(
        usize::try_from(store.traffic(BUCKET)?.attempts)?,
        origin.hits()?.lines().count()
    );
    scheduler.stop().await;
    fs::write(origin.directory.path().join("advance-code"), [])?;
    let clock = Clock::anchored(due - 200);
    let github = origin.github_at(&store, Some(credentials), clock)?;
    let app = app_with_github(&store, github.clone());
    let scheduler = Scheduler::default().with_github(github, clock, None);
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if collection(&app, 41).await?["commit"] == "b".repeat(40) {
                return Ok::<_, Box<dyn Error>>(());
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    })
    .await??;
    assert_eq!(
        phase(&app, 42, "succeeded").await?["commit"],
        "c".repeat(40)
    );
    scheduler.stop().await;
    Ok(())
}

#[tokio::test]
async fn queued_refresh_resumes_the_same_pin_after_a_cooldown_and_restart()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("context.sqlite3");
    let signals = directory.path().join("diagnostics.sqlite3");
    let credentials = Arc::new(memory::Credentials::default());
    let store = Arc::new(SqliteStore::open(&path)?);
    let github = origin.github(&store, Some(credentials.clone()))?;
    let telemetry = Arc::new(EmbeddedTelemetry::open(&signals, 24, Limits::default())?);
    let app = traced_app(&store, github.clone(), telemetry.clone());
    authorize(&app, 0, FIRST, 200).await?;
    let mut configured = selection(1, "fixture/alpha", 41);
    configured["features"]["history"] = json!(false);
    configured["features"]["actions"] = json!(false);
    call(&app, "PUT", "/v1/github/repositories", configured, 200).await?;
    call(
        &app,
        "POST",
        "/v1/github/repositories/41/refresh",
        Value::Null,
        200,
    )
    .await?;
    let initial = collection(&app, 41).await?;
    let before_collection = origin.hits()?;
    fs::write(origin.directory.path().join("advance-code"), [])?;
    fs::write(origin.directory.path().join("throttle-code"), [])?;
    let (original_trace, _) = queued_trace(&app).await?;
    let scheduler =
        Scheduler::default().with_github(github, Clock::system(), Some(telemetry.clone()));
    let waiting = phase(&app, 41, "throttled").await?;
    assert_eq!(waiting["commit"], "a".repeat(40));
    assert_eq!(waiting["publishedAtMs"], initial["publishedAtMs"]);
    let scope = store.current_scope(41)?;
    assert_throttled_stage(&store, &scope.collection, &waiting)?;
    let cooldown = store.traffic(BUCKET)?.cooldown_until;
    assert!(
        waiting["nextAtMs"]
            .as_i64()
            .is_some_and(|due| due >= cooldown)
    );
    let (joined_trace, work_trace) = queued_trace(&app).await?;
    assert_ne!(joined_trace, original_trace);
    assert_eq!(work_trace.as_deref(), Some(original_trace.as_str()));
    assert_eq!(collection(&app, 41).await?["nextAtMs"], waiting["nextAtMs"]);
    scheduler.stop().await;
    telemetry.shutdown();
    let hits_before_restart = origin.hits()?;
    drop(app);
    drop(store);
    fs::remove_file(origin.directory.path().join("throttle-code"))?;
    fs::remove_file(origin.directory.path().join("advance-code"))?;
    let store = Arc::new(SqliteStore::open(&path)?);
    let github = origin.github(&store, Some(credentials))?;
    let telemetry = Arc::new(EmbeddedTelemetry::open(&signals, 24, Limits::default())?);
    let app = traced_app(&store, github.clone(), telemetry.clone());
    assert_eq!(collection(&app, 41).await?, waiting);
    let scheduler =
        Scheduler::default().with_github(github, Clock::system(), Some(telemetry.clone()));
    let settled = phase(&app, 41, "succeeded").await?;
    assert_eq!(settled["commit"], "b".repeat(40));
    assert_ne!(settled["publishedAtMs"], initial["publishedAtMs"]);
    assert_eq!(settled["publishedFiles"], 2);
    assert!(settled["pending"].is_null());
    assert!(store.pending_snapshot(&scope.collection)?.is_none());
    let hits_after_restart = origin.hits()?;
    let resumed = hits_after_restart
        .strip_prefix(&hits_before_restart)
        .ok_or("fixture log changed")?;
    assert!(!resumed.contains("/branches/main"));
    assert_eq!(
        resumed
            .lines()
            .filter(|line| line.contains("/git/blobs/"))
            .count(),
        1
    );
    assert_eq!(
        usize::try_from(store.traffic(BUCKET)?.attempts)?,
        hits_after_restart.lines().count()
    );
    scheduler.stop().await;
    telemetry.shutdown();
    let requests = hits_after_restart
        .strip_prefix(&before_collection)
        .ok_or("fixture log changed before refresh")?;
    assert_collection_trace(&app, &original_trace, requests.lines().count()).await?;
    assert_related_trace(&app, &joined_trace, &original_trace).await?;
    assert_origin_released(&path)
}

fn assert_throttled_stage(
    store: &SqliteStore,
    collection: &str,
    waiting: &Value,
) -> Result<(), Box<dyn Error>> {
    assert_eq!(
        waiting["pending"],
        json!({
            "commit": "b".repeat(40), "stagedFiles": 1, "phase": "collecting"
        })
    );
    let staged = store
        .pending_snapshot(collection)?
        .ok_or("interrupted snapshot")?;
    assert_eq!(staged.revision, "b".repeat(40));
    assert_eq!(store.load_job(collection)?.ok_or("job")?.state.failures, 0);
    Ok(())
}

fn assert_origin_released(path: &Path) -> Result<(), Box<dyn Error>> {
    let remaining: i64 = rusqlite::Connection::open(path)?.query_row(
        "SELECT count(*) FROM sync_jobs WHERE github_current_origin IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        remaining, 0,
        "terminal publication releases its job correlation"
    );
    Ok(())
}

#[tokio::test]
async fn measured_progress_counts_committed_files_and_is_fenced_by_repository_purge()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    for (revision, name, id) in [(1, "fixture/alpha", 41), (2, "fixture/beta", 42)] {
        let mut config = selection(revision, name, id);
        config["features"]["history"] = json!(false);
        config["features"]["actions"] = json!(false);
        call(&app, "PUT", "/v1/github/repositories", config, 200).await?;
    }
    call(&app, "POST", SYNC_ALPHA, Value::Null, 200).await?;
    let published = collection(&app, 41).await?;
    assert_eq!(published["commit"], "a".repeat(40));
    assert_eq!(published["publishedFiles"], 2);
    assert!(published["pending"].is_null());

    let hold = origin.directory.path().join("hold-code-blob");
    fs::write(&hold, [])?;
    fs::write(origin.directory.path().join("advance-code"), [])?;
    let caller = app.clone();
    let refresh = tokio::spawn(async move {
        call(&caller, "POST", SYNC_ALPHA, Value::Null, 409)
            .await
            .map_err(|error| error.to_string())
    });
    let entered = origin.directory.path().join("held-code-blob");
    tokio::time::timeout(Duration::from_secs(3), async {
        while !entered.is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;

    let hits = origin.hits()?;
    let running = collection(&app, 41).await?;
    assert_eq!(running["phase"], "running");
    assert_eq!(running["commit"], published["commit"]);
    assert_eq!(running["publishedAtMs"], published["publishedAtMs"]);
    assert_eq!(running["publishedFiles"], 2);
    assert_eq!(
        running["pending"],
        json!({
            "commit": "b".repeat(40),
            "stagedFiles": 1,
            "phase": "collecting"
        })
    );
    let beta = collection(&app, 42).await?;
    assert_eq!(beta["phase"], "queued");
    assert!(beta["pending"].is_null());
    call(&app, "POST", ALPHA_REFRESH, Value::Null, 202).await?;
    assert_eq!(collection(&app, 41).await?["pending"], running["pending"]);
    assert_eq!(origin.hits()?, hits);
    assert_eq!(
        usize::try_from(store.traffic(BUCKET)?.attempts)?,
        hits.lines().count()
    );

    call(
        &app,
        "DELETE",
        "/v1/github/repositories/41/data",
        Value::Null,
        200,
    )
    .await?;
    fs::remove_file(hold)?;
    assert_eq!(refresh.await??["code"], "github_revision_conflict");
    let purged = collection(&app, 41).await?;
    assert!(purged["pending"].is_null());
    assert!(purged["publishedFiles"].is_null());
    assert!(purged["commit"].is_null());
    assert_eq!(collection(&app, 42).await?, beta);
    assert_eq!(origin.hits()?, hits);
    Ok(())
}

#[tokio::test]
async fn background_refresh_can_join_an_already_running_timer() -> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let directory = tempfile::tempdir()?;
    let telemetry = Arc::new(EmbeddedTelemetry::open(
        &directory.path().join("diagnostics.sqlite3"),
        24,
        Limits::default(),
    )?);
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let github = origin.github(&store, Some(Arc::new(memory::Credentials::default())))?;
    let app = traced_app(&store, github.clone(), telemetry.clone());
    authorize(&app, 0, FIRST, 200).await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/alpha", 41),
        200,
    )
    .await?;

    let hold = origin.directory.path().join("hold-code-tree");
    fs::write(&hold, [])?;
    fs::write(origin.directory.path().join("advance-code"), [])?;
    let scheduler =
        Scheduler::default().with_github(github, Clock::system(), Some(telemetry.clone()));
    let entered = origin.directory.path().join("held-code-tree");
    tokio::time::timeout(Duration::from_secs(3), async {
        while !entered.is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    let (request, work) = queued_trace(&app).await?;
    let work = work.ok_or("work trace for timer")?;
    assert_ne!(request, work);
    fs::remove_file(hold)?;
    assert_eq!(
        phase(&app, 41, "succeeded").await?["commit"],
        "b".repeat(40)
    );
    scheduler.stop().await;
    telemetry.shutdown();

    let page = call(
        &app,
        "GET",
        &format!("/v1/telemetry/traces/{work}"),
        Value::Null,
        200,
    )
    .await?;
    let spans = page["spans"].as_array().ok_or("job spans")?;
    let root = spans
        .iter()
        .find(|span| span["name"] == "github_current_job")
        .ok_or("timer root")?;
    let attempt = spans
        .iter()
        .find(|span| span["name"] == "github_current_collection")
        .ok_or("collection")?;
    assert_eq!(attempt["parentSpanId"], root["spanId"]);
    assert_eq!(attempt["level"], "ok");
    assert_related_trace(&app, &request, &work).await
}

async fn assert_related_trace(
    app: &Router,
    request: &str,
    work: &str,
) -> Result<(), Box<dyn Error>> {
    let page = call(
        app,
        "GET",
        &format!("/v1/telemetry/traces/{request}"),
        Value::Null,
        200,
    )
    .await?;
    let spans = page["spans"].as_array().ok_or("request spans")?;
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0]["httpStatus"], 202);
    assert_eq!(spans[0]["workTraceId"], work);
    Ok(())
}

async fn queued_trace(app: &Router) -> Result<(String, Option<String>), Box<dyn Error>> {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(ALPHA_REFRESH)
                .header("Authorization", "Bearer local-session-fixture")
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(response.status().as_u16(), 202);
    let trace = response.headers()["x-context-trace-id"]
        .to_str()?
        .to_owned();
    let work_trace = response
        .headers()
        .get("x-context-work-trace-id")
        .map(|id| id.to_str().map(str::to_owned))
        .transpose()?;
    Ok((trace, work_trace))
}

async fn assert_collection_trace(
    app: &Router,
    id: &str,
    dispatched: usize,
) -> Result<(), Box<dyn Error>> {
    let page = call(
        app,
        "GET",
        &format!("/v1/telemetry/traces/{id}"),
        Value::Null,
        200,
    )
    .await?;
    let spans = page["spans"].as_array().ok_or("trace spans")?;
    let root = spans
        .iter()
        .find(|span| span["httpStatus"] == 202)
        .ok_or("root")?;
    let attempts: Vec<_> = spans
        .iter()
        .filter(|span| span["name"] == "github_current_collection")
        .collect();
    assert_eq!(attempts.len(), 2);
    assert!(
        attempts
            .iter()
            .all(|span| span["parentSpanId"] == root["spanId"])
    );
    assert_eq!(
        attempts
            .iter()
            .filter(|span| span["level"] == "error")
            .count(),
        1
    );
    assert_eq!(
        attempts.iter().filter(|span| span["level"] == "ok").count(),
        1
    );
    assert_ne!(attempts[0]["spanId"], attempts[1]["spanId"]);
    let events: Vec<_> = attempts
        .iter()
        .map(|span| span["events"][0]["name"].as_str())
        .collect();
    assert!(events.contains(&Some("Collection waiting on provider cooldown")));
    assert!(events.contains(&Some("Collection publication updated")));
    let hops: Vec<_> = spans
        .iter()
        .filter(|span| span["name"] == "github_provider_request")
        .collect();
    assert_eq!(hops.len(), dispatched);
    assert!(hops.iter().all(|hop| {
        hop["httpMethod"] == "GET"
            && hop["value"].as_i64().is_some_and(|duration| duration >= 0)
            && attempts
                .iter()
                .any(|attempt| attempt["spanId"] == hop["parentSpanId"])
    }));
    assert!(hops.iter().any(|hop| {
        hop["httpStatus"] == 429
            && hop["level"] == "error"
            && hop["httpRoute"] == "/repos/{owner}/{repo}/git/blobs/{sha}"
    }));
    assert!(!page.to_string().contains("fixture/alpha"));
    assert!(!page.to_string().contains(FIRST));
    Ok(())
}

#[tokio::test]
async fn cancelled_sync_request_releases_the_job_to_the_session_worker()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let github = origin.github(&store, Some(Arc::new(memory::Credentials::default())))?;
    let directory = tempfile::tempdir()?;
    let signals = directory.path().join("diagnostics.sqlite3");
    let telemetry = Arc::new(EmbeddedTelemetry::open(&signals, 24, Limits::default())?);
    let app = traced_app(&store, github.clone(), telemetry.clone());
    authorize(&app, 0, FIRST, 200).await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/alpha", 41),
        200,
    )
    .await?;
    call(&app, "POST", SYNC_ALPHA, Value::Null, 200).await?;
    let scheduler =
        Scheduler::default().with_github(github, Clock::system(), Some(telemetry.clone()));
    fs::write(origin.directory.path().join("advance-code"), [])?;
    let hold = origin.directory.path().join("hold-code-tree");
    fs::write(&hold, [])?;
    let caller = app.clone();
    let request = tokio::spawn(async move {
        call(&caller, "POST", SYNC_ALPHA, Value::Null, 200)
            .await
            .map_err(|error| error.to_string())
    });
    let entered = origin.directory.path().join("held-code-tree");
    tokio::time::timeout(Duration::from_secs(3), async {
        while !entered.is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    assert_eq!(phase(&app, 41, "running").await?["commit"], "a".repeat(40));
    request.abort();
    assert!(request.await.is_err_and(|error| error.is_cancelled()));
    fs::remove_file(hold)?;
    let completed = phase(&app, 41, "succeeded").await?;
    assert_eq!(completed["commit"], "b".repeat(40));
    let traffic = store.traffic(BUCKET)?;
    assert_eq!(traffic.uncertain, 1);
    assert_eq!(
        usize::try_from(traffic.attempts)?,
        origin.hits()?.lines().count()
    );
    scheduler.stop().await;
    telemetry.shutdown();
    assert_interrupted_provider_request(&app, &signals).await
}

async fn assert_interrupted_provider_request(
    app: &Router,
    signals: &Path,
) -> Result<(), Box<dyn Error>> {
    let db = rusqlite::Connection::open(signals)?;
    let (trace_id, span_id, count): (String, String, u32) = db.query_row(
        "SELECT trace_id, span_id, count(*) OVER ()
         FROM signals WHERE signal='trace' AND name='github_provider_request' AND level IS NULL",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(count, 1);
    let page = call(
        app,
        "GET",
        &format!("/v1/telemetry/traces/{trace_id}"),
        Value::Null,
        200,
    )
    .await?;
    let spans = page["spans"].as_array().ok_or("interrupted trace spans")?;
    let span = spans
        .iter()
        .find(|span| span["spanId"] == span_id)
        .ok_or("interrupted provider span detail")?;
    assert_eq!(span["httpRoute"], "/repos/{owner}/{repo}/git/trees/{sha}");
    assert!(span.get("level").is_none());
    assert!(span.get("httpStatus").is_none());
    Ok(())
}

#[tokio::test]
async fn an_expired_pin_is_reconciled_without_hiding_the_previous_commit()
-> Result<(), Box<dyn Error>> {
    use ctxfab_server::domain::corpus::snapshot::Plan;

    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let github = origin.github(&store, Some(Arc::new(memory::Credentials::default())))?;
    let app = app_with_github(&store, github.clone());
    authorize(&app, 0, FIRST, 200).await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(1, "fixture/alpha", 41),
        200,
    )
    .await?;
    call(&app, "POST", SYNC_ALPHA, Value::Null, 200).await?;
    let scope = store.current_scope(41)?;
    let stranded = store.begin_snapshot(&Plan {
        collection: &scope.collection,
        generation: scope.generation,
        predecessor: scope.predecessor,
        revision: &"d".repeat(40),
        tracked_ref: "refs/heads/main",
    })?;
    store.append_snapshot(stranded.id, 0, None, &[])?;
    fs::write(origin.directory.path().join("advance-code"), [])?;
    let response = call(&app, "POST", SYNC_ALPHA, Value::Null, 409).await?;
    assert_eq!(response["code"], "github_expired_pin");
    assert_eq!(phase(&app, 41, "retry").await?["commit"], "a".repeat(40));
    assert!(store.pending_snapshot(&scope.collection)?.is_none());
    let scheduler = Scheduler::default().with_github(github, Clock::system(), None);
    assert_eq!(
        phase(&app, 41, "succeeded").await?["commit"],
        "b".repeat(40)
    );
    scheduler.stop().await;
    Ok(())
}
