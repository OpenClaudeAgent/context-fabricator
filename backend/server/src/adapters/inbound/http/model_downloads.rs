use axum::body::Bytes;
use axum::extract::{Path, State, rejection::JsonRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::{Api, response::problem};
use crate::domain::model::DownloadError;

pub(super) fn routes() -> Router<Api> {
    Router::new()
        .route("/v1/model-downloads", get(catalog).post(request_download))
        .route("/v1/model-downloads/{profile}", delete(remove))
        .route(
            "/v1/model-downloads/{profile}/removal-plan",
            get(removal_plan),
        )
        .route(
            "/v1/model-downloads/{profile}/partial",
            delete(discard_partial),
        )
        .route("/v1/model-downloads/{profile}/pause", post(pause))
        .route("/v1/model-downloads/{profile}/resume", post(resume))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DownloadRequest {
    profile: String,
}

async fn catalog(State(api): State<Api>) -> Response {
    let Some(downloads) = api.downloads else {
        return rejected(DownloadError::Unavailable);
    };
    match downloads.snapshot().await {
        Ok(catalog) => Json(catalog).into_response(),
        Err(error) => rejected(error),
    }
}

async fn request_download(
    State(api): State<Api>,
    input: Result<Json<DownloadRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = input else {
        return problem(
            StatusCode::BAD_REQUEST,
            "invalid_model_download",
            "Send a valid model profile.".into(),
            Some("profile"),
        );
    };
    if request.profile.is_empty() || request.profile.len() > 128 {
        return problem(
            StatusCode::BAD_REQUEST,
            "invalid_model_download",
            "Send a model profile from the download catalog.".into(),
            Some("profile"),
        );
    }
    let Some(downloads) = api.downloads else {
        return rejected(DownloadError::Unavailable);
    };
    match downloads.request(&request.profile) {
        Ok(()) => (StatusCode::ACCEPTED, Json(request)).into_response(),
        Err(error) => rejected(error),
    }
}

async fn discard_partial(State(api): State<Api>, Path(profile): Path<String>) -> Response {
    let Some(downloads) = api.downloads else {
        return rejected(DownloadError::Unavailable);
    };
    match tokio::task::spawn_blocking(move || downloads.discard_partial(&profile)).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(error)) => rejected(error),
        Err(_) => rejected(DownloadError::Unavailable),
    }
}

async fn pause(State(api): State<Api>, Path(profile): Path<String>) -> Response {
    let Some(downloads) = api.downloads else {
        return rejected(DownloadError::Unavailable);
    };
    match tokio::task::spawn_blocking(move || downloads.pause(&profile)).await {
        Ok(Ok(())) => StatusCode::ACCEPTED.into_response(),
        Ok(Err(error)) => rejected(error),
        Err(_) => rejected(DownloadError::Unavailable),
    }
}

async fn resume(State(api): State<Api>, Path(profile): Path<String>) -> Response {
    let Some(downloads) = api.downloads else {
        return rejected(DownloadError::Unavailable);
    };
    match tokio::task::spawn_blocking(move || downloads.resume(&profile)).await {
        Ok(Ok(())) => StatusCode::ACCEPTED.into_response(),
        Ok(Err(error)) => rejected(error),
        Err(_) => rejected(DownloadError::Unavailable),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemovalRequest {
    identity: String,
}

async fn removal_plan(State(api): State<Api>, Path(profile): Path<String>) -> Response {
    let Some(downloads) = api.downloads else {
        return rejected(DownloadError::Unavailable);
    };
    match tokio::task::spawn_blocking(move || downloads.removal_plan(&profile)).await {
        Ok(Ok(plan)) => Json(plan).into_response(),
        Ok(Err(error)) => rejected(error),
        Err(_) => rejected(DownloadError::Unavailable),
    }
}

async fn remove(State(api): State<Api>, Path(profile): Path<String>, body: Bytes) -> Response {
    let request = if body.is_empty() {
        None
    } else {
        match serde_json::from_slice::<RemovalRequest>(&body) {
            Ok(request) if !request.identity.is_empty() && request.identity.len() <= 2048 => {
                Some(request)
            }
            _ => {
                return problem(
                    StatusCode::BAD_REQUEST,
                    "invalid_model_removal",
                    "Send the identity from the current model removal plan.".into(),
                    Some("identity"),
                );
            }
        }
    };
    let Some(downloads) = api.downloads else {
        return rejected(DownloadError::Unavailable);
    };
    match tokio::task::spawn_blocking(move || {
        if let Some(request) = request {
            downloads.remove_guarded(&profile, Some(&request.identity))
        } else {
            downloads.remove(&profile)
        }
    })
    .await
    {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(error)) => rejected(error),
        Err(_) => rejected(DownloadError::Unavailable),
    }
}

fn rejected(error: DownloadError) -> Response {
    let status = match error {
        DownloadError::UnknownProfile => StatusCode::NOT_FOUND,
        DownloadError::InUse | DownloadError::StaleRemoval => StatusCode::CONFLICT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    problem(status, error.code(), error.to_string(), None)
}
