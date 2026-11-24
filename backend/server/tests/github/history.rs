use super::{
    EmbeddedTelemetry, FIRST, Origin, assert_provider_hops, authorize, call, memory,
    post_with_trace, repository_trace_activity, retained_trace, selection, traced_app,
};
use axum::Router;
use ctxfab_server::adapters::outbound::github::BUCKET;
use ctxfab_server::adapters::outbound::sqlite::SqliteStore;
use ctxfab_server::adapters::outbound::telemetry::Limits;
use ctxfab_server::application::{
    github::history::{Reopen, Store, schedule::ScheduleStore},
    retrieval::VectorIndex,
    sync::TrafficLedger,
};
use ctxfab_server::domain::github;
use serde_json::{Value, json};
use std::error::Error;
use std::sync::Arc;

const HISTORY: &str = "/v1/github/repositories/41/history";

#[tokio::test]
async fn history_follows_first_parent_and_reuses_a_published_lineage() -> Result<(), Box<dyn Error>>
{
    let origin = Origin::start().await?;
    let data = tempfile::tempdir()?;
    let database = data.path().join("history.sqlite3");
    let credentials = Arc::new(memory::Credentials::default());
    let store = Arc::new(SqliteStore::open(&database)?);
    let telemetry = Arc::new(EmbeddedTelemetry::open(
        &data.path().join("diagnostics.sqlite3"),
        24,
        Limits::default(),
    )?);
    let app = traced_app(
        &store,
        origin.github(&store, Some(credentials.clone()))?,
        telemetry.clone(),
    );
    enable_history(&origin, &app).await?;

    assert_eq!(
        call(&app, "GET", HISTORY, Value::Null, 200).await?,
        json!({})
    );
    let before = origin.hits()?;
    let attempts = store.traffic(BUCKET)?.attempts;
    let (first, trace, _) = post_with_trace(&app, HISTORY, 200).await?;
    let a = "a".repeat(40);
    let root = "d".repeat(40);
    let other_parent = "e".repeat(40);
    assert_eq!(first["tip"], a);
    assert_eq!(first["boundary"], "root");
    assert_eq!(first["commits"][0]["parents"], json!([root, other_parent]));
    assert_eq!(first["commits"][1]["id"], root);
    assert_eq!(first["commits"][1]["parents"], json!([]));
    assert_history_is_metadata_only(&app, &store, &first).await?;
    let requests = origin.hits()?;
    assert_eq!(
        requests.strip_prefix(&before),
        Some(
            format!("GET /repos/fixture/alpha\nGET /repos/fixture/alpha/branches/main\n\
                GET /repos/fixture/alpha/git/commits/{a}\nGET /repos/fixture/alpha/git/commits/{root}\n\
                GET /repos/fixture/alpha\n")
                .as_str()
        )
    );
    assert_eq!(store.traffic(BUCKET)?.attempts - attempts, 5);
    telemetry.shutdown();
    let attempt = assert_provider_hops(
        &app,
        &trace,
        "github_history_collection",
        &[
            "/repos/{owner}/{repo}",
            "/repos/{owner}/{repo}/branches/{branch}",
            "/repos/{owner}/{repo}/git/commits/{sha}",
            "/repos/{owner}/{repo}/git/commits/{sha}",
            "/repos/{owner}/{repo}",
        ],
    )
    .await?;
    assert!(attempt["parentSpanId"].is_string());
    drop(app);
    drop(store);

    let store = Arc::new(SqliteStore::open(&database)?);
    let app = origin.app(&store, Some(credentials))?;
    assert_eq!(
        call(&app, "GET", HISTORY, Value::Null, 200).await?["snapshot"],
        first
    );
    assert_eq!(origin.hits()?, requests);
    assert_eq!(call(&app, "POST", HISTORY, Value::Null, 200).await?, first);
    assert!(
        repository_trace_activity(&app, "historyActivity")
            .await?
            .is_null()
    );
    let warmed = origin.hits()?;
    assert_eq!(warmed.lines().count() - requests.lines().count(), 3);
    std::fs::write(origin.directory.path().join("advance-code"), [])?;
    let next = call(&app, "POST", HISTORY, Value::Null, 200).await?;
    let b = "b".repeat(40);
    assert_eq!(next["tip"], b);
    assert_eq!(next["commits"][1], first["commits"][0]);
    assert_eq!(next["commits"][2], first["commits"][1]);
    assert_eq!(
        origin.hits()?.strip_prefix(&warmed),
        Some(
            format!(
                "GET /repos/fixture/alpha\nGET /repos/fixture/alpha/branches/main\n\
            GET /repos/fixture/alpha/git/commits/{b}\nGET /repos/fixture/alpha\n"
            )
            .as_str()
        )
    );
    Ok(())
}

