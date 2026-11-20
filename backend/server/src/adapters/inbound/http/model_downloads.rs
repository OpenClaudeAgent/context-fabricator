use axum::extract::{State, rejection::JsonRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::{Api, response::problem};
use crate::runtime::model_downloads::DownloadError;

pub(super) fn routes() -> Router<Api> {
    Router::new().route("/v1/model-downloads", get(catalog).post(request_download))
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

fn rejected(error: DownloadError) -> Response {
    let status = if error == DownloadError::UnknownProfile {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    problem(status, error.code(), error.to_string(), None)
}
