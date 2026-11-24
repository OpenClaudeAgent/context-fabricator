use super::*;
use ctxfab_server::application::answers::EvidenceStore;

#[tokio::test]
async fn stale_graph_profile_rebuilds_from_the_unchanged_published_manifest()
-> Result<(), Box<dyn Error>> {
    use ctxfab_server::domain::github::graph::PROFILE;
    let origin = Origin::start().await?;
    std::fs::write(origin.directory.path().join("collect-graph"), [])?;
    let db = origin.directory.path().join("graph-profile-upgrade.sqlite");
    let store = Arc::new(SqliteStore::open(&db)?);
    let credentials = Arc::new(memory::Credentials::default());
    let app = origin.app(&store, Some(credentials.clone()))?;
    authorize(&app, 0, FIRST, 200).await?;
    let mut settings = selection(1, "fixture/alpha", 41);
    settings["features"] =
        json!({"currentCode":true,"codeGraph":true,"actions":false,"history":false});
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        settings.clone(),
        200,
    )
    .await?;
    let refresh = "/v1/github/repositories/41/refresh";
    let lookup =
        "/v1/github/repositories/41/graph?branch=main&root=src%2Flib.rs&symbol=graph_example";
    let revision = "a".repeat(40);
    assert_eq!(
        call(&app, "POST", refresh, Value::Null, 200).await?,
        json!({"commit": revision, "changed": true}),
    );
    let original = call(&app, "GET", lookup, Value::Null, 200).await?;
    assert_eq!(original["snapshot"]["profile"], PROFILE);
    let raw = rusqlite::Connection::open(&db)?;
    let published_id = || {
        raw.query_row(
            "SELECT id FROM source_snapshots WHERE phase='published'",
            [],
            |row| row.get::<_, i64>(0),
        )
    };
    let saved_publication = published_id()?;
    let simulate_old_profile = || {
        raw.execute(
        "UPDATE github_graph_publications SET profile='rust-tree-sitter-0.24.0-navigation-v1' WHERE repository_id=41",
        [],
    )
    };
    simulate_old_profile()?;
    drop(app);
    drop(store);
    let store = Arc::new(SqliteStore::open(&db)?);
    let app = origin.app(&store, Some(credentials.clone()))?;
    assert!(call(&app, "GET", lookup, Value::Null, 200).await?["snapshot"].is_null());
    assert_no_graph_passages(&store)?;

    let before = origin.hits()?;
    assert_eq!(
        call(&app, "POST", refresh, Value::Null, 200).await?,
        json!({"commit": revision, "changed": true}),
    );
    let hits = origin.hits()?;
    let rebuild_requests = &hits[before.len()..];
    assert!(!rebuild_requests.contains("/git/blobs/"));
    assert!(!rebuild_requests.contains("/git/trees/"));
    assert_eq!(published_id()?, saved_publication);
    assert_eq!(call(&app, "GET", lookup, Value::Null, 200).await?, original);
    assert_graph_passages(&store)?;
    assert_eq!(
        call(&app, "POST", refresh, Value::Null, 200).await?,
        json!({"commit": revision, "changed": false}),
    );
    simulate_old_profile()?;
    let github = origin.github(&store, Some(credentials.clone()))?;
    let due = github
        .next_current_code()?
        .ok_or("ordinary scheduled refresh")?;
    assert!(due.at_ms > 0);
    let github = origin.github_at(
        &store,
        Some(credentials),
        ctxfab_server::runtime::clock::Clock::anchored(due.at_ms),
    )?;
    let scheduled = github.run_current_code(&due, None).await?;
    assert_eq!(scheduled.commit, revision);
    assert!(scheduled.changed);
    assert_eq!(published_id()?, saved_publication);
    assert_graph_passages(&store)?;

    // Disabled graphs keep their historical profile on the read-only navigation endpoint.
    let app = super::app_with_github(&store, github);
    simulate_old_profile()?;
    settings["revision"] = json!(2);
    settings["features"]["codeGraph"] = json!(false);
    call(&app, "PUT", "/v1/github/repositories", settings, 200).await?;
    let before = origin.hits()?;
    let retained = call(&app, "GET", lookup, Value::Null, 200).await?;
    assert_eq!(retained["access"], "retained");
    assert_eq!(
        retained["snapshot"]["profile"],
        "rust-tree-sitter-0.24.0-navigation-v1"
    );
    assert_eq!(retained["snapshot"]["hits"], original["snapshot"]["hits"]);
    assert_no_graph_passages(&store)?;
    assert_eq!(origin.hits()?, before);
    Ok(())
}

