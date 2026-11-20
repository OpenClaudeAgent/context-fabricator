use axum::Json;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use super::Api;
use crate::application::{Failure, ports::Repository, telemetry::Observation};

#[derive(Serialize)]
struct Problem {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<&'static str>,
}

pub(super) fn traced(mut response: Response, span: Option<Box<dyn Observation>>) -> Response {
    if let Some(span) = span {
        let trace_id = uuid::Uuid::from_bytes(span.link().trace)
            .simple()
            .to_string();
        if let Ok(value) = HeaderValue::from_str(&trace_id) {
            response.headers_mut().insert("x-context-trace-id", value);
        }
        span.finish(response.status().is_success());
    }
    response
}

pub(super) async fn read<T: Serialize + Send + 'static>(
    api: Api,
    operation: impl FnOnce(&dyn Repository) -> Result<T, Failure> + Send + 'static,
) -> Response {
    respond(move || operation(&*api.repository)).await
}

pub(super) async fn respond<T: Serialize + Send + 'static>(
    operation: impl FnOnce() -> Result<T, Failure> + Send + 'static,
) -> Response {
    match tokio::task::spawn_blocking(operation).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => failure(error),
        Err(_) => failure(Failure::Storage),
    }
}

pub(super) fn failure(error: Failure) -> Response {
    let (status, code, field) = match error {
        Failure::Storage => (StatusCode::SERVICE_UNAVAILABLE, "storage_unavailable", None),
        Failure::Conflict => (StatusCode::CONFLICT, "stale_settings", Some("revision")),
        Failure::InvalidRetention => (
            StatusCode::BAD_REQUEST,
            "invalid_settings",
            Some("diagnosticRetentionHours"),
        ),
        Failure::InvalidStorageLimit => (
            StatusCode::BAD_REQUEST,
            "invalid_settings",
            Some("diagnosticStorageLimitMb"),
        ),
        Failure::InvalidQuery => (StatusCode::BAD_REQUEST, "invalid_query", Some("query")),
        Failure::InvalidSearchScope => (StatusCode::BAD_REQUEST, "invalid_search_scope", None),
    };
    problem(status, code, error.to_string(), field)
}

pub(super) fn problem(
    status: StatusCode,
    code: &'static str,
    message: String,
    field: Option<&'static str>,
) -> Response {
    (
        status,
        Json(Problem {
            code,
            message,
            field,
        }),
    )
        .into_response()
}
