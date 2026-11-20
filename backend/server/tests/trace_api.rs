use std::error::Error;

use ctxfab_server::adapters::outbound::telemetry::{EmbeddedTelemetry, Limits};
use ctxfab_server::application::telemetry::{Event, Observer, Stage, Telemetry};

#[test]
fn abandoned_span_has_no_success_outcome() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let telemetry = EmbeddedTelemetry::open(
        &directory.path().join("trace.sqlite3"),
        24,
        Limits::default(),
    )?;

    let parent = telemetry.start(Stage::ApiAnswer, None);
    let context = parent.link();
    telemetry
        .start(Stage::AnswerRetrieval, Some(context))
        .finish(true);
    drop(telemetry.start(Stage::AnswerGeneration, Some(context)));
    parent.finish(false);
    telemetry.shutdown();

    let snapshot = telemetry.snapshot();
    let mut outcomes = snapshot
        .traces
        .iter()
        .map(|span| (span.name.as_str(), span.level.as_deref()))
        .collect::<Vec<_>>();
    outcomes.sort_unstable();
    assert_eq!(
        outcomes,
        [
            ("answer_generation", None),
            ("answer_retrieval", Some("ok")),
            ("api_answer", Some("error"))
        ],
    );
    assert!(
        !snapshot
            .logs
            .iter()
            .any(|log| log.name == "answer_generation")
    );
    Ok(())
}

use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use ctxfab_server::adapters::{inbound::http, outbound::sqlite::SqliteStore};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn get(
    app: &Router,
    path: &str,
    authenticated: bool,
) -> Result<(u16, Value), Box<dyn Error>> {
    let response = request(app, path, authenticated).await?;
    let status = response.status().as_u16();
    assert_eq!(response.headers()["content-type"], "application/json");
    let body = to_bytes(response.into_body(), 65_536).await?;
    Ok((status, serde_json::from_slice(&body)?))
}

async fn request(
    app: &Router,
    path: &str,
    authenticated: bool,
) -> Result<axum::response::Response, Box<dyn Error>> {
    let mut request = Request::builder().uri(path);
    if authenticated {
        request = request.header("authorization", "Bearer trace-fixture");
    }
    Ok(app.clone().oneshot(request.body(Body::empty())?).await?)
}

#[tokio::test]
async fn search_trace_explains_operations_and_branch_without_query_text()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("signals.sqlite3");
    let telemetry = Arc::new(EmbeddedTelemetry::open(&path, 24, Limits::default())?);
    let app = http::router_with_services(
        Arc::new(SqliteStore::open(Path::new(":memory:"))?),
        "trace-fixture",
        http::Services {
            telemetry: Some(telemetry.clone()),
            ..http::Services::default()
        },
    );
    let response = request(&app, "/v1/search?query=private-sentinel", true).await?;
    assert_eq!(response.status(), 200);
    let trace_id = response.headers()["x-context-trace-id"].to_str()?;
    let rejected = request(&app, "/v1/search", true).await?;
    assert_eq!(rejected.status(), 400);
    let rejected_id = rejected.headers()["x-context-trace-id"].to_str()?;
    telemetry.shutdown();

    let (status, page) = get(&app, &format!("/v1/telemetry/traces/{trace_id}"), true).await?;
    assert_eq!(status, 200);
    assert!(!page.to_string().contains("private-sentinel"));
    let root = &page["spans"][0];
    let child = &page["spans"][1];
    assert_eq!(root["name"], "api_query");
    assert_eq!(root["displayName"], "Search request");
    assert_eq!(root["operation"], "GET /v1/search");
    assert_eq!(child["operation"], "search.bm25");
    assert_eq!(child["parentSpanId"], root["spanId"]);
    let root_start = root["startedAtUnixMicros"]
        .as_i64()
        .ok_or("missing root start")?;
    let child_start = child["startedAtUnixMicros"]
        .as_i64()
        .ok_or("missing child start")?;
    assert!(root_start > 0 && child_start >= root_start);
    let event = &root["events"][0];
    assert_eq!(event["name"], "BM25 lookup selected");
    let offset = event["offsetMicros"]
        .as_i64()
        .ok_or("missing event offset")?;
    let duration = root["value"].as_i64().ok_or("missing duration")?;
    assert!((0..=duration).contains(&offset));
    assert!(page["logs"][0].get("operation").is_none());
    assert_eq!(page["logs"][0]["spanId"], root["spanId"]);

    let (status, page) = get(&app, &format!("/v1/telemetry/traces/{rejected_id}"), true).await?;
    assert_eq!(status, 200);
    let rejected_span = &page["spans"][0];
    assert_eq!(rejected_span["level"], "error");
    assert_eq!(rejected_span["events"][0]["name"], "Invalid search request");
    assert_eq!(page["spans"].as_array().map(Vec::len), Some(1));
    Ok(())
}

