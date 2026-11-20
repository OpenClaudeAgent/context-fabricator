use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use super::{
    Api,
    response::{failure, problem, respond},
};
use crate::application::Failure;
use crate::application::telemetry::{Command, LogFilter};

pub(super) fn routes() -> Router<Api> {
    Router::new()
        .route("/v1/telemetry", get(snapshot).put(command))
        .route("/v1/telemetry/logs", get(logs))
        .route("/v1/telemetry/traces/{traceId}", get(trace))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TelemetryRequest {
    command: Command,
}

async fn snapshot(State(api): State<Api>) -> Response {
    let Some(telemetry) = api.telemetry else {
        return failure(Failure::Storage);
    };
    respond(move || Ok(telemetry.snapshot())).await
}

async fn command(
    State(api): State<Api>,
    input: Result<Json<TelemetryRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = input else {
        return problem(
            StatusCode::BAD_REQUEST,
            "invalid_telemetry_control",
            "Select a diagnostic command.".into(),
            Some("command"),
        );
    };
    let Some(telemetry) = api.telemetry else {
        return failure(Failure::Storage);
    };
    respond(move || telemetry.command(request.command)).await
}

async fn logs(State(api): State<Api>, input: Result<Query<LogFilter>, QueryRejection>) -> Response {
    let Ok(Query(filter)) = input else {
        return invalid_filter();
    };
    if filter.before_id.is_some_and(|id| id < 1)
        || filter
            .query
            .as_ref()
            .is_some_and(|query| query.len() > 512 || query.chars().count() > 128)
    {
        return invalid_filter();
    }
    let Some(telemetry) = api.telemetry else {
        return failure(Failure::Storage);
    };
    respond(move || telemetry.logs(&filter)).await
}

fn invalid_filter() -> Response {
    problem(
        StatusCode::BAD_REQUEST,
        "invalid_diagnostic_log_filter",
        "Enter a query of up to 128 characters and choose a supported log level.".into(),
        None,
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TraceQuery {
    before_id: Option<i64>,
}

async fn trace(
    State(api): State<Api>,
    path: Result<Path<String>, PathRejection>,
    query: Result<Query<TraceQuery>, QueryRejection>,
) -> Response {
    let Ok(Path(trace_id)) = path else {
        return invalid_trace_id();
    };
    if trace_id.len() != 32
        || !trace_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid_trace_id();
    }
    let before_id = match query {
        Ok(Query(TraceQuery { before_id })) if before_id.is_none_or(|id| id > 0) => before_id,
        _ => {
            return problem(
                StatusCode::BAD_REQUEST,
                "invalid_trace_cursor",
                "Supply a positive trace page cursor.".into(),
                Some("beforeId"),
            );
        }
    };
    let Some(telemetry) = api.telemetry else {
        return failure(Failure::Storage);
    };
    respond(move || telemetry.trace(&trace_id, before_id)).await
}

fn invalid_trace_id() -> Response {
    problem(
        StatusCode::BAD_REQUEST,
        "invalid_trace_id",
        "Supply a 32-character lowercase hexadecimal trace ID.".into(),
        Some("traceId"),
    )
}
