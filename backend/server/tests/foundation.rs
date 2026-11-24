use std::error::Error;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use ctxfab_server::adapters::{inbound::http, outbound::sqlite::SqliteStore};
use ctxfab_server::application::{
    AnswerLanguage, Failure, SearchQuery, Settings,
    ports::{CorpusStore, Repository},
};
use ctxfab_server::domain::corpus::{Batch, Commit, Document, DocumentKind};
use ctxfab_server::runtime::session::Session;
use serde_json::{Value, json};
use tower::ServiceExt;

#[test]
fn settings_survive_restart_and_reject_stale_edits() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("store.sqlite3");
    let store = SqliteStore::open(&database)?;
    let initial = store.settings()?;
    assert_eq!(initial.revision, 0);
    assert_eq!(initial.inference_artifact_limit_bytes(), 16_000_000_000);
    let saved = store.save_settings(Settings {
        answer_language: AnswerLanguage::Fr,
        diagnostic_storage_limit_mb: Some(5),
        inference_artifact_limit_gb: Some(24),
        ..initial.clone()
    })?;
    assert_eq!(saved.revision, 1);
    assert_eq!(store.save_settings(initial), Err(Failure::Conflict));
    assert_eq!(
        store.save_settings(Settings {
            diagnostic_retention_hours: 0,
            ..saved.clone()
        }),
        Err(Failure::InvalidRetention)
    );
    assert_eq!(
        store.save_settings(Settings {
            diagnostic_storage_limit_mb: Some(1),
            ..saved.clone()
        }),
        Err(Failure::InvalidStorageLimit)
    );
    assert_eq!(
        store.save_settings(Settings {
            inference_artifact_limit_gb: Some(0),
            ..saved.clone()
        }),
        Err(Failure::InvalidInferenceArtifactLimit)
    );
    let old_client: Settings = serde_json::from_value(json!({
        "revision": saved.revision, "answerLanguage": "fr", "diagnosticRetentionHours": saved.diagnostic_retention_hours
    }))?;
    assert_eq!(old_client.inference_artifact_limit_gb, None);
    let saved = store.save_settings(old_client)?;
    assert_eq!(saved.diagnostic_storage_limit_mb, Some(5));
    assert_eq!(saved.inference_artifact_limit_gb, Some(24));
    drop(store);
    let reopened = SqliteStore::open(&database)?;
    assert_eq!(reopened.settings()?, saved);
    assert_eq!(
        reopened
            .diagnostics()?
            .first()
            .map(|event| event.code.as_str()),
        Some("settings_saved")
    );
    Ok(())
}

#[test]
fn incremental_commit_and_deletion_protect_the_search_index() -> Result<(), Box<dyn Error>> {
    let store = SqliteStore::open(std::path::Path::new(":memory:"))?;
    let documents = [
        Document {
            source: "test/main.rs",
            revision: "a",
            kind: DocumentKind::Current {
                tracked_ref: "refs/heads/main",
            },
            title: "current",
            content: "rate limit",
        },
        Document {
            source: "test/main.rs",
            revision: "b",
            kind: DocumentKind::History,
            title: "history",
            content: "rate limit changed",
        },
        Document {
            source: "test/ci.yml",
            revision: "b",
            kind: DocumentKind::Workflow,
            title: "workflow",
            content: "build",
        },
    ];
    let mut batch = Batch {
        collection: "first",
        generation: 0,
        previous_cursor: None,
        next_cursor: "page-1",
        documents: &documents,
    };
    assert_eq!(store.commit_batch(&batch)?, Commit::Indexed);
    assert_eq!(store.commit_batch(&batch)?, Commit::AlreadyIndexed);
    let query = SearchQuery::parse("rate-limit")?;
    assert_eq!(store.search(&query)?.len(), 2);
    assert_eq!(store.status()?.indexed_documents, 3);
    batch.next_cursor = "page-2";
    assert_eq!(store.commit_batch(&batch)?, Commit::StaleCheckpoint);
    batch.previous_cursor = Some("page-1");
    assert_eq!(store.commit_batch(&batch)?, Commit::Indexed);
    let second = Batch {
        collection: "second",
        previous_cursor: None,
        ..batch
    };
    assert_eq!(store.commit_batch(&second)?, Commit::Indexed);
    assert_eq!(store.search(&query)?.len(), 2);
    store.delete_collection("first")?;
    assert_eq!(store.commit_batch(&batch)?, Commit::StaleCheckpoint);
    assert_eq!(store.search(&query)?.len(), 2);
    store.delete_collection("second")?;
    assert!(store.search(&query)?.is_empty());
    assert_eq!(SearchQuery::parse("%%"), Err(Failure::InvalidQuery));
    assert_eq!(
        SearchQuery::parse(&"a".repeat(513)),
        Err(Failure::InvalidQuery)
    );
    Ok(())
}