async fn assert_history_is_metadata_only(
    app: &Router,
    store: &SqliteStore,
    snapshot: &Value,
) -> Result<(), Box<dyn Error>> {
    assert_eq!(snapshot["commits"][0]["summary"], "Merge tracked change");
    let search = "/v1/search?query=tracked";
    assert_eq!(call(app, "GET", search, Value::Null, 200).await?, json!([]));
    let scoped = format!("{search}&repositoryId=41&branch=main");
    assert_eq!(
        call(app, "GET", &scoped, Value::Null, 200).await?,
        json!([])
    );
    assert!(store.pending("history-isolation", 10)?.is_empty());
    Ok(())
}

async fn enable_history(origin: &Origin, app: &Router) -> Result<(), Box<dyn Error>> {
    std::fs::write(origin.directory.path().join("collect-history"), [])?;
    authorize(app, 0, FIRST, 200).await?;
    call(
        app,
        "PUT",
        "/v1/github/repositories",
        history_settings(1, true),
        200,
    )
    .await?;
    Ok(())
}

fn history_settings(revision: u32, enabled: bool) -> Value {
    let mut config = selection(revision, "fixture/alpha", 41);
    config["features"] =
        json!({"currentCode": false, "history": enabled, "actions": false, "codeGraph": false});
    config
}

#[tokio::test]
async fn incomplete_refresh_preserves_publication_and_the_next_lineage_is_bounded()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    enable_history(&origin, &app).await?;
    let prior = call(&app, "POST", HISTORY, Value::Null, 200).await?;
    let scope = store.history_scope(41)?;
    std::fs::write(origin.directory.path().join("long-history"), [])?;
    let failure = origin.directory.path().join("fail-history");
    std::fs::write(&failure, [])?;
    call(&app, "POST", HISTORY, Value::Null, 503).await?;
    assert_eq!(
        call(&app, "GET", HISTORY, Value::Null, 200).await?["snapshot"],
        prior
    );
    std::fs::remove_file(failure)?;
    let before = store.traffic(BUCKET)?.attempts;
    let next = call(&app, "POST", HISTORY, Value::Null, 200).await?;
    assert_eq!(next["tip"], format!("{:040x}", 1));
    assert_eq!(next["boundary"], "commitLimit");
    let commits = next["commits"].as_array().ok_or("history commits")?;
    assert_eq!(commits.len(), 16);
    assert_eq!(commits[15]["parents"], json!([format!("{:040x}", 17)]));
    assert_eq!(store.traffic(BUCKET)?.attempts - before, 19);
    call(
        &app,
        "PUT",
        "/v1/github/rate-limit",
        json!({"revision": 2, "requestsPerMinute": 60}),
        200,
    )
    .await?;
    assert!(store.history_scope(41)? == scope);
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        history_settings(3, false),
        200,
    )
    .await?;
    let requests = origin.hits()?;
    call(&app, "POST", HISTORY, Value::Null, 403).await?;
    assert_eq!(origin.hits()?, requests);
    let stale = serde_json::from_value::<github::history::Snapshot>(prior)?;
    assert!(matches!(
        store.publish_history(&scope, &stale, 0, 0, None),
        Err(github::Error::Conflict)
    ));
    assert_eq!(
        call(&app, "GET", HISTORY, Value::Null, 200).await?["snapshot"],
        next
    );
    Ok(())
}

#[tokio::test]
async fn changing_the_binding_while_history_is_reading_the_pin_fences_the_next_request()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    enable_history(&origin, &app).await?;
    let prior = call(&app, "POST", HISTORY, Value::Null, 200).await?;
    let release_pin = origin.directory.path().join("hold-history-branch");
    std::fs::write(origin.directory.path().join("advance-code"), [])?;
    std::fs::write(&release_pin, [])?;
    let branch_requested = origin.directory.path().join("held-history-branch");
    let refresh = {
        let app = app.clone();
        tokio::spawn(async move {
            call(&app, "POST", HISTORY, Value::Null, 409)
                .await
                .map_err(|error| error.to_string())
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !branch_requested.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    call(
        &app,
        "DELETE",
        "/v1/github/credential?revision=2",
        Value::Null,
        200,
    )
    .await?;
    let requests = origin.hits()?;
    std::fs::remove_file(release_pin)?;
    assert_eq!(refresh.await??["code"], "github_revision_conflict");
    assert_eq!(origin.hits()?, requests);
    assert_eq!(
        call(&app, "GET", HISTORY, Value::Null, 200).await?["snapshot"],
        prior
    );
    Ok(())
}

#[tokio::test]
async fn github_unauthorized_refresh_marks_the_history_credential_for_reconnect()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    enable_history(&origin, &app).await?;
    origin.expire()?;
    let reply = call(&app, "POST", HISTORY, Value::Null, 409).await?;
    assert_eq!(reply["code"], "github_reconnect");
    let config = call(&app, "GET", "/v1/github", Value::Null, 200).await?;
    assert_eq!(config["credential"], "reconnectRequired");
    let requests = origin.hits()?;
    call(&app, "POST", HISTORY, Value::Null, 403).await?;
    assert_eq!(origin.hits()?, requests);
    Ok(())
}

#[tokio::test]
async fn repository_identity_is_verified_again_before_publishing() -> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    enable_history(&origin, &app).await?;
    let prior = call(&app, "POST", HISTORY, Value::Null, 200).await?;
    std::fs::write(origin.directory.path().join("advance-code"), [])?;
    std::fs::write(origin.directory.path().join("history-repo-changed"), [])?;
    let reply = call(&app, "POST", HISTORY, Value::Null, 409).await?;
    assert_eq!(reply["code"], "github_revision_conflict");
    assert_eq!(
        call(&app, "GET", HISTORY, Value::Null, 200).await?["snapshot"],
        prior
    );
    Ok(())
}

