use std::error::Error;
use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, header};
use ctxfab_inference::{DIMENSIONS, Embedder, Embedding, EmbeddingError, Input, PROFILE};
use ctxfab_server::adapters::{
    inbound::http,
    outbound::{
        sqlite::SqliteStore,
        telemetry::{EmbeddedTelemetry, Limits},
    },
};
use ctxfab_server::application::{
    retrieval::Retrieval,
    telemetry::{Sample, Telemetry},
};
use ctxfab_server::domain::corpus::{Document, DocumentKind};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{add, fixture_service};

const TOKEN: &str = "retrieval-fixture";
const DOCUMENTS: [Document<'static>; 2] = [
    Document {
        source: "src/sync.rs",
        revision: "main-9",
        kind: DocumentKind::Current {
            tracked_ref: "refs/heads/main",
        },
        title: "Burstguard",
        content: "The burstguard paces requests.",
    },
    Document {
        source: "src/sync.rs",
        revision: "release-8",
        kind: DocumentKind::History,
        title: "Cold archive",
        content: "Archive cadence was recorded.",
    },
];

fn problem(code: &str, message: &str, field: Option<&str>) -> Value {
    match field {
        Some(field) => json!({"code": code, "message": message, "field": field}),
        None => json!({"code": code, "message": message}),
    }
}

fn invalid_limit() -> Value {
    problem(
        "invalid_rebuild_limit",
        "Supply a rebuild batch limit from 1 to 32.",
        Some("limit"),
    )
}

fn progress(state: &str, indexed: usize, total: usize) -> Value {
    json!({"state": state, "profile": PROFILE, "dimensions": DIMENSIONS, "indexed": indexed, "total": total})
}

fn current() -> Value {
    json!({"source":"src/sync.rs","revision":"main-9","kind":"current",
        "trackedRef":"refs/heads/main","title":"Burstguard","access":"available"})
}

fn historical() -> Value {
    json!({"source":"src/sync.rs","revision":"release-8","kind":"history",
        "title":"Cold archive","access":"available"})
}

async fn expect(
    app: &Router,
    request: Request<Body>,
    status: u16,
    payload: Value,
) -> Result<Option<String>, Box<dyn Error>> {
    let operation = format!("{} {}", request.method(), request.uri());
    let response = app.clone().oneshot(request).await?;
    assert_eq!(response.status(), status, "{operation}");
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/json",
        "{operation}"
    );
    let trace = response
        .headers()
        .get("x-context-trace-id")
        .map(|header| header.to_str().map(str::to_owned))
        .transpose()?;
    let actual: Value = serde_json::from_slice(&to_bytes(response.into_body(), 16_384).await?)?;
    assert_eq!(actual, payload, "{operation}");
    Ok(trace)
}

async fn json_request(
    app: &Router,
    method: Method,
    path: &str,
    status: u16,
    payload: Value,
) -> Result<Option<String>, Box<dyn Error>> {
    expect(
        app,
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .body(Body::empty())?,
        status,
        payload,
    )
    .await
}

