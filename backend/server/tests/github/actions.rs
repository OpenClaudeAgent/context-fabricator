use super::{
    EmbeddedTelemetry, FIRST, Origin, assert_provider_hops, authorize, call, memory,
    post_with_trace, repository_trace_activity, retained_trace, selection, traced_app,
};
use ctxfab_server::adapters::outbound::sqlite::SqliteStore;
use ctxfab_server::adapters::outbound::telemetry::Limits;
use ctxfab_server::application::{
    github::actions::{Store, Trigger},
    sync::TrafficLedger,
};
use ctxfab_server::domain::github;
use serde_json::{Value, json};
use std::{error::Error, sync::Arc, time::Duration};

const ACTIONS: &str = "/v1/github/repositories/41/actions";

#[tokio::test]
async fn actions_retains_the_latest_attempts_when_the_window_fills() -> Result<(), Box<dyn Error>> {
    use ctxfab_server::runtime::clock::Clock;

    let origin = Origin::start().await?;
    for flag in ["actions-fixture", "collect-history"] {
        std::fs::write(origin.directory.path().join(flag), [])?;
    }
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db.sqlite3");
    let store = Arc::new(SqliteStore::open(&path)?);
    let clock = Clock::anchored(1_800_000_000_000);
    let github = origin.github_at(
        &store,
        Some(Arc::new(memory::Credentials::default())),
        clock,
    )?;
    let app = super::app_with_github(&store, github.clone());
    authorize(&app, 0, FIRST, 200).await?;
    let mut config = selection(1, "fixture/alpha", 41);
    config["features"] =
        json!({"currentCode":false,"history":false,"actions":true,"codeGraph":false});
    call(&app, "PUT", "/v1/github/repositories", config, 200).await?;

    let mut previous = github.refresh_actions(41, None).await?;
    let template = previous.runs.pop().ok_or("fixture workflow run")?;
    let run_id = template.id;
    let (expired, tied) = (1_497, 1_498);
    previous.runs = (1_000..=tied)
        .map(|id| {
            let mut run = template.clone();
            run.id = id;
            run.created_at = "2025-12-31T00:00:00Z".into();
            run.updated_at = if id >= expired {
                run.created_at.clone()
            } else {
                "2025-12-31T00:01:00Z".into()
            };
            run
        })
        .collect();
    assert_eq!(previous.runs.len(), 499);
    let scope = store.actions_scope(41)?;
    let due = store.actions_due(Some(41))?.ok_or("Actions cadence")?;
    store.publish_actions(&scope, &due, &previous, clock.now(), None)?;

    let full = github.refresh_actions(41, None).await?;
    assert_eq!(full.runs.len(), 500);
    assert!(full.runs.iter().any(|run| run.id == expired));
    assert!(full.runs.iter().any(|run| run.id == tied));

    std::fs::write(origin.directory.path().join("actions-rerun"), [])?;
    let rolled = github.refresh_actions(41, None).await?;
    assert_eq!(rolled.runs.len(), 500);
    assert!(rolled.runs.iter().all(|run| run.id != expired));
    assert!(rolled.runs.iter().any(|run| run.id == tied));
    assert_eq!(
        rolled
            .runs
            .iter()
            .filter(|run| run.id == run_id)
            .map(|run| (run.attempt, run.commit_provenance.as_str()))
            .collect::<Vec<_>>(),
        [
            (1, "verifiedRepositoryCommit"),
            (2, "forkOrUnknownReportedHead")
        ]
    );
    assert_eq!(rolled.attempt_coverage, "observedLatestAttemptsOnly");

    drop(app);
    drop(github);
    drop(store);
    let reopened = SqliteStore::open(&path)?;
    assert_eq!(
        serde_json::to_value(reopened.actions_state(41)?.snapshot)?,
        serde_json::to_value(rolled)?
    );
    Ok(())
}