#[tokio::test]
async fn explicit_comparison_proves_both_trees_and_first_parent() -> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    enable_history(&origin, &app).await?;
    std::fs::write(origin.directory.path().join("advance-code"), [])?;
    call(&app, "POST", HISTORY, Value::Null, 200).await?;
    let child = "b".repeat(40);
    let path = format!("{HISTORY}/comparisons/{child}");
    let before = store.traffic(BUCKET)?.attempts;
    let comparison = call(&app, "GET", &path, Value::Null, 200).await?;
    assert_eq!(comparison["parent"], "a".repeat(40));
    assert_eq!(comparison["child"], child);
    assert_eq!(comparison["baseline"], "firstParent");
    let changes = comparison["changes"].as_array().ok_or("changes")?;
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0]["kind"], "deleted");
    assert_eq!(changes[0]["before"]["path"], "src/removed.rs");
    assert_eq!(changes[1]["kind"], "renamed");
    assert_eq!(changes[1]["before"]["path"], "src/old.rs");
    assert_eq!(changes[1]["after"]["path"], "src/renamed.rs");
    assert_eq!(changes[1]["before"]["text"], changes[1]["after"]["text"]);
    assert_eq!(changes[2]["kind"], "added");
    assert_eq!(changes[2]["after"]["path"], "src/z-new.rs");
    assert!(store.traffic(BUCKET)?.attempts > before);
    let root = "d".repeat(40);
    let root_reply = call(
        &app,
        "GET",
        &format!("{HISTORY}/comparisons/{root}"),
        Value::Null,
        200,
    )
    .await?;
    assert_eq!(root_reply["baseline"], "emptyTree");
    assert!(root_reply["parent"].is_null());
    assert!(
        root_reply["changes"]
            .as_array()
            .ok_or("root changes")?
            .iter()
            .all(|c| c["kind"] == "added")
    );
    let other_parent = "e".repeat(40);
    call(
        &app,
        "GET",
        &format!("{HISTORY}/comparisons/{other_parent}"),
        Value::Null,
        400,
    )
    .await?;
    assert_eq!(
        call(&app, "GET", "/v1/search?query=tracked", Value::Null, 200).await?,
        json!([])
    );
    assert!(
        store
            .pending("history-comparison-isolation", 10)?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn modified_comparison_reopens_exact_blob_bytes_and_fences_changed_binding()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    enable_history(&origin, &app).await?;
    std::fs::write(origin.directory.path().join("advance-code"), [])?;
    std::fs::write(origin.directory.path().join("history-modified"), [])?;
    call(&app, "POST", HISTORY, Value::Null, 200).await?;
    let path = format!("{HISTORY}/comparisons/{}", "b".repeat(40));
    let comparison = call(&app, "GET", &path, Value::Null, 200).await?;
    let changes = comparison["changes"].as_array().ok_or("changes")?;
    let modified = changes
        .iter()
        .find(|c| c["kind"] == "modified")
        .ok_or("modified")?;
    assert_eq!(
        modified["before"]["text"],
        "line one\r\nrepeat\r\nrepeat\r\n"
    );
    assert_eq!(
        modified["after"]["text"],
        "line one\r\nchanged\r\nrepeat\r\n"
    );
    let hold = origin.directory.path().join("hold-history-blob");
    std::fs::write(&hold, [])?;
    let pending = {
        let app = app.clone();
        let path = path.clone();
        tokio::spawn(async move {
            call(&app, "GET", &path, Value::Null, 409)
                .await
                .map_err(|e| e.to_string())
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !origin.directory.path().join("held-history-blob").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    call(
        &app,
        "DELETE",
        "/v1/github/credential?revision=2",
        Value::Null,
        200,
    )
    .await?;
    let hits = origin.hits()?;
    std::fs::remove_file(hold)?;
    assert_eq!(pending.await??["code"], "github_revision_conflict");
    assert_eq!(origin.hits()?, hits);
    Ok(())
}

#[tokio::test]
async fn comparison_exposes_partial_content_and_rejects_truncated_trees()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    enable_history(&origin, &app).await?;
    std::fs::write(origin.directory.path().join("advance-code"), [])?;
    std::fs::write(origin.directory.path().join("history-partial"), [])?;
    call(&app, "POST", HISTORY, Value::Null, 200).await?;
    let path = format!("{HISTORY}/comparisons/{}", "b".repeat(40));
    let reply = call(&app, "GET", &path, Value::Null, 200).await?;
    assert_eq!(reply["coverage"], "partialContent");
    let changes = reply["changes"].as_array().ok_or("changes")?;
    let large = changes
        .iter()
        .find(|c| c["after"]["path"] == "src/large.bin")
        .ok_or("large")?;
    assert_eq!(large["after"]["coverage"], "tooLarge");
    assert!(large["after"]["text"].is_null());
    let link = changes
        .iter()
        .find(|c| c["after"]["path"] == "src/link")
        .ok_or("link")?;
    assert_eq!(link["after"]["coverage"], "unsupportedMode");
    let hits = origin.hits()?;
    assert!(!hits.contains(&format!("/git/blobs/{}", "9".repeat(40))));
    std::fs::write(origin.directory.path().join("history-truncated"), [])?;
    call(&app, "GET", &path, Value::Null, 422).await?;
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn materialized_evidence_is_immutable_scoped_and_durable() -> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let data = tempfile::tempdir()?;
    let database = data.path().join("evidence.sqlite3");
    let credentials = Arc::new(memory::Credentials::default());
    let store = Arc::new(SqliteStore::open(&database)?);
    let app = origin.app(&store, Some(credentials.clone()))?;
    enable_history(&origin, &app).await?;
    std::fs::write(origin.directory.path().join("advance-code"), [])?;
    std::fs::write(origin.directory.path().join("history-modified"), [])?;
    let snapshot = call(&app, "POST", HISTORY, Value::Null, 200).await?;
    let scope = store.history_scope(41)?;
    let child = "b".repeat(40);
    let path = format!("{HISTORY}/comparisons/{child}");
    let evidence_path = format!("{HISTORY}/evidence/{child}");
    assert!(
        call(&app, "GET", &evidence_path, Value::Null, 200)
            .await?
            .is_null()
    );
    let saved = call(&app, "POST", &path, Value::Null, 200).await?;
    assert_eq!(saved["projectionVersion"], 1);
    assert!(
        saved["changes"]
            .as_array()
            .ok_or("changes")?
            .iter()
            .all(|c| c["change"]["before"]["text"].is_null()
                && c["change"]["after"]["text"].is_null())
    );
    let modified = saved["changes"]
        .as_array()
        .ok_or("changes")?
        .iter()
        .find(|c| c["change"]["kind"] == "modified")
        .ok_or("modified")?;
    let hunk = &modified["projection"]["hunks"][0];
    assert_eq!(hunk["before"]["text"], "repeat\r\n");
    assert_eq!(hunk["after"]["text"], "changed\r\n");
    let index = saved["changes"]
        .as_array()
        .ok_or("changes")?
        .iter()
        .position(|c| c["change"]["kind"] == "modified")
        .ok_or("modified index")?;
    assert!(
        store
            .reopen_hunk(&Reopen {
                repository_id: 41,
                branch: "other",
                child: &child,
                parent: Some(&"a".repeat(40)),
                change: index,
                hunk: 0
            })?
            .is_none()
    );
    assert!(
        store
            .reopen_hunk(&Reopen {
                repository_id: 41,
                branch: "main",
                child: &child,
                parent: Some(&"e".repeat(40)),
                change: index,
                hunk: 0
            })?
            .is_none()
    );
    let reopened = store
        .reopen_hunk(&Reopen {
            repository_id: 41,
            branch: "main",
            child: &child,
            parent: Some(&"a".repeat(40)),
            change: index,
            hunk: 0,
        })?
        .ok_or("reopen")?;
    assert!(!reopened.retained);
    assert_eq!(
        reopened.hunk.before.as_ref().ok_or("before")?.text,
        "repeat\r\n"
    );
    assert_eq!(
        reopened.hunk.after.as_ref().ok_or("after")?.text,
        "changed\r\n"
    );
    let blob = hunk["before"]["blob"].as_str().ok_or("blob")?;
    let connection = rusqlite::Connection::open(&database)?;
    connection.execute(
        "UPDATE github_history_blobs SET bytes = ?1 WHERE blob = ?2",
        rusqlite::params![b"corrupt".as_slice(), blob],
    )?;
    assert!(
        store
            .reopen_hunk(&Reopen {
                repository_id: 41,
                branch: "main",
                child: &child,
                parent: Some(&"a".repeat(40)),
                change: index,
                hunk: 0
            })
            .is_err()
    );
    connection.execute(
        "UPDATE github_history_blobs SET bytes = ?1 WHERE blob = ?2",
        rusqlite::params![b"line one\r\nrepeat\r\nrepeat\r\n".as_slice(), blob],
    )?;
    drop(connection);
    assert_eq!(call(&app, "POST", &path, Value::Null, 200).await?, saved);
    assert_eq!(
        call(&app, "GET", &evidence_path, Value::Null, 200).await?,
        saved
    );
    std::fs::remove_file(origin.directory.path().join("history-modified"))?;
    call(&app, "POST", &path, Value::Null, 409).await?;
    assert_eq!(
        call(&app, "GET", &evidence_path, Value::Null, 200).await?,
        saved
    );
    let document = serde_json::from_value(saved.clone())?;
    let fingerprint = serde_json::to_string(&serde_json::from_value::<github::history::Snapshot>(
        snapshot,
    )?)?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        history_settings(2, false),
        200,
    )
    .await?;
    assert!(matches!(
        store.publish_comparison(
            &scope,
            &fingerprint,
            &child,
            Some(&"a".repeat(40)),
            &document,
            &[]
        ),
        Err(github::Error::Conflict)
    ));
    assert!(
        call(&app, "GET", &evidence_path, Value::Null, 200)
            .await?
            .is_null()
    );
    drop(app);
    drop(store);
    let store = Arc::new(SqliteStore::open(&database)?);
    let app = origin.app(&store, Some(credentials))?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        history_settings(3, true),
        200,
    )
    .await?;
    assert_eq!(
        call(&app, "GET", &evidence_path, Value::Null, 200).await?,
        saved
    );
    std::fs::write(origin.directory.path().join("long-history"), [])?;
    call(&app, "POST", HISTORY, Value::Null, 200).await?;
    assert!(
        call(&app, "GET", &evidence_path, Value::Null, 200)
            .await?
            .is_null()
    );
    Ok(())
}

