use axum::extract::{
    Query, State,
    rejection::{JsonRejection, QueryRejection},
};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::{
    Api,
    response::{failure, problem},
};
use crate::application::{
    Failure,
    inference_setup::{EmbeddingSelectionError, embedding_models},
    retrieval::{Progress, RetrievalError},
};

pub(super) fn routes() -> Router<Api> {
    Router::new()
        .route("/v1/retrieval", get(retrieval_progress))
        .route(
            "/v1/retrieval/index",
            get(index_state).put(set_index_state).delete(delete_index),
        )
        .route("/v1/retrieval/rebuild", post(rebuild))
        .route(
            "/v1/retrieval/models",
            get(installed_models).put(select_model),
        )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelSelection {
    profile: String,
}

async fn installed_models(State(api): State<Api>) -> Response {
    let (Some(downloads), Some(retrieval)) = (api.downloads, api.retrieval) else {
        return model_rejection(&EmbeddingSelectionError::Unavailable);
    };
    match embedding_models(&downloads, &retrieval).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => model_rejection(&error),
    }
}

async fn select_model(
    State(api): State<Api>,
    input: Result<Json<ModelSelection>, JsonRejection>,
) -> Response {
    let update = match input {
        Ok(Json(update))
            if (1..=128).contains(&update.profile.len())
                && update.profile.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                }) =>
        {
            update
        }
        _ => {
            return problem(
                StatusCode::BAD_REQUEST,
                "invalid_embedding_profile",
                "Supply a valid embedding model profile.".into(),
                Some("profile"),
            );
        }
    };
    let Some(setup) = api.inference_setup else {
        return model_rejection(&EmbeddingSelectionError::Unavailable);
    };
    match setup.select_embedding(update.profile).await {
        Ok(value) => Json(value).into_response(),
        Err(error) => model_rejection(&error),
    }
}

fn model_rejection(error: &EmbeddingSelectionError) -> Response {
    let (status, code) = match error {
        EmbeddingSelectionError::NotFound => {
            (StatusCode::NOT_FOUND, "embedding_model_not_installed")
        }
        EmbeddingSelectionError::Conflict => (StatusCode::CONFLICT, "embedding_selection_conflict"),
        EmbeddingSelectionError::Readiness => {
            (StatusCode::SERVICE_UNAVAILABLE, "embedding_model_unready")
        }
        EmbeddingSelectionError::Unavailable => (
            StatusCode::SERVICE_UNAVAILABLE,
            "embedding_selection_unavailable",
        ),
    };
    problem(status, code, error.to_string(), None)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexUpdate {
    enabled: bool,
}

async fn index_state(State(api): State<Api>) -> Response {
    retrieval_response(move || {
        api.retrieval
            .ok_or(RetrievalError::NotConfigured)?
            .index_state()
    })
    .await
}
async fn set_index_state(
    State(api): State<Api>,
    body: Result<Json<IndexUpdate>, JsonRejection>,
) -> Response {
    let Ok(Json(update)) = body else {
        return problem(
            StatusCode::BAD_REQUEST,
            "invalid_index_state",
            "Supply an enabled boolean.".into(),
            Some("enabled"),
        );
    };
    retrieval_response(move || {
        api.retrieval
            .ok_or(RetrievalError::NotConfigured)?
            .set_index_enabled(update.enabled)
    })
    .await
}
async fn delete_index(State(api): State<Api>) -> Response {
    retrieval_response(move || {
        api.retrieval
            .ok_or(RetrievalError::NotConfigured)?
            .delete_index()
    })
    .await
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
    retrieval_response(move || {
        api.retrieval
            .ok_or(RetrievalError::NotConfigured)?
            .rebuild(limit)
    })
    .await
}

pub(super) async fn retrieval_response<T: Serialize + Send + 'static>(
    operation: impl FnOnce() -> Result<T, RetrievalError> + Send + 'static,
) -> Response {
    match tokio::task::spawn_blocking(operation).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(error)) => retrieval_rejection(error),
        Err(_) => failure(Failure::Storage),
    }
}

pub(super) fn retrieval_rejection(error: RetrievalError) -> Response {
    let code = match error {
        RetrievalError::NotConfigured => "embedding_not_configured",
        RetrievalError::Runtime => "embedding_runtime_unavailable",
        RetrievalError::Disabled => "embedding_index_disabled",
        RetrievalError::Incomplete => "embedding_index_incomplete",
        RetrievalError::Busy => "embedding_busy",
        RetrievalError::Storage => "embedding_storage_unavailable",
        RetrievalError::StorageLimit => "github_storage_limit",
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
