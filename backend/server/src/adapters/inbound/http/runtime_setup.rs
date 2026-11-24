use std::path::{Path, PathBuf};

use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;

use super::{Api, response::problem};
use crate::application::runtime_setup::{RuntimeSetup, RuntimeStatus};

pub(super) fn routes() -> Router<Api> {
    Router::new()
        .route("/v1/runtime", get(status))
        .route("/v1/runtime/install", post(install))
        .route("/v1/runtime/download", post(download))
}

async fn download(State(api): State<Api>) -> Json<RuntimeStatus> {
    Json(
        api.runtime_setup
            .as_ref()
            .map_or_else(RuntimeStatus::unavailable, RuntimeSetup::request_download),
    )
}

async fn status(State(api): State<Api>) -> Json<RuntimeStatus> {
    Json(
        api.runtime_setup
            .as_ref()
            .map_or_else(RuntimeStatus::unavailable, |setup| setup.status()),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Installation {
    archive_path: String,
}

async fn install(
    State(api): State<Api>,
    request: Result<Json<Installation>, JsonRejection>,
) -> Response {
    let Ok(Json(request)) = request else {
        return invalid();
    };
    if request.archive_path.chars().count() > 2048
        || request.archive_path.contains('\0')
        || !Path::new(&request.archive_path).is_absolute()
    {
        return invalid();
    }
    let Some(setup) = api.runtime_setup else {
        return Json(RuntimeStatus::unavailable()).into_response();
    };
    match tokio::task::spawn_blocking(move || setup.install(&PathBuf::from(request.archive_path)))
        .await
    {
        Ok(status) => Json(status).into_response(),
        Err(_) => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "runtime_setup_unavailable",
            "Runtime setup is unavailable.".into(),
            None,
        ),
    }
}

fn invalid() -> Response {
    problem(
        StatusCode::BAD_REQUEST,
        "invalid_runtime_archive",
        "Choose a local runtime archive.".into(),
        Some("archivePath"),
    )
}
