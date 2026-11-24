use axum::extract::{State, rejection::JsonRejection};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};

use super::{
    Api,
    response::{problem, read},
};
use crate::application::Settings;

pub(super) fn routes() -> Router<Api> {
    Router::new()
        .route("/v1/status", get(status))
        .route("/v1/settings", get(settings).put(save_settings))
        .route("/v1/diagnostics", get(diagnostics))
}

async fn status(State(api): State<Api>) -> Response {
    let retrieval = api.retrieval.clone();
    read(api, move |store| {
        let mut status = store.status()?;
        if let Some(retrieval) = retrieval {
            status.retrieval = retrieval.progress().ok();
        }
        Ok(status)
    })
    .await
}

async fn settings(State(api): State<Api>) -> Response {
    read(api, |store| {
        store.settings().map(Settings::with_answer_language_options)
    })
    .await
}

async fn diagnostics(State(api): State<Api>) -> Response {
    read(api, |store| store.diagnostics()).await
}

async fn save_settings(
    State(api): State<Api>,
    input: Result<Json<Settings>, JsonRejection>,
) -> Response {
    let Ok(Json(settings)) = input else {
        return problem(
            StatusCode::BAD_REQUEST,
            "invalid_settings",
            "Send a valid settings document.".to_owned(),
            None,
        );
    };
    let telemetry = api.telemetry.clone();
    read(api, move |store| {
        let saved = store.save_settings(settings)?;
        if let Some(telemetry) = telemetry {
            telemetry.apply_settings(&saved);
        }
        Ok(saved.with_answer_language_options())
    })
    .await
}
