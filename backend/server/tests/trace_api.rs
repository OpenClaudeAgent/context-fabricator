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
    send(app, "GET", path, authenticated).await
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    authenticated: bool,
) -> Result<axum::response::Response, Box<dyn Error>> {
    let mut request = Request::builder()
        .uri(path)
        .method(method)
        .header("user-agent", "PrivateAgentName");
    if authenticated {
        request = request.header("authorization", "Bearer trace-fixture");
    }
    Ok(app.clone().oneshot(request.body(Body::empty())?).await?)
}

#[tokio::test]
async fn ingress_traces_routing_and_auth_outcomes_without_recording_telemetry_reads()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let telemetry = Arc::new(EmbeddedTelemetry::open(
        &directory.path().join("trace.sqlite3"),
        24,
        Limits::default(),
    )?);
    let app = http::router_with_services(
        Arc::new(SqliteStore::open(Path::new(":memory:"))?),
        "trace-fixture",
        http::Services {
            telemetry: Some(telemetry.clone()),
            ..http::Services::default()
        },
    );
    let cases = ingress_cases();
    let mut ids = Vec::new();
    for (method, path, auth, status, ..) in cases {
        let response = send(&app, method, path, auth).await?;
        assert_eq!(response.status().as_u16(), status);
        ids.push(
            response
                .headers()
                .get("x-context-trace-id")
                .ok_or("expected trace header")?
                .to_str()?
                .to_owned(),
        );
    }
    for method in ["GET", "HEAD"] {
        for path in [
            "/v1/telemetry",
            "/v1/telemetry/logs",
            "/v1/telemetry/answer-usage",
        ] {
            assert!(
                send(&app, method, path, true)
                    .await?
                    .headers()
                    .get("x-context-trace-id")
                    .is_none()
            );
        }
    }
    let (status, usage) = get(&app, "/v1/telemetry/answer-usage", true).await?;
    assert_eq!(status, 200);
    assert_eq!(usage, json!({"groups": []}));
    telemetry.shutdown();
    let (_, summary) = get(&app, "/v1/telemetry", true).await?;
    let recent = summary["traces"]
        .as_array()
        .ok_or("expected recent traces")?;
    assert_eq!(recent.len(), cases.len());
    let (_, logs) = get(&app, "/v1/telemetry/logs", true).await?;
    let records = logs["records"].as_array().ok_or("log records")?;
    assert_eq!(records.len(), cases.len());
    for ((method, _, _, status, route, problem), id) in cases.into_iter().zip(ids) {
        let path = format!("/v1/telemetry/traces/{id}");
        assert!(
            request(&app, &path, true)
                .await?
                .headers()
                .get("x-context-trace-id")
                .is_none()
        );
        let (_, trace) = get(&app, &path, true).await?;
        assert_eq!(trace["spans"].as_array().map(Vec::len), Some(1));
        let root = &trace["spans"][0];
        let method = if method == "PRIVATE-VERB" {
            "OTHER"
        } else {
            method
        };
        assert_eq!(root["httpMethod"], method);
        assert_eq!(root["httpStatus"], status);
        assert_eq!(root["clientKind"], "other");
        assert_eq!(root["httpRoute"].as_str(), route);
        assert_eq!(root["problemCode"].as_str(), problem);
        let preview = recent
            .iter()
            .find(|item| item["traceId"].as_str() == Some(id.as_str()))
            .ok_or("expected correlated trace summary")?;
        for field in [
            "displayName",
            "operation",
            "httpMethod",
            "httpRoute",
            "httpStatus",
            "problemCode",
        ] {
            assert_eq!(preview[field], root[field], "{field} for {id}");
        }
        assert_request_log(records, &id, root)?;
        assert!(root.get("parentSpanId").is_none());
        assert!(!trace.to_string().contains("private-"));
        assert!(!trace.to_string().contains("PRIVATE-VERB"));
        assert!(!trace.to_string().contains("PrivateAgentName"));
    }
    assert!(!summary.to_string().contains("private-"));
    assert_log_search_uses_recorded_operation(&app).await
}

async fn assert_log_search_uses_recorded_operation(app: &Router) -> Result<(), Box<dyn Error>> {
    for (query, status) in [
        ("query=GET%20%2Fv1%2Fstatus&level=info", 200),
        ("query=route_not_found&level=error", 404),
    ] {
        let (_, page) = get(app, &format!("/v1/telemetry/logs?{query}"), true).await?;
        let records = page["records"].as_array().ok_or("filtered records")?;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["httpStatus"], status);
    }
    Ok(())
}

fn assert_request_log(records: &[Value], id: &str, root: &Value) -> Result<(), Box<dyn Error>> {
    let log = records
        .iter()
        .find(|item| item["traceId"] == id)
        .ok_or("request log")?;
    for field in [
        "spanId",
        "displayName",
        "httpMethod",
        "httpRoute",
        "httpStatus",
        "problemCode",
        "value",
    ] {
        assert_eq!(log[field], root[field], "{field} for {id}");
    }
    assert_eq!(
        log["level"],
        if root["level"] == "ok" {
            "info"
        } else {
            "error"
        }
    );
    let encoded = log.to_string();
    for private in [
        "private",
        "PRIVATE-VERB",
        "PrivateAgentName",
        "trace-fixture",
    ] {
        assert!(!encoded.contains(private), "{id} retained private data");
    }
    Ok(())
}

type IngressCase = (
    &'static str,
    &'static str,
    bool,
    u16,
    Option<&'static str>,
    Option<&'static str>,
);