#[tokio::test]
async fn graph_only_refresh_publishes_scoped_edges_without_general_search_rows()
-> Result<(), Box<dyn Error>> {
    let origin = Origin::start().await?;
    std::fs::write(origin.directory.path().join("collect-graph"), [])?;
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let app = origin.app(&store, Some(Arc::new(memory::Credentials::default())))?;
    authorize(&app, 0, FIRST, 200).await?;
    let mut settings = selection(1, "fixture/alpha", 41);
    settings["features"] =
        json!({"currentCode":false,"codeGraph":true,"actions":false,"history":false});
    let configured = call(
        &app,
        "PUT",
        "/v1/github/repositories",
        settings.clone(),
        200,
    )
    .await?;
    assert_eq!(
        configured["repositories"][0]["codeCollection"]["phase"],
        "queued"
    );
    let lookup =
        "/v1/github/repositories/41/graph?branch=main&root=src%2Flib.rs&symbol=graph_example";
    assert!(call(&app, "GET", lookup, Value::Null, 200).await?["snapshot"].is_null());
    call(
        &app,
        "POST",
        "/v1/github/repositories/41/refresh",
        Value::Null,
        200,
    )
    .await?;
    let state = call(&app, "GET", "/v1/github", Value::Null, 200).await?;
    let status = &state["repositories"][0]["codeCollection"];
    assert_eq!(status["phase"], "succeeded");
    assert_eq!(status["commit"], "a".repeat(40));
    assert_eq!(status["canRefresh"], true);
    assert_eq!(status["publishedFiles"], 2);
    assert!(status["pending"].is_null());
    assert!(status["publishedAtMs"].is_i64());
    assert!(status["lastCheckedAtMs"].is_i64());
    let result = call(&app, "GET", lookup, Value::Null, 200).await?;
    assert_eq!(result["access"], "available");
    let hits = result["snapshot"]["hits"].as_array().ok_or("graph hits")?;
    assert_eq!(hits.len(), 2);
    let usage = hits.iter().find(|h| h["role"] == "call").ok_or("call")?;
    assert_eq!(usage["site"]["path"], "src/lib.rs");
    assert_eq!(usage["target"]["path"], "src/target.rs");
    assert_eq!(usage["resolution"], "resolved");
    assert_eq!(
        call(
            &app,
            "GET",
            "/v1/search?query=graph_example",
            Value::Null,
            200
        )
        .await?,
        json!([])
    );
    assert!(store.pending("graph-isolation", 20)?.is_empty());
    assert_no_graph_passages(&store)?;

    std::fs::write(origin.directory.path().join("advance-code"), [])?;
    call(
        &app,
        "POST",
        "/v1/github/repositories/41/refresh",
        Value::Null,
        200,
    )
    .await?;
    let advanced = call(&app, "GET", lookup, Value::Null, 200).await?;
    assert_eq!(advanced["snapshot"]["revision"], "b".repeat(40));
    let hits = advanced["snapshot"]["hits"]
        .as_array()
        .ok_or("advanced graph hits")?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["resolution"], "unresolved");

    settings["revision"] = json!(2);
    settings["features"] =
        json!({"currentCode":true,"codeGraph":false,"actions":false,"history":false});
    call(&app, "PUT", "/v1/github/repositories", settings, 200).await?;
    assert_eq!(
        call(&app, "GET", lookup, Value::Null, 200).await?["access"],
        "retained"
    );
    call(
        &app,
        "DELETE",
        "/v1/github/repositories/41?revision=3",
        Value::Null,
        200,
    )
    .await?;
    let retained = call(&app, "GET", lookup, Value::Null, 200).await?;
    assert_eq!(retained["access"], "retained");
    assert_eq!(retained["snapshot"]["revision"], "b".repeat(40));
    Ok(())
}

