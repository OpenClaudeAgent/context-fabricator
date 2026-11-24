use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router, middleware};
use serde::Deserialize;

use super::{
    Api,
    response::{failure, problem},
    telemetry::parse_trace_id,
};
use crate::application::{
    Failure,
    answer_history::{Entry, Journal, Settings},
};
use crate::runtime::clock::Clock;

pub(super) fn routes() -> Router<Api> {
    Router::new()
        .route("/v1/answer-history", get(list).delete(clear))
        .route("/v1/answer-history/{id}", get(detail))
        .route("/v1/answer-history/by-trace/{traceId}", get(by_trace))
        .route("/v1/answer-history/settings", get(settings).put(update))
        .layer(middleware::map_response(
            |mut response: Response| async move {
                response
                    .headers_mut()
                    .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                response
            },
        ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cursor {
    before_id: Option<i64>,
}

async fn list(State(api): State<Api>, input: Result<Query<Cursor>, QueryRejection>) -> Response {
    let Ok(Query(cursor)) = input else {
        return invalid("beforeId");
    };
    if cursor.before_id.is_some_and(|id| id <= 0) {
        return invalid("beforeId");
    }
    run(api, move |journal| {
        journal
            .list(cursor.before_id, Clock::system().now())
            .map(Json)
    })
    .await
}

async fn detail(State(api): State<Api>, input: Result<Path<i64>, PathRejection>) -> Response {
    let Ok(Path(id)) = input else {
        return invalid("id");
    };
    if id <= 0 {
        return invalid("id");
    }
    run(api, move |journal| {
        journal.get(id, Clock::system().now()).map(entry_response)
    })
    .await
}

async fn by_trace(State(api): State<Api>, input: Result<Path<String>, PathRejection>) -> Response {
    let Ok(Path(value)) = input else {
        return invalid("traceId");
    };
    let Some(trace_id) = parse_trace_id(&value) else {
        return invalid("traceId");
    };
    run(api, move |journal| {
        journal
            .get_by_trace(trace_id, Clock::system().now())
            .map(entry_response)
    })
    .await
}

fn entry_response(entry: Option<Entry>) -> Response {
    entry.map_or_else(
        || {
            problem(
                StatusCode::NOT_FOUND,
                "answer_history_entry_unavailable",
                "The saved answer is no longer available.".to_owned(),
                None,
            )
        },
        |entry| Json(entry).into_response(),
    )
}

async fn settings(State(api): State<Api>) -> Response {
    run(api, |journal| journal.settings().map(Json)).await
}

async fn update(State(api): State<Api>, input: Result<Json<Settings>, JsonRejection>) -> Response {
    let Ok(Json(settings)) = input else {
        return invalid("settings");
    };
    run(api, move |journal| {
        journal.update(settings, Clock::system().now()).map(Json)
    })
    .await
}

async fn clear(State(api): State<Api>) -> Response {
    run(api, |journal| {
        journal.clear()?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

async fn run<T: IntoResponse + Send + 'static>(
    api: Api,
    action: impl FnOnce(&dyn Journal) -> Result<T, Failure> + Send + 'static,
) -> Response {
    let Some(journal) = api.answer_history else {
        return failure(Failure::Storage);
    };
    match tokio::task::spawn_blocking(move || action(&*journal)).await {
        Ok(Ok(output)) => output.into_response(),
        Ok(Err(error)) => failure(error),
        Err(_) => failure(Failure::Storage),
    }
}

fn invalid(field: &'static str) -> Response {
    problem(
        StatusCode::BAD_REQUEST,
        "invalid_answer_history_input",
        "Supply valid answer history input.".to_owned(),
        Some(field),
    )
}