#[tokio::test]
async fn automatic_lineage_obeys_cadence_reuses_commits_and_fences_old_jobs()
-> Result<(), Box<dyn Error>> {
    use ctxfab_server::runtime::{clock::Clock, scheduler::Scheduler};
    use std::time::Duration;
    let origin = Origin::start().await?;
    let db = origin.directory.path().join("schedule.sqlite3");
    std::fs::write(origin.directory.path().join("collect-history"), [])?;
    let store = Arc::new(SqliteStore::open(&db)?);
    let credentials = Arc::new(memory::Credentials::default());
    let github = origin.github(&store, Some(credentials.clone()))?;
    let app = super::app_with_github(&store, github.clone());
    authorize(&app, 0, FIRST, 200).await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        history_settings(1, true),
        200,
    )
    .await?;
    let due = github.next_history()?.ok_or("history due")?;
    assert_eq!(due.at_ms, 0);
    let scheduler = Scheduler::default().with_github(github.clone(), Clock::system(), None);
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            if github.next_history()?.is_some_and(|due| due.at_ms > 0) {
                break Ok::<_, github::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    scheduler.stop().await;
    let query = call(&app, "GET", HISTORY, Value::Null, 200).await?;
    let checked = query["schedule"]["lastCheckedAtMs"]
        .as_i64()
        .ok_or("checked")?;
    let next = github.next_history()?.ok_or("published cadence")?;
    assert_eq!(next.at_ms - checked, next.interval_minutes * 60_000);
    assert_eq!(query["snapshot"]["tip"], "a".repeat(40));
    let traffic = origin.hits()?;
    assert!(matches!(
        github.run_history(&due, None).await,
        Err(github::Error::Conflict)
    ));
    assert_eq!(origin.hits()?, traffic);
    assert!(!traffic.contains("/git/blobs/"));
    std::fs::write(origin.directory.path().join("advance-code"), [])?;
    let later = Clock::anchored(next.at_ms);
    let reopened = Arc::new(SqliteStore::open(&db)?);
    let later_github = origin.github_at(&reopened, Some(credentials), later)?;
    assert_eq!(
        later_github
            .next_history()?
            .ok_or("persisted schedule")?
            .at_ms,
        next.at_ms
    );
    later_github.run_history(&next, None).await?;
    let second = origin.hits()?;
    assert_eq!(
        second[traffic.len()..]
            .lines()
            .filter(|l| l.contains("/git/commits/"))
            .count(),
        1
    );
    let stale_snapshot =
        serde_json::from_value::<github::history::Snapshot>(query["snapshot"].clone())?;
    let scope = reopened.history_scope(41)?;
    assert!(matches!(
        reopened.publish_history(&scope, &stale_snapshot, next.at_ms, later.now(), None),
        Err(github::Error::Conflict)
    ));
    assert_eq!(
        call(&app, "GET", HISTORY, Value::Null, 200).await?["snapshot"]["tip"],
        "b".repeat(40)
    );
    let before_pause = later_github.next_history()?.ok_or("next history")?;
    call(
        &app,
        "DELETE",
        "/v1/github/repositories/41/data",
        Value::Null,
        200,
    )
    .await?;
    assert!(later_github.next_history()?.is_none());
    assert!(later_github.run_history(&before_pause, None).await.is_err());
    assert_eq!(origin.hits()?, second);
    Ok(())
}

