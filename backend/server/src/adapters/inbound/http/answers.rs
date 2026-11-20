use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use ctxfab_inference::generation::{GenerationError, Models};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use super::{
    Api,
    response::{failure, problem, traced},
};
use crate::application::{Failure, SearchQuery, answers::AnswerError, telemetry::Stage};

pub(super) fn routes() -> Router<Api> {
    Router::new()
        .route("/v1/answers", post(ask))
        .route("/v1/answers/{requestId}", delete(cancel_answer))
        .route(
            "/v1/generation/models",
            get(generation_models).put(select_model),
        )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Question {
    query: String,
    request_id: Option<String>,
    #[serde(default, deserialize_with = "scope_value")]
    repository_id: Option<i64>,
    #[serde(default, deserialize_with = "scope_value")]
    branch: Option<String>,
}

fn scope_value<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    input: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(input).map(Some)
}

async fn ask(State(api): State<Api>, input: Result<Json<Question>, JsonRejection>) -> Response {
    let span = api
        .telemetry
        .as_ref()
        .map(|observer| observer.start(Stage::ApiAnswer, None));
    let Ok(Json(question)) = input else {
        return traced(failure(Failure::InvalidQuery), span);
    };
    let query =
        match SearchQuery::parse_scoped(&question.query, question.repository_id, question.branch) {
            Ok(query) => query,
            Err(error) => return traced(failure(error), span),
        };
    let Ok(id) = question
        .request_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
    else {
        return traced(invalid_request_id(), span);
    };
    let Some(answers) = api.answers else {
        return traced(
            problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "generation_not_configured",
                "The local generation model is not configured.".into(),
                None,
            ),
            span,
        );
    };
    let trace = span.as_ref().map(|span| span.link());
    let result = tokio::task::spawn_blocking(move || {
        answers.ask(
            &query,
            id,
            api.telemetry
                .as_deref()
                .map(|observer| observer as &dyn crate::application::telemetry::Observer),
            trace,
        )
    })
    .await;
    let response = match result {
        Ok(Ok(answer)) => Json(answer).into_response(),
        Ok(Err(error)) => answer_failure(error),
        Err(_) => failure(Failure::Storage),
    };
    traced(response, span)
}

fn answer_failure(error: AnswerError) -> Response {
    match error {
        AnswerError::Storage => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "answer_storage_unavailable",
            error.to_string(),
            None,
        ),
        AnswerError::Generation(error) => generation_failure(error),
    }
}

fn generation_failure(error: GenerationError) -> Response {
    let (status, code) = match error {
        GenerationError::Runtime => (
            StatusCode::SERVICE_UNAVAILABLE,
            "generation_runtime_unavailable",
        ),
        GenerationError::Busy => (StatusCode::SERVICE_UNAVAILABLE, "generation_busy"),
        GenerationError::Output => (StatusCode::SERVICE_UNAVAILABLE, "generation_invalid_output"),
        GenerationError::Cancelled => (StatusCode::CONFLICT, "answer_cancelled"),
        GenerationError::UnknownRequest => (StatusCode::NOT_FOUND, "answer_not_active"),
        GenerationError::UnknownProfile => (StatusCode::NOT_FOUND, "generation_profile_unknown"),
        GenerationError::ProfileUnavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "generation_profile_unavailable",
        ),
    };
    problem(status, code, error.to_string(), None)
}

fn invalid_request_id() -> Response {
    problem(
        StatusCode::BAD_REQUEST,
        "invalid_request_id",
        "Supply a UUID for the answer request.".into(),
        Some("requestId"),
    )
}

async fn cancel_answer(State(api): State<Api>, Path(request): Path<String>) -> Response {
    let Ok(id) = Uuid::parse_str(&request) else {
        return invalid_request_id();
    };
    let Some(answers) = api.answers else {
        return generation_failure(GenerationError::UnknownRequest);
    };
    let span = api
        .telemetry
        .as_ref()
        .map(|observer| observer.start(Stage::AnswerCancellation, None));
    let response = inference_response(move || {
        answers.cancel(id)?;
        Ok(serde_json::json!({"requestId":id.to_string(),"status":"cancelled"}))
    })
    .await;
    traced(response, span)
}

async fn generation_models(State(api): State<Api>) -> Response {
    inference_response(move || {
        api.answers
            .map_or_else(|| Ok(Models::default()), |answers| answers.models())
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    profile: String,
}

async fn select_model(
    State(api): State<Api>,
    input: Result<Json<Selection>, JsonRejection>,
) -> Response {
    let Ok(Json(selection)) = input else {
        return problem(
            StatusCode::BAD_REQUEST,
            "invalid_generation_profile",
            "Supply a generation profile ID.".into(),
            Some("profile"),
        );
    };
    let Some(answers) = api.answers else {
        return generation_failure(GenerationError::UnknownProfile);
    };
    let span = api
        .telemetry
        .as_ref()
        .map(|observer| observer.start(Stage::GenerationSelection, None));
    let response = inference_response(move || answers.select_model(&selection.profile)).await;
    traced(response, span)
}

async fn inference_response<T: Serialize + Send + 'static>(
    operation: impl FnOnce() -> Result<T, GenerationError> + Send + 'static,
) -> Response {
    match tokio::task::spawn_blocking(operation).await {
        Ok(Ok(payload)) => Json(payload).into_response(),
        Ok(Err(error)) => generation_failure(error),
        Err(_) => failure(Failure::Storage),
    }
}