#[tokio::test]
async fn lexical_api_preserves_provenance_and_structured_rejections_without_embeddings()
-> Result<(), Box<dyn Error>> {
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    add(&store, "search", &DOCUMENTS)?;
    let app = http::router(store, TOKEN);
    let unauthorized = problem(
        "unauthorized",
        "A local session credential is required.",
        None,
    );
    for (method, path) in [
        (Method::GET, "/v1/search"),
        (Method::GET, "/v1/retrieval"),
        (Method::POST, "/v1/retrieval/rebuild?limit=0"),
    ] {
        let missing = Request::builder()
            .method(method.clone())
            .uri(path)
            .body(Body::empty())?;
        expect(&app, missing, 401, unauthorized.clone()).await?;
        let browser = Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::ORIGIN, "https://fixture.invalid")
            .body(Body::empty())?;
        expect(&app, browser, 401, unauthorized.clone()).await?;
    }

    for path in [
        "/v1/search?query=burstguard",
        "/v1/search?query=burstguard&mode=bm25",
    ] {
        json_request(&app, Method::GET, path, 200, json!([current()])).await?;
    }
    json_request(&app, Method::GET, "/v1/search?query=absent", 200, json!([])).await?;
    json_request(
        &app,
        Method::GET,
        "/v1/retrieval",
        200,
        json!({"state":"notConfigured","dimensions":0,"indexed":0,"total":0}),
    )
    .await?;

    let invalid = problem(
        "invalid_query",
        "Search text must contain 1 to 512 characters.",
        Some("query"),
    );
    for path in [
        "/v1/search",
        "/v1/search?query=%25",
        "/v1/search?query=burstguard&mode=unknown",
        "/v1/search?query=burstguard&query=archive",
    ] {
        json_request(&app, Method::GET, path, 400, invalid.clone()).await?;
    }
    let path = format!("/v1/search?query={}", "%C3%A9".repeat(512));
    json_request(&app, Method::GET, &path, 200, json!([])).await?;
    json_request(&app, Method::GET, &format!("{path}%C3%A9"), 400, invalid).await?;

    let unconfigured = problem(
        "embedding_not_configured",
        "The local embedding runtime is not configured.",
        None,
    );
    for mode in ["semantic", "hybrid"] {
        json_request(
            &app,
            Method::GET,
            &format!("/v1/search?query=burstguard&mode={mode}"),
            503,
            unconfigured.clone(),
        )
        .await?;
    }
    json_request(
        &app,
        Method::POST,
        "/v1/retrieval/rebuild",
        503,
        unconfigured,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn vector_api_reports_progress_ranking_and_correlated_traces() -> Result<(), Box<dyn Error>> {
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    add(&store, "search", &DOCUMENTS)?;
    let service = Arc::new(fixture_service(&store, PROFILE));
    let path = tempfile::tempdir()?;
    let telemetry = Arc::new(EmbeddedTelemetry::open(
        &path.path().join("signals.sqlite3"),
        24,
        Limits::default(),
    )?);
    let app = http::router_with_services(
        store,
        TOKEN,
        http::Services {
            retrieval: Some(service),
            telemetry: Some(telemetry.clone()),
            ..http::Services::default()
        },
    );

    json_request(
        &app,
        Method::GET,
        "/v1/retrieval",
        200,
        progress("incomplete", 0, 2),
    )
    .await?;
    let unavailable = json_request(
        &app,
        Method::GET,
        "/v1/search?query=archive&mode=semantic",
        503,
        problem(
            "embedding_index_incomplete",
            "The local embedding index is incomplete for the active profile.",
            None,
        ),
    )
    .await?;
    json_request(
        &app,
        Method::POST,
        "/v1/retrieval/rebuild?limit=1",
        200,
        progress("incomplete", 1, 2),
    )
    .await?;
    json_request(
        &app,
        Method::POST,
        "/v1/retrieval/rebuild?limit=32",
        200,
        progress("ready", 2, 2),
    )
    .await?;
    let lexical = json_request(
        &app,
        Method::GET,
        "/v1/search?query=burstguard",
        200,
        json!([current()]),
    )
    .await?;
    json_request(
        &app,
        Method::GET,
        "/v1/search?query=archive&mode=semantic",
        200,
        json!([historical(), current()]),
    )
    .await?;
    let hybrid = json_request(
        &app,
        Method::GET,
        "/v1/search?query=archive&mode=hybrid",
        200,
        json!([historical(), current()]),
    )
    .await?;

    telemetry.shutdown();
    let traces = telemetry.snapshot().traces;
    for (id, child_name, level) in [
        (unavailable, "vector_retrieval", "error"),
        (lexical, "lexical_lookup", "ok"),
        (hybrid, "vector_retrieval", "ok"),
    ] {
        assert_search_trace(&traces, id.as_deref(), child_name, level)?;
    }
    Ok(())
}

fn assert_search_trace(
    traces: &[Sample],
    id: Option<&str>,
    child_name: &str,
    level: &str,
) -> Result<(), Box<dyn Error>> {
    let id = id.ok_or("missing search response trace header")?;
    assert_eq!(id.len(), 32);
    assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let parent = traces
        .iter()
        .find(|span| span.name == "api_query" && span.trace_id.as_deref() == Some(id))
        .ok_or("missing API span for trace header")?;
    let child = traces
        .iter()
        .find(|span| span.name == child_name && span.trace_id == parent.trace_id)
        .ok_or("missing retrieval child span")?;
    assert_eq!(parent.level.as_deref(), Some(level));
    assert_eq!(child.level.as_deref(), Some(level));
    assert!(parent.span_id.is_some());
    assert_eq!(child.parent_span_id, parent.span_id);
    Ok(())
}

#[tokio::test]
async fn rebuild_enforces_batch_limits_and_keeps_rejected_requests_read_only()
-> Result<(), Box<dyn Error>> {
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let sources: Vec<_> = (0..33).map(|n| format!("fixture/{n}.txt")).collect();
    let documents: Vec<_> = sources
        .iter()
        .map(|source| Document {
            source,
            ..DOCUMENTS[0]
        })
        .collect();
    add(&store, "limits", &documents)?;
    let retrieval = Arc::new(fixture_service(&store, PROFILE));
    let app = http::router_with_services(
        store,
        TOKEN,
        http::Services {
            retrieval: Some(retrieval),
            ..http::Services::default()
        },
    );

    for query in ["limit=0", "limit=33", "limit=invalid", "limit=1&limit=2"] {
        json_request(
            &app,
            Method::POST,
            &format!("/v1/retrieval/rebuild?{query}"),
            400,
            invalid_limit(),
        )
        .await?;
    }
    json_request(
        &app,
        Method::GET,
        "/v1/retrieval",
        200,
        progress("incomplete", 0, 33),
    )
    .await?;
    json_request(
        &app,
        Method::POST,
        "/v1/retrieval/rebuild",
        200,
        progress("incomplete", 16, 33),
    )
    .await?;
    json_request(
        &app,
        Method::POST,
        "/v1/retrieval/rebuild?limit=1",
        200,
        progress("incomplete", 17, 33),
    )
    .await?;
    json_request(
        &app,
        Method::POST,
        "/v1/retrieval/rebuild?limit=32",
        200,
        progress("ready", 33, 33),
    )
    .await?;
    Ok(())
}

struct FailingEmbedder(EmbeddingError);

impl Embedder for FailingEmbedder {
    fn embed(&self, _: Input<'_>) -> Result<Embedding, EmbeddingError> {
        Err(self.0)
    }
}

#[tokio::test]
async fn rebuild_failures_have_specific_codes_and_leave_progress_pending()
-> Result<(), Box<dyn Error>> {
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    add(&store, "failures", &DOCUMENTS)?;
    for (error, code, message) in [
        (
            EmbeddingError::Runtime,
            "embedding_runtime_unavailable",
            "The local embedding runtime is unavailable.",
        ),
        (
            EmbeddingError::Busy,
            "embedding_busy",
            "The local embedding runtime or rebuild is busy.",
        ),
        (
            EmbeddingError::Vector,
            "embedding_vector_invalid",
            "The embedding vector is incompatible with the active profile.",
        ),
        (
            EmbeddingError::PassageTooLarge,
            "embedding_passage_too_large",
            "A source passage exceeds the embedding profile input budget.",
        ),
    ] {
        let retrieval = Arc::new(Retrieval::new(
            store.clone(),
            store.clone(),
            Arc::new(FailingEmbedder(error)),
            PROFILE,
        ));
        let app = http::router_with_services(
            store.clone(),
            TOKEN,
            http::Services {
                retrieval: Some(retrieval),
                ..http::Services::default()
            },
        );
        json_request(
            &app,
            Method::POST,
            "/v1/retrieval/rebuild",
            503,
            problem(code, message, None),
        )
        .await?;
        json_request(
            &app,
            Method::GET,
            "/v1/retrieval",
            200,
            progress("incomplete", 0, 2),
        )
        .await?;
    }
    Ok(())
}

#[tokio::test]
async fn index_controls_are_authenticated_and_leave_bm25_available() -> Result<(), Box<dyn Error>> {
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    add(&store, "search", &DOCUMENTS)?;
    let service = Arc::new(fixture_service(&store, PROFILE));
    service.rebuild(32)?;
    let app = http::router_with_services(
        store,
        TOKEN,
        http::Services {
            retrieval: Some(service),
            ..http::Services::default()
        },
    );
    expect(
        &app,
        Request::builder()
            .uri("/v1/retrieval/index")
            .body(Body::empty())?,
        401,
        problem(
            "unauthorized",
            "A local session credential is required.",
            None,
        ),
    )
    .await?;
    json_request(
        &app,
        Method::GET,
        "/v1/retrieval/index",
        200,
        json!({"profile":PROFILE,"enabled":true,"epoch":0}),
    )
    .await?;
    let update = Request::builder()
        .method(Method::PUT)
        .uri("/v1/retrieval/index")
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"enabled":false}"#))?;
    expect(
        &app,
        update,
        200,
        json!({"profile":PROFILE,"enabled":false,"epoch":1}),
    )
    .await?;
    json_request(
        &app,
        Method::GET,
        "/v1/retrieval",
        200,
        progress("disabled", 2, 2),
    )
    .await?;
    json_request(
        &app,
        Method::GET,
        "/v1/search?query=burstguard&mode=semantic",
        503,
        problem(
            "embedding_index_disabled",
            "The local embedding index is disabled.",
            None,
        ),
    )
    .await?;
    json_request(
        &app,
        Method::GET,
        "/v1/search?query=burstguard",
        200,
        json!([current()]),
    )
    .await?;
    json_request(
        &app,
        Method::DELETE,
        "/v1/retrieval/index",
        200,
        json!({"profile":PROFILE,"enabled":false,"epoch":2,"removedVectors":2,"removedBytes":8192}),
    )
    .await?;
    json_request(
        &app,
        Method::GET,
        "/v1/retrieval",
        200,
        progress("disabled", 0, 2),
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn index_controls_reject_untyped_input_and_notify_on_committed_state()
-> Result<(), Box<dyn Error>> {
    let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
    let changes = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = changes.clone();
    let service = fixture_service(&store, PROFILE).with_state_listener(move |enabled| {
        if let Ok(mut events) = observed.lock() {
            events.push(enabled);
        }
    });
    let app = http::router_with_services(
        store,
        TOKEN,
        http::Services {
            retrieval: Some(Arc::new(service)),
            ..http::Services::default()
        },
    );

    let update = |payload: Value| {
        Request::builder()
            .method(Method::PUT)
            .uri("/v1/retrieval/index")
            .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(payload.to_string()))
    };
    expect(
        &app,
        update(json!({"enabled":"false"}))?,
        400,
        problem(
            "invalid_index_state",
            "Supply an enabled boolean.",
            Some("enabled"),
        ),
    )
    .await?;
    assert!(
        changes
            .lock()
            .map_err(|_| "fixture listener poisoned")?
            .is_empty()
    );
    for (enabled, epoch) in [(false, 1), (false, 1), (true, 2)] {
        expect(
            &app,
            update(json!({"enabled":enabled}))?,
            200,
            json!({"profile":PROFILE,"enabled":enabled,"epoch":epoch}),
        )
        .await?;
    }
    json_request(
        &app,
        Method::DELETE,
        "/v1/retrieval/index",
        200,
        json!({"profile":PROFILE,"enabled":false,"epoch":3,"removedVectors":0,"removedBytes":0}),
    )
    .await?;
    assert_eq!(
        *changes.lock().map_err(|_| "fixture listener poisoned")?,
        [false, false, true, false]
    );
    Ok(())
}