#[tokio::test]
async fn history_schedule_retries_failures_and_rejects_a_reverified_generation()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    std::fs::write(origin.directory.path().join("collect-history"), [])?;
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let github = origin.github(&store, Some(Arc::new(memory::Credentials::default())))?;
    let app = super::app_with_github(&store, github.clone());
    authorize(&app, 0, FIRST, 200).await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        history_settings(1, true),
        200,
    )
    .await?;
    let initial = github.next_history()?.ok_or("initial history")?;
    let deny = origin.directory.path().join("deny-contents");
    std::fs::write(&deny, [])?;
    assert!(github.run_history(&initial, None).await.is_err());
    let state = call(&app, "GET", HISTORY, Value::Null, 200).await?;
    assert!(state["snapshot"].is_null());
    assert_eq!(state["schedule"]["lastError"], "access");
    let checked = state["schedule"]["lastCheckedAtMs"]
        .as_i64()
        .ok_or("failure time")?;
    assert_eq!(
        github.next_history()?.ok_or("retry")?.at_ms,
        checked + 60_000
    );
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        history_settings(2, true),
        200,
    )
    .await?;
    assert!(github.next_history()?.is_none());
    std::fs::remove_file(deny)?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        history_settings(3, true),
        200,
    )
    .await?;
    let fresh = github.next_history()?.ok_or("fresh generation")?;
    assert_ne!(fresh.generation, initial.generation);
    assert_eq!(fresh.at_ms, 0);
    let traffic = origin.hits()?;
    assert!(matches!(
        github.run_history(&initial, None).await,
        Err(github::Error::Conflict)
    ));
    assert_eq!(origin.hits()?, traffic);
    github.run_history(&fresh, None).await?;
    assert_eq!(
        call(&app, "GET", HISTORY, Value::Null, 200).await?["snapshot"]["tip"],
        "a".repeat(40)
    );
    Ok(())
}

