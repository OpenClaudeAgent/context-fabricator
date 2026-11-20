use axum::extract::{Query, State, rejection::QueryRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::{
    Api,
    response::{failure, problem, traced},
};
use crate::application::{
    Failure,
    retrieval::{Progress, RetrievalError},
    telemetry::Stage,
};

pub(super) fn routes() -> Router<Api> {
    Router::new()
        .route("/v1/retrieval", get(retrieval_progress))
        .route("/v1/retrieval/rebuild", post(rebuild))
}

#[derive(Deserialize)]
struct Rebuild {
    #[serde(default = "rebuild_limit")]
    limit: u16,
}

const fn rebuild_limit() -> u16 {
    16
}

async fn retrieval_progress(State(api): State<Api>) -> Response {
    retrieval_response(move || {
        api.retrieval.map_or_else(
            || Ok(Progress::not_configured()),
            |retrieval| retrieval.progress(),
        )
    })
    .await
}

async fn rebuild(
    State(api): State<Api>,
    input: Result<Query<Rebuild>, QueryRejection>,
) -> Response {
    let Ok(Query(Rebuild {
        limit: limit @ 1..=32,
    })) = input
    else {
        return problem(
            StatusCode::BAD_REQUEST,
            "invalid_rebuild_limit",
            "Supply a rebuild batch limit from 1 to 32.".into(),
            Some("limit"),
        );
    };
    let span = api
        .telemetry
        .as_ref()
        .map(|observer| observer.start(Stage::EmbeddingRebuild, None));
    let response = retrieval_response(move || {
        api.retrieval
            .ok_or(RetrievalError::NotConfigured)?
            .rebuild(limit)
    })
    .await;
    traced(response, span)
}

pub(super) async fn retrieval_response<T: Serialize + Send + 'static>(
    operation: impl FnOnce() -> Result<T, RetrievalError> + Send + 'static,
) -> Response {
    match tokio::task::spawn_blocking(operation).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => {
            let code = match error {
                RetrievalError::NotConfigured => "embedding_not_configured",
                RetrievalError::Runtime => "embedding_runtime_unavailable",
                RetrievalError::Incomplete => "embedding_index_incomplete",
                RetrievalError::Busy => "embedding_busy",
                RetrievalError::Storage => "embedding_storage_unavailable",
                RetrievalError::Vector => "embedding_vector_invalid",
                RetrievalError::PassageTooLarge => "embedding_passage_too_large",
            };
            problem(
                StatusCode::SERVICE_UNAVAILABLE,
                code,
                error.to_string(),
                None,
            )
        }
        Err(_) => failure(Failure::Storage),
    }
}
