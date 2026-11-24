use axum::extract::{
    Query, State,
    rejection::{JsonRejection, QueryRejection},
};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use super::{Api, response::problem};
use crate::domain::model::{ArtifactKind, discovery::DiscoveryError};

pub(super) fn routes() -> Router<Api> {
    Router::new()
        .route("/v1/model-catalog", get(search))
        .route("/v1/model-catalog/variants", get(variants))
        .route("/v1/model-catalog/downloads", post(download))
}

#[derive(Deserialize)]
struct Search {
    query: String,
    kind: Option<ArtifactKind>,
}

#[derive(Deserialize)]
struct RepositorySelection {
    repository: String,
    revision: String,
    kind: Option<ArtifactKind>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DownloadSelection {
    repository: String,
    revision: String,
    file: String,
    #[serde(default)]
    kind: ArtifactKind,
    sha256: Option<String>,
}

async fn search(State(api): State<Api>, input: Result<Query<Search>, QueryRejection>) -> Response {
    let Ok(Query(search)) = input else {
        return rejected(DiscoveryError::Invalid);
    };
    let Some(downloads) = api.downloads else {
        return rejected(DiscoveryError::Unavailable);
    };
    match downloads
        .search(&search.query, search.kind.unwrap_or_default())
        .await
    {
        Ok(mut results) => {
            // Older clients reject unknown response keys. Explicit mode opts into discovery details.
            if search.kind.is_none() {
                for result in &mut results {
                    result.kind = None;
                }
            }
            Json(results).into_response()
        }
        Err(error) => rejected(error),
    }
}

async fn variants(
    State(api): State<Api>,
    input: Result<Query<RepositorySelection>, QueryRejection>,
) -> Response {
    let Ok(Query(selection)) = input else {
        return rejected(DiscoveryError::Invalid);
    };
    let Some(downloads) = api.downloads else {
        return rejected(DiscoveryError::Unavailable);
    };
    match downloads
        .variants(
            &selection.repository,
            &selection.revision,
            selection.kind.unwrap_or_default(),
        )
        .await
    {
        Ok(mut files) => {
            if selection.kind.is_none() {
                for file in &mut files {
                    file.sha256 = None;
                }
            }
            Json(files).into_response()
        }
        Err(error) => rejected(error),
    }
}

async fn download(
    State(api): State<Api>,
    input: Result<Json<DownloadSelection>, JsonRejection>,
) -> Response {
    let Ok(Json(selection)) = input else {
        return rejected(DiscoveryError::Invalid);
    };
    let Some(downloads) = api.downloads else {
        return rejected(DiscoveryError::Unavailable);
    };
    match downloads
        .discover_and_request(
            &selection.repository,
            &selection.revision,
            &selection.file,
            selection.kind,
            selection.sha256.as_deref(),
        )
        .await
    {
        Ok(profile) => (StatusCode::ACCEPTED, Json(json!({ "profile": profile }))).into_response(),
        Err(error) => rejected(error),
    }
}

fn rejected(error: DiscoveryError) -> Response {
    let status = match error {
        DiscoveryError::Invalid => StatusCode::BAD_REQUEST,
        DiscoveryError::Unsupported => StatusCode::NOT_FOUND,
        DiscoveryError::StaleApproval => StatusCode::CONFLICT,
        DiscoveryError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        DiscoveryError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    problem(status, error.code(), error.to_string(), None)
}
