use super::*;
use ctxfab_server::application::{ports::SnapshotStore, sync::CollectionJobs};

const ALPHA_REFRESH: &str = "/v1/github/repositories/41/refresh?background=true";
const SYNC_ALPHA: &str = "/v1/github/repositories/41/refresh";

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
    let credentials = Arc::new(memory::Credentials::default());
    let store = Arc::new(SqliteStore::open(&path)?);
    let github = origin.github(&store, Some(credentials.clone()))?;
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
    call(
        &app,
        "POST",
        "/v1/github/repositories/41/refresh",
        Value::Null,
        200,
    )
    .await?;
    let initial = collection(&app, 41).await?;
    fs::write(origin.directory.path().join("advance-code"), [])?;
    fs::write(origin.directory.path().join("throttle-code"), [])?;
    call(&app, "POST", ALPHA_REFRESH, Value::Null, 202).await?;
    let scheduler = Scheduler::default().with_github(github, Clock::system(), None);
    let waiting = phase(&app, 41, "throttled").await?;
    assert_eq!(waiting["commit"], "a".repeat(40));
    assert_eq!(waiting["publishedAtMs"], initial["publishedAtMs"]);
    let scope = store.current_scope(41)?;
    let staged = store
        .pending_snapshot(&scope.collection)?
        .ok_or("interrupted snapshot")?;
    assert_eq!(staged.revision, "b".repeat(40));
    assert_eq!(
        store
            .load_job(&scope.collection)?
            .ok_or("job")?
            .state
            .failures,
        0
    );
    let cooldown = store.traffic(BUCKET)?.cooldown_until;
    assert!(
        waiting["nextAtMs"]
            .as_i64()
            .is_some_and(|due| due >= cooldown)
    );
    call(&app, "POST", ALPHA_REFRESH, Value::Null, 202).await?;
    assert_eq!(collection(&app, 41).await?["nextAtMs"], waiting["nextAtMs"]);
    scheduler.stop().await;
    let hits_before_restart = origin.hits()?;
    drop(app);
    drop(store);
    fs::remove_file(origin.directory.path().join("throttle-code"))?;
    fs::remove_file(origin.directory.path().join("advance-code"))?;
    let store = Arc::new(SqliteStore::open(&path)?);
    let github = origin.github(&store, Some(credentials))?;
    let app = app_with_github(&store, github.clone());
    let scheduler = Scheduler::default().with_github(github, Clock::system(), None);
    assert_eq!(collection(&app, 41).await?["nextAtMs"], waiting["nextAtMs"]);
    let settled = phase(&app, 41, "succeeded").await?;
    assert_eq!(settled["commit"], "b".repeat(40));
    assert_ne!(settled["publishedAtMs"], initial["publishedAtMs"]);
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
    Ok(())
}

#[tokio::test]
async fn cancelled_sync_request_releases_the_job_to_the_session_worker()
-> Result<(), Box<dyn Error>> {
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
    let scheduler = Scheduler::default().with_github(github, Clock::system(), None);
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