#[tokio::test]
async fn usage_groups_retained_answer_requests_and_measures_only_published_latency()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("signals.sqlite3");
    let limits = Limits {
        max_rows_per_signal: 1000,
        ..Limits::default()
    };
    let telemetry = Arc::new(EmbeddedTelemetry::open(&path, 24, limits)?);
    let db = rusqlite::Connection::open(&path)?;
    let now: i64 = db.query_row("SELECT unixepoch()", [], |row| row.get(0))?;
    let model = ctxfab_inference::generation::GENERATION_PROFILE;
    for (offset, duration) in (1..=20).map(|n| (n, n * 10)) {
        insert_usage_sample(
            &db,
            (
                offset,
                now - 3,
                duration,
                Some(200),
                Some("Cited answer published"),
                None,
            ),
            model,
        )?;
    }
    for (offset, status, event) in [
        (
            21,
            Some(200),
            Some("Answer withheld for insufficient evidence"),
        ),
        (22, Some(409), Some("Answer request cancelled")),
        (23, Some(503), None),
        (24, None, None),
        (25, Some(401), None),
    ] {
        insert_usage_sample(&db, (offset, now - 1, 9_999, status, event, None), model)?;
    }
    insert_usage_sample(
        &db,
        (
            26,
            now - 90_000,
            999,
            Some(200),
            Some("Cited answer published"),
            None,
        ),
        model,
    )?;
    insert_usage_sample(
        &db,
        (
            27,
            now,
            999,
            Some(200),
            Some("Cited answer published"),
            Some("fedcba9876543210"),
        ),
        model,
    )?;
    db.execute(
        "INSERT INTO signals(signal, timestamp, name, value, trace_id)
         VALUES('trace', ?1, 'api_answer', 999, 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb')",
        [now - 2],
    )?;
    db.execute(
        "WITH RECURSIVE attempts(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM attempts WHERE n<505)
         INSERT INTO signals(signal, timestamp, name, span_timing)
         SELECT 'trace', ?1, 'api_answer', '{\"httpStatus\":401}' FROM attempts",
        [now],
    )?;
    let app = http::router_with_services(
        Arc::new(SqliteStore::open(Path::new(":memory:"))?),
        "trace-fixture",
        http::Services {
            telemetry: Some(telemetry.clone()),
            ..http::Services::default()
        },
    );
    let path = "/v1/telemetry/answer-usage";
    assert_eq!(get(&app, path, false).await?.0, 401);
    let (status, report) = get(&app, path, true).await?;
    assert_eq!(status, 200);
    assert_eq!(
        report,
        json!({
            "firstAt": now - 3, "lastAt": now - 1,
            "groups": [
                {"client": "desktop", "modelProfile": model, "published": 20, "abstained": 1,
                 "cancelled": 1, "failed": 1, "unknown": 1, "p50Micros": 100, "p95Micros": 190,
                 "latestTraceId": format!("{:032x}", 24)},
                {"client": "other", "published": 0, "abstained": 0, "cancelled": 0,
                 "failed": 0, "unknown": 1, "latestTraceId": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
            ]
        })
    );
    telemetry.shutdown();
    Ok(())
}

type UsageFixture<'a> = (u32, i64, u32, Option<u16>, Option<&'a str>, Option<&'a str>);

fn insert_usage_sample(
    db: &rusqlite::Connection,
    (id, time, elapsed, status, event, parent): UsageFixture<'_>,
    model: &str,
) -> rusqlite::Result<()> {
    let timing = json!({
        "startedAtUnixMicros": time * 1_000_000,
        "httpStatus": status,
        "clientKind": "desktop",
        "modelProfile": model,
        "events": event.into_iter().map(|name| json!({"name":name,"offsetMicros":0})).collect::<Vec<_>>()
    });
    db.execute(
        "INSERT INTO signals(signal, timestamp, name, value, trace_id, span_id, parent_span_id, span_timing)
         VALUES('trace', ?1, 'api_answer', ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![time, elapsed, format!("{id:032x}"), format!("{id:016x}"), parent, timing.to_string()],
    )?;
    Ok(())
}

fn ingress_cases() -> [IngressCase; 7] {
    [
        (
            "GET",
            "/v1/status?secret-query=private",
            true,
            200,
            Some("/v1/status"),
            None,
        ),
        (
            "GET",
            "/v1/search?private-search=secret",
            false,
            401,
            Some("/v1/search"),
            Some("unauthorized"),
        ),
        (
            "PATCH",
            "/v1/search",
            true,
            405,
            Some("/v1/search"),
            Some("method_not_allowed"),
        ),
        (
            "GET",
            "/v1/answer-history/private-segment",
            true,
            400,
            Some("/v1/answer-history/{id}"),
            Some("invalid_answer_history_input"),
        ),
        (
            "PRIVATE-VERB",
            "/unmatched-private-segment",
            false,
            404,
            None,
            Some("route_not_found"),
        ),
        ("HEAD", "/v1/status", true, 200, Some("/v1/status"), None),
        (
            "PATCH",
            "/v1/search",
            false,
            401,
            Some("/v1/search"),
            Some("unauthorized"),
        ),
    ]
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
    assert!(root_start + offset <= child_start);
    assert!(page["logs"][0].get("operation").is_none());
    assert_eq!(page["logs"][0]["spanId"], root["spanId"]);

    let (status, page) = get(&app, &format!("/v1/telemetry/traces/{rejected_id}"), true).await?;
    assert_eq!(status, 200);
    let rejected_span = &page["spans"][0];
    assert_eq!(rejected_span["level"], "error");
    assert_eq!(rejected_span["httpStatus"], 400);
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
