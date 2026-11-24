use super::{FIRST, Origin, authorize, call, memory, selection};
use ctxfab_server::adapters::outbound::sqlite::SqliteStore;
use ctxfab_server::application::retrieval::{RetrievalError, VectorIndex};
use serde_json::{Value, json};
use std::{error::Error, sync::Arc};

#[tokio::test]
async fn logical_limit_blocks_growth_and_preserves_previous_publications()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("budget.sqlite3");
    let store = Arc::new(SqliteStore::open(&path)?);
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
    let budget = call(&app, "GET", "/v1/github/storage", Value::Null, 200).await?;
    assert_eq!(budget["limitGb"], 10);
    call(
        &app,
        "PUT",
        "/v1/github/storage",
        json!({"revision": 0, "limitGb": 0}),
        400,
    )
    .await?;
    call(
        &app,
        "PUT",
        "/v1/github/storage",
        json!({"revision": 0, "limitGb": 1}),
        200,
    )
    .await?;
    call(
        &app,
        "PUT",
        "/v1/github/storage",
        json!({"revision": 0, "limitGb": 2}),
        409,
    )
    .await?;
    let refresh = "/v1/github/repositories/41/refresh";
    let first = call(&app, "POST", refresh, Value::Null, 200).await?;
    assert_eq!(first["commit"], "a".repeat(40));
    let before = call(&app, "GET", "/v1/github/storage", Value::Null, 200).await?["bytes"]
        .as_i64()
        .ok_or("bytes")?;
    assert!(before > 0);
    let db = rusqlite::Connection::open(&path)?;
    db.execute(
        "INSERT INTO collections(name) VALUES ('github/current/collision')",
        [],
    )?;
    db.execute("INSERT INTO source_blobs(collection, blob, content) VALUES ('github/current/collision', 'synthetic', 'unrelated')", [])?;
    let uncharged: i64 = db.query_row("SELECT sum(bytes) FROM github_storage_ledger", [], |r| {
        r.get(0)
    })?;
    assert_eq!(uncharged, before);
    assert!(db.execute("INSERT INTO github_code_scopes(collection, repository_id, branch) VALUES ('github/current/collision', 99, 'main')", []).is_err());
    assert_ledger(&db)?;
    db.execute(
        "UPDATE github_storage_budget SET limit_bytes = ?1",
        [before],
    )?;
    let passage = store
        .pending("budget-fixture", 1)?
        .into_iter()
        .next()
        .ok_or("passage")?;
    let mut values = vec![0.0; ctxfab_inference::DIMENSIONS];
    values[0] = 1.0;
    let embedding = ctxfab_inference::Embedding::new(values)?;
    assert_eq!(
        store.save_vector("budget-fixture", &passage, &embedding),
        Err(RetrievalError::StorageLimit)
    );
    std::fs::write(origin.directory.path().join("advance-code"), [])?;
    let blocked = call(&app, "POST", refresh, Value::Null, 422).await?;
    assert_eq!(blocked["code"], "github_storage_limit");
    let state = call(&app, "GET", "/v1/github", Value::Null, 200).await?;
    assert_eq!(
        state["repositories"][0]["codeCollection"]["commit"],
        "a".repeat(40)
    );
    let after: i64 = db.query_row("SELECT sum(bytes) FROM github_storage_usage", [], |r| {
        r.get(0)
    })?;
    assert_eq!(before, after);
    db.execute(
        "UPDATE github_storage_budget SET limit_bytes = 1000000000",
        [],
    )?;
    assert_eq!(
        call(&app, "POST", refresh, Value::Null, 200).await?["commit"],
        "b".repeat(40)
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn purge_is_scoped_pauses_background_work_and_explicit_refresh_resumes()
-> Result<(), Box<dyn Error>> {
    use ctxfab_server::application::github::current::Store as CurrentStore;
    let origin = Origin::start().await?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("purge.sqlite3");
    let store = Arc::new(SqliteStore::open(&path)?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    let mut alpha = selection(1, "fixture/alpha", 41);
    alpha["features"]["actions"] = json!(false);
    call(&app, "PUT", "/v1/github/repositories", alpha, 200).await?;
    call(
        &app,
        "POST",
        "/v1/github/repositories/41/refresh",
        Value::Null,
        200,
    )
    .await?;
    std::fs::write(origin.directory.path().join("collect-history"), [])?;
    call(
        &app,
        "POST",
        "/v1/github/repositories/41/history",
        Value::Null,
        200,
    )
    .await?;
    call(
        &app,
        "POST",
        &format!(
            "/v1/github/repositories/41/history/comparisons/{}",
            "a".repeat(40)
        ),
        Value::Null,
        200,
    )
    .await?;
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        selection(2, "fixture/beta", 42),
        200,
    )
    .await?;
    call(
        &app,
        "POST",
        "/v1/github/repositories/42/refresh",
        Value::Null,
        200,
    )
    .await?;
    let passage = store
        .pending("purge-fixture", 1)?
        .into_iter()
        .next()
        .ok_or("passage")?;
    let mut values = vec![0.0; ctxfab_inference::DIMENSIONS];
    values[0] = 1.0;
    assert!(store.save_vector(
        "purge-fixture",
        &passage,
        &ctxfab_inference::Embedding::new(values)?
    )?);
    let scope = store.current_scope(41)?;
    let db = rusqlite::Connection::open(&path)?;
    db.execute(
        "INSERT INTO github_history_blobs(blob,bytes) VALUES ('shared-fixture', X'010203')",
        [],
    )?;
    for id in [41, 42] {
        db.execute("INSERT INTO github_history_evidence(repository_id,branch,child,parent,projection_version,document) VALUES (?1,'retained','fixture','',1,'{}')", [id])?;
        db.execute("INSERT INTO github_history_evidence_blobs(repository_id,branch,child,parent,projection_version,blob) VALUES (?1,'retained','fixture','',1,'shared-fixture')", [id])?;
    }
    let before: i64 = db.query_row("SELECT sum(bytes) FROM github_storage_usage", [], |r| {
        r.get(0)
    })?;
    assert!(before > 0);
    call(
        &app,
        "DELETE",
        "/v1/github/repositories/41/data",
        Value::Null,
        200,
    )
    .await?;
    assert!(store.check_scope(&scope).is_err());
    let after: i64 = db.query_row("SELECT sum(bytes) FROM github_storage_usage", [], |r| {
        r.get(0)
    })?;
    assert!(after > 0 && after < before);
    assert!(store.current_scope(42).is_ok());
    let shared: i64 = db.query_row(
        "SELECT count(*) FROM github_history_blobs WHERE blob='shared-fixture'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(shared, 1);
    let paused: i64 = db.query_row(
        "SELECT count(*) FROM github_data_paused WHERE repository_id=41",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(paused, 1);
    assert_eq!(
        call(
            &app,
            "GET",
            "/v1/github/repositories/41/data",
            Value::Null,
            200
        )
        .await?["paused"],
        true
    );
    let config = call(&app, "GET", "/v1/github", Value::Null, 200).await?;
    assert!(
        config["repositories"]
            .as_array()
            .ok_or("repositories")?
            .iter()
            .any(|r| r["repository"]["id"] == 41)
    );
    assert!(store.current_scope(41).is_err());
    call(
        &app,
        "POST",
        "/v1/github/repositories/41/actions",
        Value::Null,
        403,
    )
    .await?;
    assert!(store.current_scope(41).is_err());
    call(
        &app,
        "POST",
        "/v1/github/repositories/41/refresh",
        Value::Null,
        200,
    )
    .await?;
    assert!(store.current_scope(41).is_ok());
    assert_eq!(
        call(
            &app,
            "GET",
            "/v1/github/repositories/41/data",
            Value::Null,
            200
        )
        .await?["paused"],
        false
    );
    assert_ledger(&db)?;
    Ok(())
}

#[tokio::test]
async fn limit_rolls_back_history_and_actions_without_replacing_last_good()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    std::fs::write(origin.directory.path().join("collect-history"), [])?;
    std::fs::write(origin.directory.path().join("actions-fixture"), [])?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("history-actions.sqlite3");
    let store = Arc::new(SqliteStore::open(&path)?);
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
    let history = "/v1/github/repositories/41/history";
    let actions = "/v1/github/repositories/41/actions";
    let prior_history = call(&app, "POST", history, Value::Null, 200).await?;
    let prior_actions = call(&app, "POST", actions, Value::Null, 200).await?;
    let db = rusqlite::Connection::open(&path)?;
    let used: i64 = db.query_row("SELECT sum(bytes) FROM github_storage_usage", [], |r| {
        r.get(0)
    })?;
    db.execute("UPDATE github_storage_budget SET limit_bytes = ?1", [used])?;
    std::fs::write(origin.directory.path().join("advance-code"), [])?;
    let rejected = call(&app, "POST", history, Value::Null, 422).await?;
    assert_eq!(rejected["code"], "github_storage_limit");
    assert_eq!(
        call(&app, "GET", history, Value::Null, 200).await?["snapshot"],
        prior_history
    );
    std::fs::write(origin.directory.path().join("actions-rerun"), [])?;
    let rejected = call(&app, "POST", actions, Value::Null, 422).await?;
    assert_eq!(rejected["code"], "github_storage_limit");
    assert_eq!(
        call(&app, "GET", actions, Value::Null, 200).await?["snapshot"],
        prior_actions
    );
    let after: i64 = db.query_row("SELECT sum(bytes) FROM github_storage_usage", [], |r| {
        r.get(0)
    })?;
    assert_eq!(after, used);
    assert_ledger(&db)?;
    Ok(())
}

fn assert_ledger(db: &rusqlite::Connection) -> Result<(), Box<dyn Error>> {
    let mismatch: i64 = db.query_row("SELECT count(*) FROM github_storage_usage u JOIN github_storage_ledger l USING(family) WHERE u.bytes <> l.bytes", [], |r| r.get(0))?;
    assert_eq!(mismatch, 0);
    Ok(())
}