#[test]
fn session_is_exclusive_and_publishes_private_discovery() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let private = directory.path().join("private");
    let mut first = Session::acquire(&private)?;
    assert!(Session::acquire(&private).is_err());
    first.publish(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9999), "credential")?;
    let endpoint_file = private.join("endpoint.json");
    assert_eq!(
        std::fs::metadata(&endpoint_file)?.permissions().mode() & 0o777,
        0o600
    );
    let endpoint: Value = serde_json::from_slice(&std::fs::read(&endpoint_file)?)?;
    assert_eq!(endpoint["url"], "http://127.0.0.1:9999");
    assert_eq!(endpoint["token"], "credential");
    drop(first);
    assert!(!endpoint_file.exists());
    let _next_session = Session::acquire(&private)?;
    Ok(())
}

#[tokio::test]
async fn local_api_authentication_and_settings_contract() -> Result<(), Box<dyn Error>> {
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    store.record("backend_started", "Backend started")?;
    let app = http::router(store, "fixture-token");
    let cases = [
        (
            "GET",
            "/v1/retrieval",
            None,
            None,
            Value::Null,
            401,
            "unauthorized",
        ),
        (
            "GET",
            "/v1/telemetry",
            None,
            None,
            Value::Null,
            401,
            "unauthorized",
        ),
        (
            "GET",
            "/v1/status",
            None,
            None,
            Value::Null,
            401,
            "unauthorized",
        ),
        (
            "GET",
            "/v1/status",
            Some("Bearer invalid"),
            None,
            Value::Null,
            401,
            "unauthorized",
        ),
        (
            "GET",
            "/v1/status",
            Some("Bearer fixture-token"),
            Some("http://evil.invalid"),
            Value::Null,
            401,
            "unauthorized",
        ),
        (
            "GET",
            "/v1/status",
            Some("Bearer fixture-token"),
            None,
            Value::Null,
            200,
            "",
        ),
    ];
    check_api(&app, cases).await
}

#[tokio::test]
async fn expanded_answer_language_preserves_the_legacy_settings_contract()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("store.sqlite3");
    let app = http::router(Arc::new(SqliteStore::open(&database)?), "fixture-token");
    let (status, selected) = update_language_settings(
        &app,
        json!({
            "revision": 0, "answerLanguage": "auto", "answerLocale": "es",
            "diagnosticRetentionHours": 24,
        }),
    )
    .await?;
    assert_eq!(status, 200);
    assert_eq!(selected["answerLanguage"], "auto");
    assert_eq!(selected["answerLocale"], "es");
    assert!(
        selected["answerLanguageOptions"]
            .as_array()
            .is_some_and(|options| {
                options
                    .iter()
                    .any(|item| item["code"] == "es" && item["nativeName"] == "Español")
            })
    );

    let (status, preserved) = update_language_settings(
        &app,
        json!({
            "revision": 1, "answerLanguage": "auto", "diagnosticRetentionHours": 36,
        }),
    )
    .await?;
    assert_eq!(status, 200);
    assert_eq!(preserved["answerLocale"], "es");
    assert_eq!(preserved["diagnosticRetentionHours"], 36);
    let (status, rejected) = update_language_settings(
        &app,
        json!({
            "revision": 2, "answerLanguage": "auto", "answerLocale": "zz",
            "diagnosticRetentionHours": 36,
        }),
    )
    .await?;
    assert_eq!(status, 400);
    assert_eq!(rejected["code"], "invalid_settings");
    drop(app);

    let reopened = Arc::new(SqliteStore::open(&database)?);
    assert_eq!(
        reopened.settings()?.preferred_answer_language(),
        AnswerLanguage::Es
    );
    let reopened = http::router(reopened, "fixture-token");
    let (status, fallback) = update_language_settings(
        &reopened,
        json!({
            "revision": 2, "answerLanguage": "fr", "diagnosticRetentionHours": 36,
        }),
    )
    .await?;
    assert_eq!(status, 200);
    assert_eq!(fallback["answerLanguage"], "fr");
    assert!(fallback.get("answerLocale").is_none());
    Ok(())
}