#[tokio::test]
async fn manual_history_refresh_joins_the_scheduled_observation_in_flight()
-> Result<(), Box<dyn Error>> {
    use std::time::Duration;
    let origin = Origin::start().await?;
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let github = origin.github(&store, Some(Arc::new(memory::Credentials::default())))?;
    let diagnostics = origin.directory.path().join("diagnostics.sqlite3");
    let telemetry = Arc::new(EmbeddedTelemetry::open(
        &diagnostics,
        24,
        Limits::default(),
    )?);
    let app = traced_app(&store, github.clone(), telemetry.clone());
    enable_history(&origin, &app).await?;
    let due = github.next_history()?.ok_or("initial history job")?;
    let release_pin = origin.directory.path().join("hold-history-branch");
    let branch_requested = origin.directory.path().join("held-history-branch");
    std::fs::write(&release_pin, [])?;
    let baseline = origin.hits()?;
    let explicit = async {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !branch_requested.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        let manual = post_with_trace(&app, HISTORY, 200);
        tokio::pin!(manual);
        tokio::select! {
            biased;
            _ = &mut manual => return Err("manual refresh finished before the held pin".into()),
            () = tokio::task::yield_now() => {}
        }
        std::fs::remove_file(release_pin)?;
        manual.await
    };
    let (scheduled, explicit) = tokio::join!(github.run_history(&due, Some(&*telemetry)), explicit);
    scheduled?;
    let (snapshot, joined_trace, work_trace) = explicit?;
    assert_eq!(snapshot["tip"], "a".repeat(40));
    let activity = repository_trace_activity(&app, "historyActivity").await?;
    telemetry.shutdown();
    let producer = retained_trace(&diagnostics, "github_history_collection")?;
    assert_eq!(work_trace.as_deref(), Some(producer.as_str()));
    assert_ne!(joined_trace, producer);
    assert_eq!(activity["publishedSnapshot"]["traceId"], producer);
    assert_eq!(activity["latestAttempt"], activity["publishedSnapshot"]);
    let work = call(
        &app,
        "GET",
        &format!("/v1/telemetry/traces/{producer}"),
        Value::Null,
        200,
    )
    .await?;
    let span = work["spans"]
        .as_array()
        .ok_or("work trace spans")?
        .iter()
        .find(|span| span["name"] == "github_history_collection")
        .ok_or("lineage span")?;
    assert_eq!(activity["publishedSnapshot"]["spanId"], span["spanId"]);
    let request = call(
        &app,
        "GET",
        &format!("/v1/telemetry/traces/{joined_trace}"),
        Value::Null,
        200,
    )
    .await?;
    let request_spans = request["spans"].as_array().ok_or("joined request spans")?;
    assert_eq!(request_spans.len(), 1);
    assert_eq!(request_spans[0]["workTraceId"], producer);
    let traffic = origin.hits()?;
    assert_eq!(
        traffic[baseline.len()..]
            .lines()
            .filter(|p| p.ends_with("/branches/main"))
            .count(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn oversized_comparison_preserves_lineage_and_the_next_tick_prepares_another_child()
-> Result<(), Box<dyn Error>> {
    use ctxfab_server::runtime::clock::Clock;

    let origin = Origin::start().await?;
    for flag in ["collect-history", "advance-code", "history-truncated"] {
        std::fs::write(origin.directory.path().join(flag), [])?;
    }
    let db = origin.directory.path().join("preparation.sqlite3");
    let store = Arc::new(SqliteStore::open(&db)?);
    let credentials = Arc::new(memory::Credentials::default());
    let mut github = origin.github(&store, Some(credentials.clone()))?;
    let signals = origin.directory.path().join("diagnostics.sqlite3");
    let telemetry = Arc::new(EmbeddedTelemetry::open(&signals, 24, Limits::default())?);
    let app = traced_app(&store, github.clone(), telemetry.clone());
    authorize(&app, 0, FIRST, 200).await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        history_settings(1, true),
        200,
    )
    .await?;
    let tip = "b".repeat(40);
    let parent = "a".repeat(40);
    let mut due = github.next_history()?.ok_or("initial due")?;
    let before = origin.hits()?.lines().count();
    github.run_history(&due, Some(&*telemetry)).await?;
    let state = store.history_state(41)?;
    assert_eq!(state.snapshot.ok_or("published lineage")?.tip, tip);
    assert!(
        state
            .schedule
            .ok_or("successful cadence")?
            .last_error
            .is_none()
    );
    assert!(store.read_comparison_evidence(41, &tip)?.is_none());
    assert!(store.read_comparison_evidence(41, &parent)?.is_none());
    telemetry.shutdown();
    let dispatched = origin.hits()?.lines().count() - before;
    assert_scheduled_preparation_trace(&app, &signals, dispatched).await?;
    std::fs::remove_file(origin.directory.path().join("history-truncated"))?;
    for tick in 0..2 {
        due = github.next_history()?.ok_or("next due")?;
        github = origin.github_at(
            &Arc::new(SqliteStore::open(&db)?),
            Some(credentials.clone()),
            Clock::anchored(due.at_ms),
        )?;
        let before = origin.hits()?;
        github.run_history(&due, None).await?;
        let hits = origin.hits()?;
        let requests = hits[before.len()..].lines().count();
        // Unchanged lineage requires two repository checks and a branch pin.
        assert!(requests <= 3 + 32);
        if tick == 1 {
            assert_eq!(requests, 3);
        }
        assert!(store.read_comparison_evidence(41, &tip)?.is_none());
        assert!(store.read_comparison_evidence(41, &parent)?.is_some());
    }
    Ok(())
}

async fn assert_scheduled_preparation_trace(
    app: &Router,
    signals: &std::path::Path,
    dispatched: usize,
) -> Result<(), Box<dyn Error>> {
    let trace = retained_trace(signals, "github_history_job")?;
    let page = call(
        app,
        "GET",
        &format!("/v1/telemetry/traces/{trace}"),
        Value::Null,
        200,
    )
    .await?;
    let spans = page["spans"].as_array().ok_or("trace spans")?;
    let named = |name| spans.iter().find(|span| span["name"] == name);
    let job = named("github_history_job").ok_or("scheduled job")?;
    let collect = named("github_history_collection").ok_or("lineage collection")?;
    let prepare = named("github_history_preparation").ok_or("evidence preparation")?;
    assert_eq!(collect["parentSpanId"], job["spanId"]);
    assert_eq!(prepare["parentSpanId"], job["spanId"]);
    assert_eq!(job["level"], "ok");
    assert_eq!(collect["level"], "ok");
    assert_eq!(prepare["level"], "error");
    assert_eq!(prepare["problemCode"], "collectionLimit");
    let hops: Vec<_> = spans
        .iter()
        .filter(|span| span["name"] == "github_provider_request")
        .collect();
    assert_eq!(hops.len(), dispatched);
    assert!(hops.iter().all(|hop| {
        hop["parentSpanId"] == collect["spanId"] || hop["parentSpanId"] == prepare["spanId"]
    }));
    assert!(hops.iter().any(|hop| {
        hop["parentSpanId"] == prepare["spanId"]
            && hop["httpRoute"] == "/repos/{owner}/{repo}/git/trees/{sha}"
            && hop["httpStatus"] == 200
            && hop["level"] == "ok"
    }));
    Ok(())
}

#[tokio::test]
async fn scheduled_comparison_publishes_two_changed_crates() -> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    for flag in ["collect-history", "advance-code", "history-two-crates"] {
        std::fs::write(origin.directory.path().join(flag), [])?;
    }
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let credentials = Arc::new(memory::Credentials::default());
    let github = origin.github(&store, Some(credentials))?;
    let app = super::app_with_github(&store, github.clone());
    authorize(&app, 0, FIRST, 200).await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        history_settings(1, true),
        200,
    )
    .await?;

    let before = origin.hits()?;
    github
        .run_history(&github.next_history()?.ok_or("due")?, None)
        .await?;
    let child = "b".repeat(40);
    let parent = "a".repeat(40);
    let state = store
        .history_state(41)?
        .snapshot
        .ok_or("published lineage")?;
    assert_eq!(state.tip, child);
    let evidence = call(
        &app,
        "GET",
        &format!("{HISTORY}/evidence/{child}"),
        Value::Null,
        200,
    )
    .await?;
    assert_eq!(evidence["child"], child);
    assert_eq!(evidence["parent"], parent);
    let changes = evidence["changes"].as_array().ok_or("published changes")?;
    assert_eq!(changes.len(), 2);
    for (change, path) in changes
        .iter()
        .zip(["crates/fast-id/src/lib.rs", "crates/fs/src/which.rs"])
    {
        let change = &change["change"];
        assert_eq!(change["kind"], "modified");
        assert_eq!(change["before"]["path"], path);
        assert_eq!(change["after"]["path"], path);
    }
    let hits = origin.hits()?;
    let traffic = hits[before.len()..].lines().collect::<Vec<_>>();
    let first_tree = traffic
        .iter()
        .position(|path| path.contains("/git/trees/"))
        .ok_or("first tree")?;
    let preparation = &traffic[first_tree - 1..];
    assert_eq!(preparation.len(), 18);
    assert_eq!(
        preparation
            .iter()
            .filter(|path| path.contains("/git/trees/"))
            .count(),
        12
    );
    assert_eq!(
        preparation
            .iter()
            .filter(|path| path.contains("/git/blobs/"))
            .count(),
        4
    );
    assert_eq!(preparation.first(), preparation.last());
    Ok(())
}

