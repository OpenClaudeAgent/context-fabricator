use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use ctxfab_inference::generation::{GenerationError, Models};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use super::{
    Api,
    request_trace::RequestTrace,
    response::{failure, problem},
};
use crate::application::{
    Failure, SearchQuery,
    answers::{Answer, AnswerError, AnswerRequest, EvidenceReview, ModelContext, Submission},
    telemetry::{Event, Observer},
};
use crate::runtime::clock::Clock;

pub(super) mod benchmark;

pub(super) fn routes() -> Router<Api> {
    Router::new()
        .route("/v1/answers", post(ask))
        .route(
            "/v1/benchmarks/corpus",
            get(benchmark::list)
                .post(benchmark::load)
                .delete(benchmark::unload),
        )
        .route("/v1/benchmarks/corpus/embeddings", post(benchmark::embed))
        .route("/v1/benchmarks/actions", get(benchmark::actions::list))
        .route(
            "/v1/benchmarks/actions/downloads",
            get(benchmark::action_downloads::list).post(benchmark::action_downloads::download),
        )
        .route(
            "/v1/benchmarks/answers",
            post(benchmark::ask).layer(DefaultBodyLimit::max(benchmark::MAX_REQUEST_BYTES)),
        )
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
    #[serde(default)]
    include_model_context: bool,
    #[serde(default)]
    inspect_pipeline: bool,
    #[serde(default, deserialize_with = "scope_value")]
    repository_id: Option<i64>,
    #[serde(default, deserialize_with = "scope_value")]
    branch: Option<String>,
    #[serde(default, deserialize_with = "scope_value")]
    history_commit: Option<String>,
    #[serde(default, deserialize_with = "scope_value")]
    actions_run_id: Option<i64>,
    #[serde(default, deserialize_with = "scope_value")]
    actions_run_attempt: Option<u32>,
}

fn scope_value<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    input: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(input).map(Some)
}

async fn ask(
    State(api): State<Api>,
    Extension(request): Extension<RequestTrace>,
    input: Result<Json<Question>, JsonRejection>,
) -> Response {
    let Ok(Json(mut question)) = input else {
        return failure(Failure::InvalidQuery);
    };
    let query = match SearchQuery::parse_scoped(
        &question.query,
        question.repository_id,
        question.branch.take(),
    ) {
        Ok(query) => query,
        Err(error) => return failure(error),
    };
    let Ok(id) = question
        .request_id
        .as_deref()
        .map(Uuid::parse_str)
        .transpose()
    else {
        return invalid_request_id();
    };
    let trace = request.clone();
    let result = tokio::task::spawn_blocking(move || {
        match Submission::parse(
            &query,
            question.history_commit.as_deref(),
            question.actions_run_id,
            question.actions_run_attempt,
        ) {
            Ok(submission) => answer_question(&api, &submission, id, &trace, &question),
            Err(error) => (failure(error), Event::AnswerFailed),
        }
    })
    .await;
    let (response, outcome) =
        result.unwrap_or_else(|_| (failure(Failure::Storage), Event::AnswerFailed));
    request.event(outcome);
    response
}

fn answer_question(
    api: &Api,
    submission: &Submission<'_>,
    id: Option<Uuid>,
    request_trace: &RequestTrace,
    question: &Question,
) -> (Response, Event) {
    let Some(answers) = &api.answers else {
        return generation_missing();
    };
    let observer = api
        .telemetry
        .as_deref()
        .map(|observer| observer as &dyn Observer);
    let metadata = AnswerRequest {
        id,
        observer,
        trace: request_trace.link(),
        include_model_context: question.include_model_context || question.inspect_pipeline,
        inspect_pipeline: question.inspect_pipeline,
        record_model: &|model| request_trace.model_profile(model),
    };
    let journal = if question.inspect_pipeline {
        None
    } else {
        api.answer_history.as_deref()
    };
    let result = submission.execute(answers, &metadata, journal, || Clock::system().now());
    present_answer(result)
}

fn present_answer(result: Result<Answer, AnswerError>) -> (Response, Event) {
    match result {
        Ok(answer) => {
            let outcome = if answer.outcome == "answered" {
                Event::AnswerPublished
            } else {
                Event::AnswerAbstained
            };
            let response = PresentedAnswer {
                model_context: answer.model_context.as_ref(),
                evidence_review: answer.evidence_review.as_ref(),
                answer: &answer,
            };
            (
                ([(header::CACHE_CONTROL, "no-store")], Json(response)).into_response(),
                outcome,
            )
        }
        Err(error) => {
            let outcome = if matches!(error, AnswerError::Generation(GenerationError::Cancelled)) {
                Event::AnswerCancelled
            } else {
                Event::AnswerFailed
            };
            (answer_failure(error), outcome)
        }
    }
}

fn generation_missing() -> (Response, Event) {
    (
        problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "generation_not_configured",
            "The local generation model is not configured.".into(),
            None,
        ),
        Event::AnswerFailed,
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PresentedAnswer<'a> {
    #[serde(flatten)]
    answer: &'a Answer,
    #[serde(skip_serializing_if = "Option::is_none")]
    model_context: Option<&'a ModelContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence_review: Option<&'a EvidenceReview>,
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
        GenerationError::Runtime | GenerationError::Deadline => (
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
    inference_response(move || {
        answers.cancel(id)?;
        Ok(serde_json::json!({"requestId":id.to_string(),"status":"cancelled"}))
    })
    .await
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
    inference_response(move || answers.select_model(&selection.profile)).await
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
