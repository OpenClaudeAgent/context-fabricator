use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};

use super::{Api, response::problem};
use crate::application::{artifact_storage::StoragePlan, runtime_setup::RuntimeSetup};

pub(super) fn routes() -> Router<Api> {
    Router::new().route("/v1/inference/storage", get(read))
}

async fn read(State(api): State<Api>) -> Response {
    let Some(report) = api.artifact_storage else {
        return Json(StoragePlan::default()).into_response();
    };
    let Ok(Ok(capacity)) = tokio::task::spawn_blocking(move || report.capacity()).await else {
        return unavailable();
    };
    let runtime = api
        .runtime_setup
        .as_deref()
        .and_then(RuntimeSetup::storage_peaks);
    let models = match api.downloads {
        Some(downloads) => match downloads.snapshot().await {
            Ok(catalog) => catalog,
            Err(_) => return unavailable(),
        },
        None => Vec::new(),
    };
    Json(StoragePlan::estimate(capacity, runtime, models)).into_response()
}

fn unavailable() -> Response {
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "inference_storage_unavailable",
        "Inference storage capacity is unavailable.".into(),
        None,
    )
}