#[tokio::test]
async fn adding_graph_to_published_code_reuses_one_walk_and_keeps_the_previous_code_when_disabled()
-> Result<(), Box<dyn Error>> {
    use ctxfab_server::application::github::current::ScheduleStore;
    let origin = Origin::start().await?;
    std::fs::write(origin.directory.path().join("collect-graph"), [])?;
    let db = origin.directory.path().join("graph-reopen.sqlite");
    let store = Arc::new(SqliteStore::open(&db)?);
    let credentials = Arc::new(memory::Credentials::default());
    let app = origin.app(&store, Some(credentials.clone()))?;
    authorize(&app, 0, FIRST, 200).await?;
    let mut settings = selection(1, "fixture/alpha", 41);
    settings["features"] =
        json!({"currentCode":true,"codeGraph":false,"history":false,"actions":false});
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        settings.clone(),
        200,
    )
    .await?;
    let refresh = "/v1/github/repositories/41/refresh";
    let lookup =
        "/v1/github/repositories/41/graph?branch=main&root=src%2Flib.rs&symbol=graph_example";
    call(&app, "POST", refresh, Value::Null, 200).await?;
    assert!(call(&app, "GET", lookup, Value::Null, 200).await?["snapshot"].is_null());
    settings["revision"] = json!(2);
    settings["features"]["codeGraph"] = json!(true);
    call(
        &app,
        "PUT",
        "/v1/github/repositories",
        settings.clone(),
        200,
    )
    .await?;
    let due = ScheduleStore::next(store.as_ref())?.ok_or("graph projection due")?;
    assert_eq!(due.repository_id, 41);
    assert_eq!(due.at_ms, 0);
    let traffic = origin.hits()?;
    call(&app, "POST", refresh, Value::Null, 200).await?;
    let after = origin.hits()?;
    assert!(!after[traffic.len()..].contains("/git/blobs/"));
    let graph = call(&app, "GET", lookup, Value::Null, 200).await?;
    let hits = graph["snapshot"]["hits"]
        .as_array()
        .ok_or("combined graph")?;
    assert_eq!(hits.len(), 2);
    let call_site = hits
        .iter()
        .find(|hit| hit["role"] == "call")
        .ok_or("call site")?;
    assert_eq!(call_site["target"]["path"], "src/target.rs");
    assert_graph_passages(&store)?;
    let original_blob = call_site["site"]["blob"].as_str().ok_or("caller blob")?;
    settings["revision"] = json!(3);
    settings["features"]["currentCode"] = json!(false);
    call(&app, "PUT", "/v1/github/repositories", settings, 200).await?;
    std::fs::write(origin.directory.path().join("advance-code"), [])?;
    assert_no_graph_passages(&store)?;
    std::fs::write(origin.directory.path().join("delete-graph-target"), [])?;
    call(&app, "POST", refresh, Value::Null, 200).await?;
    let search = call(
        &app,
        "GET",
        "/v1/search?query=graph_example&repositoryId=41&branch=main",
        Value::Null,
        200,
    )
    .await?;
    let retained = search.as_array().ok_or("code search")?;
    assert!(
        retained
            .first()
            .is_some_and(|hit| hit["revision"] == "a".repeat(40))
    );
    assert!(
        retained
            .iter()
            .all(|h| h["revision"] == "a".repeat(40) && h["access"] == "retained")
    );
    let requests = origin.hits()?;
    drop(app);
    drop(store);
    let reopened = Arc::new(SqliteStore::open(&db)?);
    let app = origin.app(&reopened, Some(credentials))?;
    let graph = call(&app, "GET", lookup, Value::Null, 200).await?;
    assert_eq!(graph["snapshot"]["revision"], "b".repeat(40));
    let hits = graph["snapshot"]["hits"]
        .as_array()
        .ok_or("after deletion")?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["role"], "call");
    assert_eq!(hits[0]["site"]["path"], call_site["site"]["path"]);
    assert_eq!(hits[0]["site"]["blob"], original_blob);
    assert_eq!(hits[0]["resolution"], "unresolved");
    assert!(hits[0]["target"].is_null());
    assert_eq!(origin.hits()?, requests);
    Ok(())
}

fn assert_graph_passages(store: &SqliteStore) -> Result<(), Box<dyn Error>> {
    let derived = store.graph_passages("crate::target::graph_example", None, None)?;
    let paths: Vec<_> = derived
        .iter()
        .filter_map(|excerpt| {
            excerpt
                .evidence
                .location
                .as_ref()
                .map(|loc| loc.path.as_str())
        })
        .collect();
    assert_eq!(paths, ["src/target.rs", "src/lib.rs"]);
    assert!(
        derived
            .iter()
            .all(|excerpt| excerpt.repository_id == Some(41)
                && excerpt.evidence.revision == "a".repeat(40))
    );
    assert!(
        store
            .graph_passages("crate::target::graph_example", Some("src/main.rs"), None)?
            .is_empty()
    );
    let wrong_branch = ctxfab_server::application::CodeScope {
        repository_id: 41,
        branch: "other".into(),
    };
    assert!(
        store
            .graph_passages("crate::target::graph_example", None, Some(&wrong_branch))?
            .is_empty()
    );
    Ok(())
}

fn assert_no_graph_passages(store: &SqliteStore) -> Result<(), Box<dyn Error>> {
    assert!(
        store
            .graph_passages("crate::target::graph_example", None, None)?
            .is_empty()
    );
    Ok(())
}