#[tokio::test]
async fn detail_preserves_legacy_spans_and_bounds_events_after_reopen() -> Result<(), Box<dyn Error>>
{
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("signals.sqlite3");
    let database = rusqlite::Connection::open(&path)?;
    database.execute_batch(
        "CREATE TABLE signals (
            id INTEGER PRIMARY KEY, signal TEXT NOT NULL, timestamp INTEGER NOT NULL,
            name TEXT NOT NULL, value INTEGER, level TEXT,
            trace_id TEXT, span_id TEXT, parent_span_id TEXT
         ) STRICT;
         INSERT INTO signals (signal, timestamp, name, value, level, trace_id, span_id)
         VALUES ('trace', unixepoch(), 'api_query', 170, 'ok',
                 '1234567890abcdef1234567890abcdef', '1234567890abcdef');",
    )?;
    drop(database);

    let telemetry = EmbeddedTelemetry::open(&path, 24, Limits::default())?;
    let mut span = telemetry.start(Stage::ApiQuery, None);
    let recorded_id = uuid::Uuid::from_bytes(span.link().trace)
        .simple()
        .to_string();
    for _ in 0..12 {
        span.event(Event::Bm25Selected);
    }
    span.finish(true);
    telemetry.shutdown();
    drop(telemetry);

    let app = http::router_with_services(
        Arc::new(SqliteStore::open(Path::new(":memory:"))?),
        "trace-fixture",
        http::Services {
            telemetry: Some(Arc::new(EmbeddedTelemetry::open(
                &path,
                24,
                Limits::default(),
            )?)),
            ..http::Services::default()
        },
    );
    let (_, legacy) = get(
        &app,
        "/v1/telemetry/traces/1234567890abcdef1234567890abcdef",
        true,
    )
    .await?;
    let span = &legacy["spans"][0];
    assert_eq!(span["value"], 170);
    assert_eq!(span["level"], "ok");
    assert_eq!(span["operation"], "GET /v1/search");
    assert!(span.get("startedAtUnixMicros").is_none() && span.get("events").is_none());
    let (_, recorded) = get(&app, &format!("/v1/telemetry/traces/{recorded_id}"), true).await?;
    let span = &recorded["spans"][0];
    let events = span["events"]
        .as_array()
        .ok_or("missing persisted events")?;
    assert_eq!(events.len(), 8);
    assert!(
        events
            .iter()
            .all(|event| event["name"] == "BM25 lookup selected")
    );
    let offsets: Vec<_> = events
        .iter()
        .filter_map(|event| event["offsetMicros"].as_i64())
        .collect();
    assert_eq!(offsets.len(), events.len());
    assert!(offsets.is_sorted());
    Ok(())
}

fn records(page: &Value) -> impl Iterator<Item = &Value> {
    ["spans", "logs"]
        .into_iter()
        .flat_map(|kind| page[kind].as_array().into_iter().flatten())
}

