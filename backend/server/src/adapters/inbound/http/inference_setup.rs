use std::path::PathBuf;

use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use serde::Deserialize;
use uuid::Uuid;

use super::{Api, response::problem};
use crate::application::inference_setup::{
    Confirmation, JournalError, PlanError, SetupError, SetupSelection,
};

pub(super) fn routes() -> Router<Api> {
    Router::new()
        .route("/v1/inference/setup/plan", post(plan))
        .route("/v1/inference/setup", get(status).post(start))
        .route("/v1/inference/setup/archive", post(resume_archive))
        .route(
            "/v1/inference/setup/archive/{operation_id}",
            delete(cancel_archive),
        )
}

async fn plan(
    State(api): State<Api>,
    input: Result<Json<SetupSelection>, JsonRejection>,
) -> Response {
    let Ok(Json(input)) = input else {
        return rejection(&SetupError::Plan(PlanError::InvalidSelection));
    };
    let Some(setup) = api.inference_setup else {
        return rejection(&SetupError::Unavailable);
    };
    match setup.preview(input).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => rejection(&error),
    }
}

async fn start(
    State(api): State<Api>,
    input: Result<Json<Confirmation>, JsonRejection>,
) -> Response {
    let Ok(Json(input)) = input else {
        return problem(
            StatusCode::BAD_REQUEST,
            "invalid_inference_setup",
            "Review the installation choices and confirmation.".into(),
            None,
        );
    };
    if input.fingerprint.len() != 64
        || !input
            .fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return problem(
            StatusCode::BAD_REQUEST,
            "invalid_inference_setup",
            "A valid setup fingerprint is required.".into(),
            Some("fingerprint"),
        );
    }
    let Some(setup) = api.inference_setup else {
        return rejection(&SetupError::Unavailable);
    };
    match setup.start(input).await {
        Ok(result) => (StatusCode::ACCEPTED, Json(result)).into_response(),
        Err(error) => rejection(&error),
    }
}

async fn status(State(api): State<Api>) -> Response {
    match api
        .inference_setup
        .as_ref()
        .map(|setup| setup.status())
        .transpose()
    {
        Ok(Some(Some(status))) => Json(status).into_response(),
        Ok(_) => problem(
            StatusCode::NOT_FOUND,
            "inference_setup_not_started",
            "No inference setup has been confirmed.".into(),
            None,
        ),
        Err(error) => rejection(&error.into()),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Archive {
    operation_id: Uuid,
    archive_path: PathBuf,
}

async fn resume_archive(
    State(api): State<Api>,
    input: Result<Json<Archive>, JsonRejection>,
) -> Response {
    let Ok(Json(archive)) = input else {
        return rejection(&SetupError::ArchiveRequired);
    };
    let Some(setup) = api.inference_setup else {
        return rejection(&SetupError::Unavailable);
    };
    match setup
        .provide_archive(archive.operation_id, archive.archive_path)
        .await
    {
        Ok(result) => (StatusCode::ACCEPTED, Json(result)).into_response(),
        Err(error) => rejection(&error),
    }
}

async fn cancel_archive(State(api): State<Api>, Path(operation_id): Path<String>) -> Response {
    let Ok(id) = Uuid::parse_str(&operation_id) else {
        return problem(
            StatusCode::BAD_REQUEST,
            "invalid_inference_setup",
            "A valid setup operation ID is required.".into(),
            Some("operationId"),
        );
    };
    let Some(setup) = api.inference_setup else {
        return rejection(&SetupError::Unavailable);
    };
    match setup.cancel_archive(id).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => rejection(&error),
    }
}

fn rejection(error: &SetupError) -> Response {
    let (status, code) = match error {
        SetupError::Plan(PlanError::InvalidSelection) | SetupError::ArchiveRequired => {
            (StatusCode::BAD_REQUEST, "invalid_inference_setup")
        }
        SetupError::StalePlan => (StatusCode::CONFLICT, "stale_inference_setup_plan"),
        SetupError::Blocked => (StatusCode::CONFLICT, "inference_setup_blocked"),
        SetupError::Journal(JournalError::Conflict) => {
            (StatusCode::CONFLICT, "inference_setup_conflict")
        }
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            "inference_setup_unavailable",
        ),
    };
    problem(status, code, error.to_string(), None)
}