#[tokio::test]
async fn background_tree_requests_stop_at_the_tick_budget_and_allow_other_children()
-> Result<(), Box<dyn Error>> {
    use ctxfab_server::runtime::clock::Clock;
    let origin = Origin::start().await?;
    for flag in ["collect-history", "advance-code", "history-wide"] {
        std::fs::write(origin.directory.path().join(flag), [])?;
    }
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let credentials = Arc::new(memory::Credentials::default());
    let github = origin.github(&store, Some(credentials.clone()))?;
    let app = super::app_with_github(&store, github.clone());
    authorize(&app, 0, FIRST, 200).await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        history_settings(1, true),
        200,
    )
    .await?;
    let before = origin.hits()?;
    github
        .run_history(&github.next_history()?.ok_or("due")?, None)
        .await?;
    let hits = origin.hits()?;
    let traffic: Vec<_> = hits[before.len()..].lines().collect();
    let first_tree = traffic
        .iter()
        .position(|p| p.contains("/git/trees/"))
        .ok_or("tree read")?;
    // The repository check just before the first tree is the first preparation request.
    assert_eq!(traffic.len() - first_tree + 1, 32);
    let tip = "b".repeat(40);
    let scope = store.history_scope(41)?;
    assert_eq!(store.preparation_attempts(&scope)?.get(&tip), Some(&None));
    assert!(store.read_comparison_evidence(41, &tip)?.is_none());
    let due = github.next_history()?.ok_or("next tick")?;
    let later = origin.github_at(&store, Some(credentials), Clock::anchored(due.at_ms))?;
    later.run_history(&due, None).await?;
    assert!(
        store
            .read_comparison_evidence(41, &"a".repeat(40))?
            .is_some()
    );
    Ok(())
}