#[tokio::test]
async fn actions_accepts_a_full_run_page_with_large_metadata_within_its_bound()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    for flag in ["actions-fixture", "actions-large-page", "collect-history"] {
        std::fs::write(origin.directory.path().join(flag), [])?;
    }
    let directory = tempfile::tempdir()?;
    let store = Arc::new(SqliteStore::open(&directory.path().join("db.sqlite3"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    let mut config = selection(1, "fixture/alpha", 41);
    config["features"] =
        json!({"currentCode":false,"history":false,"actions":true,"codeGraph":false});
    call(&app, "PUT", "/v1/github/repositories", config, 200).await?;

    let snapshot = call(&app, "POST", ACTIONS, Value::Null, 200).await?;
    assert_eq!(snapshot["runCoverage"], "latest100");
    let runs = snapshot["runs"].as_array().ok_or("runs")?;
    assert_eq!(runs.len(), 100);
    assert_eq!(runs[0]["id"], 30);
    assert_eq!(runs[99]["id"], 129);
    assert_eq!(runs[0]["commitProvenance"], "verifiedRepositoryCommit");
    assert!(runs.iter().all(|run| run.get("head_commit").is_none()));
    assert_eq!(
        origin
            .hits()?
            .lines()
            .filter(|line| line.ends_with("/actions/runs?branch=main&per_page=100&page=1"))
            .count(),
        1
    );

    std::fs::write(origin.directory.path().join("actions-over-limit"), [])?;
    call(&app, "POST", ACTIONS, Value::Null, 503).await?;
    let retained = call(&app, "GET", ACTIONS, Value::Null, 200).await?;
    assert_eq!(retained["snapshot"], snapshot);
    Ok(())
}

#[tokio::test]
async fn manual_actions_refresh_advances_the_schedule_for_its_published_snapshot()
-> Result<(), Box<dyn Error>> {
    use ctxfab_server::runtime::clock::Clock;
    let origin = Origin::start().await?;
    std::fs::write(origin.directory.path().join("actions-fixture"), [])?;
    let directory = tempfile::tempdir()?;
    let db = directory.path().join("db.sqlite3");
    let store = Arc::new(SqliteStore::open(&db)?);
    let clock = Clock::anchored(1_800_000_000_000);
    let github = origin.github_at(
        &store,
        Some(Arc::new(memory::Credentials::default())),
        clock,
    )?;
    let app = super::app_with_github(&store, github.clone());
    authorize(&app, 0, FIRST, 200).await?;
    let mut config = selection(1, "fixture/alpha", 41);
    config["features"] =
        json!({"currentCode":false,"history":false,"actions":true,"codeGraph":false});
    call(&app, "PUT", "/v1/github/repositories", config, 200).await?;
    let before = github.next_actions()?.ok_or("initial Actions schedule")?;
    assert_eq!(before.at_ms, 0);

    let hold = origin.directory.path().join("hold-history-branch");
    std::fs::write(&hold, [])?;
    let entered = origin.directory.path().join("held-history-branch");
    let manual = {
        let app = app.clone();
        tokio::spawn(async move {
            call(&app, "POST", ACTIONS, Value::Null, 200)
                .await
                .map_err(|error| error.to_string())
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while !entered.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    let scheduled = github.run_actions(&before, None);
    tokio::pin!(scheduled);
    let wait = tokio::time::timeout(Duration::ZERO, &mut scheduled).await;
    std::fs::remove_file(hold)?;
    assert!(
        wait.is_err(),
        "scheduled collection must wait for the owner"
    );
    let snapshot = manual.await??;
    let hits = origin.hits()?;
    let attempts = store.traffic(github::BUCKET)?.attempts;
    assert_eq!(scheduled.await, Err(github::Error::Conflict));
    let state = call(&app, "GET", ACTIONS, Value::Null, 200).await?;
    let after = github.next_actions()?.ok_or("updated Actions schedule")?;
    assert_eq!(state["snapshot"], snapshot);
    assert_eq!(after.generation, before.generation);
    assert!(after.at_ms > clock.now());
    assert_eq!(state["schedule"]["nextAtMs"], after.at_ms);
    assert_eq!(
        hits.lines()
            .filter(|line| line.starts_with("GET /repos/fixture/alpha/actions/runs?"))
            .count(),
        2,
        "one permissions check and one actual workflow run listing"
    );
    assert_eq!(
        github.run_actions(&after, None).await,
        Err(github::Error::Conflict)
    );
    assert_eq!(
        store.actions_failed(
            &before,
            clock.now(),
            github::Error::Throttled,
            Trigger::Scheduled,
        ),
        Err(github::Error::Conflict)
    );
    assert_eq!(origin.hits()?, hits);
    assert_eq!(store.traffic(github::BUCKET)?.attempts, attempts);
    assert_eq!(call(&app, "GET", ACTIONS, Value::Null, 200).await?, state);
    Ok(())
}

#[tokio::test]
async fn actions_preserve_attempts_and_guard_branch_scope() -> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    std::fs::write(origin.directory.path().join("actions-fixture"), [])?;
    std::fs::write(origin.directory.path().join("actions-dynamic-workflow"), [])?;
    std::fs::write(origin.directory.path().join("collect-history"), [])?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db.sqlite3");
    let store = Arc::new(SqliteStore::open(&path)?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    let mut config = selection(1, "fixture/alpha", 41);
    config["features"] =
        json!({"currentCode":false,"history":false,"actions":true,"codeGraph":false});
    call(&app, "PUT", "/v1/github/repositories", config.clone(), 200).await?;
    assert_eq!(
        call(&app, "GET", ACTIONS, Value::Null, 200).await?,
        json!({"availability":"ready"})
    );
    let before = store.traffic(github::BUCKET)?.attempts;
    let first = call(&app, "POST", ACTIONS, Value::Null, 200).await?;
    assert_eq!(first["repositoryId"], 41);
    assert_eq!(first["repository"], "fixture/alpha");
    assert_eq!(first["definitionCommit"], "a".repeat(40));
    assert_eq!(first["workflows"][0]["definition"], "name: CI\n");
    assert_eq!(
        first["workflows"][1],
        json!({
            "id": 21, "name": "Dependabot Updates",
            "path": "dynamic/dependabot/dependabot-updates", "state": "active",
            "definitionBlob": null, "definition": null,
        })
    );
    assert_eq!(first["runs"][0]["attempt"], 1);
    assert_eq!(first["runs"][0]["status"], "queued");
    assert_eq!(
        first["runs"][0]["commitProvenance"],
        "verifiedRepositoryCommit"
    );
    assert!(store.traffic(github::BUCKET)?.attempts > before);
    std::fs::write(origin.directory.path().join("actions-completed"), [])?;
    let completed = call(&app, "POST", ACTIONS, Value::Null, 200).await?;
    assert_eq!(completed["runs"][0]["status"], "completed");
    std::fs::write(origin.directory.path().join("actions-stale"), [])?;
    let stale = call(&app, "POST", ACTIONS, Value::Null, 200).await?;
    assert_eq!(stale["runs"][0]["status"], "completed");
    std::fs::remove_file(origin.directory.path().join("actions-stale"))?;
    std::fs::write(origin.directory.path().join("actions-rerun"), [])?;
    let next = call(&app, "POST", ACTIONS, Value::Null, 200).await?;
    assert_eq!(next["runs"].as_array().ok_or("runs")?.len(), 2);
    assert_eq!(next["runs"][1]["attempt"], 2);
    assert_eq!(
        next["runs"][1]["commitProvenance"],
        "forkOrUnknownReportedHead"
    );
    std::fs::write(origin.directory.path().join("actions-bad-blob"), [])?;
    call(&app, "POST", ACTIONS, Value::Null, 503).await?;
    assert_eq!(
        call(&app, "GET", ACTIONS, Value::Null, 200).await?["snapshot"],
        next
    );
    std::fs::remove_file(origin.directory.path().join("actions-bad-blob"))?;
    std::fs::write(origin.directory.path().join("actions-other-branch"), [])?;
    call(&app, "POST", ACTIONS, Value::Null, 503).await?;
    assert_eq!(
        call(&app, "GET", ACTIONS, Value::Null, 200).await?["snapshot"],
        next
    );
    std::fs::remove_file(origin.directory.path().join("actions-other-branch"))?;
    let scope = store.actions_scope(41)?;
    let due = store.actions_due(Some(41))?.ok_or("Actions cadence")?;
    config["revision"] = json!(2);
    config["features"]["actions"] = json!(false);
    call(&app, "PUT", "/v1/github/repositories", config, 200).await?;
    assert!(matches!(
        store.publish_actions(
            &scope,
            &due,
            &serde_json::from_value(next.clone())?,
            due.at_ms,
            None,
        ),
        Err(github::Error::Conflict)
    ));
    call(&app, "POST", ACTIONS, Value::Null, 403).await?;
    assert_eq!(
        call(&app, "GET", ACTIONS, Value::Null, 200).await?["availability"],
        "disabled"
    );
    drop(app);
    drop(store);
    let reopened = Arc::new(SqliteStore::open(&path)?);
    assert_eq!(
        reopened
            .actions_state(41)?
            .snapshot
            .ok_or("snapshot")?
            .runs
            .len(),
        2
    );
    Ok(())
}

#[tokio::test]
async fn manual_actions_failure_reports_latest_check_without_rescheduling()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let bad_blob = origin.directory.path().join("actions-bad-blob");
    for flag in ["actions-fixture", "collect-history"] {
        std::fs::write(origin.directory.path().join(flag), [])?;
    }
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db.sqlite3");
    let store = Arc::new(SqliteStore::open(&path)?);
    let credentials = Arc::new(memory::Credentials::default());
    let app = origin.app(&store, Some(credentials.clone()))?;
    authorize(&app, 0, FIRST, 200).await?;
    let mut config = selection(1, "fixture/alpha", 41);
    config["features"] =
        json!({"currentCode":false,"history":false,"actions":true,"codeGraph":false});
    call(&app, "PUT", "/v1/github/repositories", config, 200).await?;

    let initial_due = store.actions_due(Some(41))?.ok_or("initial due")?;
    std::fs::write(&bad_blob, [])?;
    call(&app, "POST", ACTIONS, Value::Null, 503).await?;
    let first_failure = call(&app, "GET", ACTIONS, Value::Null, 200).await?;
    assert!(first_failure.get("snapshot").is_none());
    assert_eq!(first_failure["schedule"]["lastError"], "unavailable");
    assert_eq!(store.actions_due(Some(41))?, Some(initial_due));

    std::fs::remove_file(&bad_blob)?;
    let snapshot = call(&app, "POST", ACTIONS, Value::Null, 200).await?;
    let healthy = call(&app, "GET", ACTIONS, Value::Null, 200).await?;
    assert!(healthy["schedule"].get("lastError").is_none());
    let due = store.actions_due(Some(41))?.ok_or("published schedule")?;
    std::fs::write(&bad_blob, [])?;
    call(&app, "POST", ACTIONS, Value::Null, 503).await?;
    let failed = call(&app, "GET", ACTIONS, Value::Null, 200).await?;
    assert_eq!(failed["availability"], "ready");
    assert_eq!(failed["snapshot"], snapshot);
    assert_eq!(failed["schedule"]["lastError"], "unavailable");
    assert_eq!(
        failed["schedule"]["nextAtMs"],
        healthy["schedule"]["nextAtMs"]
    );
    assert!(
        failed["schedule"]["lastCheckedAtMs"].as_i64()
            > healthy["schedule"]["lastCheckedAtMs"].as_i64()
    );
    assert_eq!(store.actions_due(Some(41))?, Some(due));

    drop(app);
    drop(store);
    let reopened = Arc::new(SqliteStore::open(&path)?);
    let restored = origin.app(&reopened, Some(credentials))?;
    assert_eq!(
        call(&restored, "GET", ACTIONS, Value::Null, 200).await?,
        failed
    );
    assert_eq!(reopened.actions_due(Some(41))?, Some(due));
    std::fs::remove_file(&bad_blob)?;
    std::fs::write(origin.directory.path().join("actions-completed"), [])?;
    let recovered = call(&restored, "POST", ACTIONS, Value::Null, 200).await?;
    assert_eq!(recovered["runs"][0]["status"], "completed");
    let state = call(&restored, "GET", ACTIONS, Value::Null, 200).await?;
    assert_eq!(state["snapshot"], recovered);
    assert!(state["schedule"].get("lastError").is_none());
    Ok(())
}

#[tokio::test]
async fn actions_trace_tracks_failed_attempt_and_retained_snapshot() -> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    std::fs::write(origin.directory.path().join("actions-fixture"), [])?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db.sqlite3");
    let store = Arc::new(SqliteStore::open(&path)?);
    let credentials = Arc::new(memory::Credentials::default());
    let diagnostics = directory.path().join("diagnostics.sqlite3");
    let telemetry = Arc::new(EmbeddedTelemetry::open(
        &diagnostics,
        24,
        Limits::default(),
    )?);
    let app = traced_app(
        &store,
        origin.github(&store, Some(credentials.clone()))?,
        telemetry.clone(),
    );
    authorize(&app, 0, FIRST, 200).await?;
    let mut config = selection(1, "fixture/alpha", 41);
    config["features"] =
        json!({"currentCode":false,"history":false,"actions":true,"codeGraph":false});
    call(&app, "PUT", "/v1/github/repositories", config.clone(), 200).await?;
    let key = "actionsActivity";
    let (first, first_trace, _) = post_with_trace(&app, ACTIONS, 200).await?;
    let first_activity = repository_trace_activity(&app, key).await?;
    assert_eq!(first_activity["publishedSnapshot"]["traceId"], first_trace);
    assert_eq!(
        first_activity["latestAttempt"],
        first_activity["publishedSnapshot"]
    );
    let (identical, producing_trace, _) = post_with_trace(&app, ACTIONS, 200).await?;
    assert_eq!(identical, first);
    assert_ne!(producing_trace, first_trace);

    std::fs::write(origin.directory.path().join("actions-bad-blob"), [])?;
    let (_, failed_trace, _) = post_with_trace(&app, ACTIONS, 503).await?;
    let failed = repository_trace_activity(&app, key).await?;
    assert_eq!(failed["latestAttempt"]["traceId"], failed_trace);
    assert_eq!(failed["publishedSnapshot"]["traceId"], producing_trace);
    assert_ne!(failed["latestAttempt"], failed["publishedSnapshot"]);
    let failed_state = call(&app, "GET", ACTIONS, Value::Null, 200).await?;
    assert_eq!(failed_state["snapshot"], first);
    assert_eq!(failed_state["schedule"]["lastError"], "unavailable");

    config["revision"] = json!(2);
    config["features"]["actions"] = json!(false);
    call(&app, "PUT", "/v1/github/repositories", config, 200).await?;
    assert_eq!(
        post_with_trace(&app, ACTIONS, 403).await?.0["code"],
        "github_access_not_verified"
    );
    let disabled = repository_trace_activity(&app, key).await?;
    assert!(disabled["latestAttempt"].is_null());
    assert_eq!(disabled["publishedSnapshot"], failed["publishedSnapshot"]);
    telemetry.shutdown();
    let trace = call(
        &app,
        "GET",
        &format!("/v1/telemetry/traces/{failed_trace}"),
        Value::Null,
        200,
    )
    .await?;
    let span = trace["spans"]
        .as_array()
        .and_then(|spans| {
            spans
                .iter()
                .find(|span| span["name"] == "github_actions_collection")
        })
        .ok_or("failed Actions collection span")?;
    assert_eq!(span["level"], "error");
    assert_eq!(span["problemCode"], failed_state["schedule"]["lastError"]);
    drop(app);
    drop(store);

    let reopened = Arc::new(SqliteStore::open(&path)?);
    let restored = origin.app(&reopened, Some(credentials))?;
    assert_eq!(repository_trace_activity(&restored, key).await?, disabled);
    call(
        &restored,
        "DELETE",
        "/v1/github/repositories/41/data",
        Value::Null,
        200,
    )
    .await?;
    assert!(repository_trace_activity(&restored, key).await?.is_null());
    Ok(())
}

#[tokio::test]
async fn actions_only_schedule_runs_and_persists_next_due() -> Result<(), Box<dyn Error>> {
    use ctxfab_server::runtime::{clock::Clock, scheduler::Scheduler};
    let origin = Origin::start().await?;
    std::fs::write(origin.directory.path().join("actions-fixture"), [])?;
    std::fs::write(origin.directory.path().join("actions-completed"), [])?;
    std::fs::write(origin.directory.path().join("collect-history"), [])?;
    let directory = tempfile::tempdir()?;
    let db = directory.path().join("db.sqlite3");
    let store = Arc::new(SqliteStore::open(&db)?);
    let credentials = Arc::new(memory::Credentials::default());
    let github = origin.github(&store, Some(credentials.clone()))?;
    let signals = directory.path().join("diagnostics.sqlite3");
    let telemetry = Arc::new(EmbeddedTelemetry::open(&signals, 24, Limits::default())?);
    let app = traced_app(&store, github.clone(), telemetry.clone());
    authorize(&app, 0, FIRST, 200).await?;
    let mut config = selection(1, "fixture/alpha", 41);
    config["features"] =
        json!({"currentCode":false,"history":false,"actions":true,"codeGraph":false});
    call(&app, "PUT", "/v1/github/repositories", config, 200).await?;
    let before = origin.hits()?.lines().count();
    let admitted = store.traffic(github::BUCKET)?.attempts;
    let scheduler =
        Scheduler::default().with_github(github, Clock::system(), Some(telemetry.clone()));
    tokio::time::timeout(std::time::Duration::from_secs(6), async {
        loop {
            if store.actions_state(41)?.snapshot.is_some()
                && store.actions_due(None)?.is_some_and(|due| due.at_ms > 0)
            {
                return Ok::<_, github::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await??;
    scheduler.stop().await;
    telemetry.shutdown();
    let trace = retained_trace(&signals, "github_actions_collection")?;
    let routes = [
        "/repos/{owner}/{repo}",
        "/repos/{owner}/{repo}/branches/{branch}",
        "/repos/{owner}/{repo}/actions/workflows",
        "/repos/{owner}/{repo}/git/trees/{sha}",
        "/repos/{owner}/{repo}/git/trees/{sha}",
        "/repos/{owner}/{repo}/git/trees/{sha}",
        "/repos/{owner}/{repo}/git/blobs/{sha}",
        "/repos/{owner}/{repo}/actions/runs",
        "/repos/{owner}/{repo}/git/commits/{sha}",
        "/repos/{owner}/{repo}",
    ];
    let attempt = assert_provider_hops(&app, &trace, "github_actions_collection", &routes).await?;
    assert!(attempt["parentSpanId"].is_null());
    assert_eq!(origin.hits()?.lines().count() - before, routes.len());
    assert_eq!(
        usize::try_from(store.traffic(github::BUCKET)?.attempts - admitted)?,
        routes.len()
    );
    let due = store.actions_due(None)?.ok_or("missing due")?.at_ms;
    let prior = call(&app, "GET", ACTIONS, Value::Null, 200).await?;
    assert_eq!(prior["snapshot"]["runs"][0]["conclusion"], "success");
    drop(app);
    drop(store);
    let reopened = Arc::new(SqliteStore::open(&db)?);
    assert_eq!(
        reopened
            .actions_due(None)?
            .ok_or("missing persisted due")?
            .at_ms,
        due
    );
    let fixture_db = rusqlite::Connection::open(&db)?;
    fixture_db.execute(
        "UPDATE github_storage_budget SET limit_bytes = (SELECT sum(bytes) FROM github_storage_usage)",
        [],
    )?;
    fixture_db.execute("UPDATE github_actions_schedule SET next_at_ms = 0", [])?;
    std::fs::write(origin.directory.path().join("actions-rerun"), [])?;
    let github = origin.github(&reopened, Some(credentials))?;
    let app = super::app_with_github(&reopened, github.clone());
    let scheduler = Scheduler::default().with_github(github, Clock::system(), None);
    let denied = tokio::time::timeout(std::time::Duration::from_secs(6), async {
        loop {
            let state = call(&app, "GET", ACTIONS, Value::Null, 200).await?;
            if state["schedule"]["lastError"] == "storageLimit" {
                return Ok::<_, Box<dyn Error>>(state);
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await??;
    scheduler.stop().await;
    assert_eq!(denied["availability"], "ready");
    assert_eq!(denied["snapshot"], prior["snapshot"]);
    let retry = denied["schedule"]["nextAtMs"].as_i64().ok_or("retry")?;
    let checked = denied["schedule"]["lastCheckedAtMs"]
        .as_i64()
        .ok_or("checked")?;
    assert_eq!(retry, checked + 60_000);
    Ok(())
}