#[tokio::test]
async fn trace_lookup_pages_retained_records_and_excludes_other_traces()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("signals.sqlite3");
    let telemetry = Arc::new(EmbeddedTelemetry::open(&path, 1, Limits::default())?);
    let parent = telemetry.start(Stage::ApiAnswer, None);
    let link = parent.link();
    let id = uuid::Uuid::from_bytes(link.trace).simple().to_string();
    for _ in 0..32 {
        telemetry
            .start(Stage::AnswerRetrieval, Some(link))
            .finish(true);
    }
    let unfinished = telemetry.start(Stage::AnswerGeneration, Some(link));
    drop(unfinished);
    parent.finish(true);
    for _ in 0..65 {
        telemetry.start(Stage::ApiQuery, None).finish(true);
    }
    telemetry.shutdown();
    assert!(
        telemetry
            .snapshot()
            .traces
            .iter()
            .all(|span| span.trace_id.as_deref() != Some(&id)),
        "the target must be older than the recent 60 spans"
    );

    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let app = http::router_with_services(
        store,
        "trace-fixture",
        http::Services {
            telemetry: Some(telemetry),
            ..http::Services::default()
        },
    );
    let url = format!("/v1/telemetry/traces/{id}");
    let (status, first) = get(&app, &url, true).await?;
    assert_eq!(status, 200);
    assert_eq!(records(&first).count(), 60);
    let cursor = first["nextBeforeId"]
        .as_i64()
        .ok_or("missing trace cursor")?;
    let (status, older) = get(&app, &format!("{url}?beforeId={cursor}"), true).await?;
    assert_eq!(status, 200);
    assert!(older.get("nextBeforeId").is_none());
    assert_eq!(records(&older).count(), 7);
    assert!(records(&older).all(|record| record["id"].as_i64().is_some_and(|id| id < cursor)));
    let all: Vec<_> = records(&first).chain(records(&older)).collect();
    assert!(all.iter().all(|record| record["traceId"] == id));
    assert_eq!(
        all.iter()
            .map(|record| &record["id"])
            .collect::<std::collections::HashSet<_>>()
            .len(),
        67
    );
    assert!(
        records(&first)
            .any(|record| record["name"] == "answer_generation" && record.get("level").is_none())
    );
    let root = all
        .iter()
        .find(|record| record["name"] == "api_answer" && record.get("value").is_some())
        .ok_or("missing root span")?;
    assert!(
        all.iter()
            .any(|record| record["parentSpanId"] == root["spanId"])
    );
    assert!(all.iter().any(|record| record["name"] == "api_answer"
        && record["level"] == "info"
        && record["spanId"] == root["spanId"]));

    rusqlite::Connection::open(path)?.execute(
        "UPDATE signals SET timestamp = 0 WHERE trace_id = ?1",
        [&id],
    )?;
    assert_eq!(
        get(&app, &url, true).await?,
        (200, json!({"spans": [], "logs": []}))
    );
    Ok(())
}

#[tokio::test]
async fn trace_lookup_requires_session_and_valid_parameters() -> Result<(), Box<dyn Error>> {
    let app = http::router(
        Arc::new(SqliteStore::open(Path::new(":memory:"))?),
        "trace-fixture",
    );
    let url = "/v1/telemetry/traces/0123456789abcdef0123456789abcdef";
    let (status, body) = get(&app, url, false).await?;
    assert_eq!(status, 401);
    assert_eq!(body["code"], "unauthorized");
    for malformed in ["/v1/telemetry/traces/invalid", "/v1/telemetry/traces/%FF"] {
        let (status, body) = get(&app, malformed, true).await?;
        assert_eq!(status, 400);
        assert_eq!(body["code"], "invalid_trace_id");
        assert_eq!(body["field"], "traceId");
    }
    let (status, body) = get(&app, &format!("{url}?beforeId=0"), true).await?;
    assert_eq!(status, 400);
    assert_eq!(body["code"], "invalid_trace_cursor");
    assert_eq!(body["field"], "beforeId");
    let (status, body) = get(&app, url, true).await?;
    assert_eq!(status, 503);
    assert_eq!(body["code"], "storage_unavailable");
    Ok(())
}