async fn update_language_settings(
    app: &axum::Router,
    body: Value,
) -> Result<(u16, Value), Box<dyn Error>> {
    let response = app
        .clone()
        .oneshot(
            Request::put("/v1/settings")
                .header("authorization", "Bearer fixture-token")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body)?))?,
        )
        .await?;
    let status = response.status().as_u16();
    let body = serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await?)?;
    Ok((status, body))
}

#[tokio::test]
async fn settings_and_diagnostics_accept_the_existing_contract() -> Result<(), Box<dyn Error>> {
    let app = http::router(
        Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?),
        "fixture-token",
    );
    let default = json!({"revision": 0, "answerLanguage": "en", "diagnosticRetentionHours": 24});
    let cases = [
        (
            "GET",
            "/v1/settings",
            Some("Bearer fixture-token"),
            None,
            Value::Null,
            200,
            "",
        ),
        (
            "PUT",
            "/v1/settings",
            Some("Bearer fixture-token"),
            None,
            Value::Null,
            400,
            "invalid_settings",
        ),
        (
            "PUT",
            "/v1/settings",
            Some("Bearer fixture-token"),
            None,
            json!({"revision":0,"answerLanguage":"fr","diagnosticRetentionHours":0}),
            400,
            "invalid_settings",
        ),
        (
            "PUT",
            "/v1/settings",
            Some("Bearer fixture-token"),
            None,
            default.clone(),
            200,
            "",
        ),
        (
            "PUT",
            "/v1/settings",
            Some("Bearer fixture-token"),
            None,
            default,
            409,
            "stale_settings",
        ),
    ];
    check_api(&app, cases).await?;
    let cases = [(
        "GET",
        "/v1/diagnostics",
        Some("Bearer fixture-token"),
        None,
        Value::Null,
        200,
        "",
    )];
    check_api(&app, cases).await
}

#[tokio::test]
async fn answers_require_authentication_and_a_configured_runtime() -> Result<(), Box<dyn Error>> {
    let store = Arc::new(SqliteStore::open(std::path::Path::new(":memory:"))?);
    let app = http::router(store, "fixture-token");
    check_api(
        &app,
        [
            (
                "PUT",
                "/v1/generation/models",
                None,
                None,
                json!({"profile":"unavailable"}),
                401,
                "unauthorized",
            ),
            (
                "GET",
                "/v1/generation/models",
                Some("Bearer fixture-token"),
                None,
                Value::Null,
                200,
                "",
            ),
            (
                "DELETE",
                "/v1/answers/aa3d5ed2-c0cf-4185-944c-d61ad2004835",
                None,
                None,
                Value::Null,
                401,
                "unauthorized",
            ),
            (
                "POST",
                "/v1/answers",
                None,
                None,
                json!({"query":"How are requests paced?"}),
                401,
                "unauthorized",
            ),
            (
                "POST",
                "/v1/answers",
                Some("Bearer fixture-token"),
                None,
                json!({"query":"  "}),
                400,
                "invalid_query",
            ),
            (
                "POST",
                "/v1/answers",
                Some("Bearer fixture-token"),
                None,
                json!({"query":"How are requests paced?"}),
                503,
                "generation_not_configured",
            ),
        ],
    )
    .await
}

type ApiCase = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    Value,
    u16,
    &'static str,
);

async fn check_api(
    app: &axum::Router,
    cases: impl IntoIterator<Item = ApiCase>,
) -> Result<(), Box<dyn Error>> {
    for (method, path, token, origin, value, status, code) in cases {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(token) = token {
            request = request.header("authorization", token);
        }
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(value.to_string()))?)
            .await?;
        assert_eq!(response.status(), status, "{method} {path}");
        assert_eq!(
            response.headers()["content-type"],
            "application/json",
            "{method} {path}"
        );
        let payload: Value = serde_json::from_slice(&to_bytes(response.into_body(), 4096).await?)?;
        if !code.is_empty() {
            assert_eq!(payload["code"], code, "{method} {path}");
            assert!(
                payload["message"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "{method} {path}"
            );
        }
    }
    Ok(())
}
