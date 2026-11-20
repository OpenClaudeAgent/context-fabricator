use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::application::telemetry::Stage;
use crate::domain::github::{Error, Selection};

use super::{
    Api,
    response::{problem, traced},
};

pub(super) fn routes() -> Router<Api> {
    Router::new()
        .route("/v1/github", get(snapshot))
        .route("/v1/github/rate-limit", put(set_rate))
        .route("/v1/github/credential", put(authorize).delete(disconnect))
        .route("/v1/github/repositories", get(discover).put(save))
        .route("/v1/github/repositories/{id}", delete(remove))
        .route("/v1/github/repositories/{id}/refresh", post(refresh))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Authorization {
    revision: u32,
    token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RateLimit {
    revision: u32,
    requests_per_minute: u16,
}

#[derive(Deserialize)]
struct Revision {
    revision: u32,
}

#[derive(Deserialize)]
struct Page {
    page: Option<u16>,
}

async fn snapshot(State(api): State<Api>) -> Response {
    reply(
        api.github
            .ok_or(Error::Storage)
            .and_then(|github| github.snapshot()),
    )
}

async fn set_rate(
    State(api): State<Api>,
    input: Result<Json<RateLimit>, JsonRejection>,
) -> Response {
    let Ok(Json(input)) = input else {
        return rejected(Error::InvalidRate);
    };
    reply(
        api.github
            .ok_or(Error::Storage)
            .and_then(|github| github.set_rate(input.revision, input.requests_per_minute)),
    )
}

async fn authorize(
    State(api): State<Api>,
    input: Result<Json<Authorization>, JsonRejection>,
) -> Response {
    let Ok(Json(input)) = input else {
        return rejected(Error::InvalidToken);
    };
    let Some(github) = api.github else {
        return rejected(Error::Storage);
    };
    reply(github.authorize(input.revision, &input.token).await)
}

async fn disconnect(
    State(api): State<Api>,
    query: Result<Query<Revision>, QueryRejection>,
) -> Response {
    let Ok(Query(input)) = query else {
        return rejected(Error::InvalidSelection);
    };
    reply(
        api.github
            .ok_or(Error::Storage)
            .and_then(|github| github.disconnect(input.revision)),
    )
}

async fn discover(State(api): State<Api>, query: Result<Query<Page>, QueryRejection>) -> Response {
    let Ok(Query(input)) = query else {
        return rejected(Error::InvalidSelection);
    };
    let Some(github) = api.github else {
        return rejected(Error::Storage);
    };
    reply(github.discover(input.page.unwrap_or(1)).await)
}

async fn save(State(api): State<Api>, input: Result<Json<Selection>, JsonRejection>) -> Response {
    let Ok(Json(input)) = input else {
        return rejected(Error::InvalidSelection);
    };
    let Some(github) = api.github else {
        return rejected(Error::Storage);
    };
    reply(github.save(&input).await)
}

async fn remove(
    State(api): State<Api>,
    id: Result<Path<i64>, PathRejection>,
    query: Result<Query<Revision>, QueryRejection>,
) -> Response {
    let (Ok(Path(id)), Ok(Query(input))) = (id, query) else {
        return rejected(Error::InvalidSelection);
    };
    reply(
        api.github
            .ok_or(Error::Storage)
            .and_then(|github| github.remove(input.revision, id)),
    )
}

#[derive(Deserialize)]
struct RefreshRequest {
    #[serde(default)]
    background: bool,
}

async fn refresh(
    State(api): State<Api>,
    id: Result<Path<i64>, PathRejection>,
    input: Result<Query<RefreshRequest>, QueryRejection>,
) -> Response {
    let (Ok(Path(id)), Ok(Query(input))) = (id, input) else {
        return rejected(Error::InvalidSelection);
    };
    let Some(github) = api.github else {
        return rejected(Error::Storage);
    };
    if input.background {
        let mut response = reply(github.queue_current_code(id));
        if response.status().is_success() {
            *response.status_mut() = StatusCode::ACCEPTED;
        }
        return response;
    }
    let span = api
        .telemetry
        .as_ref()
        .map(|telemetry| telemetry.start(Stage::GitHubCurrentCode, None));
    traced(reply(github.refresh_current_code(id).await), span)
}

fn reply<T: Serialize>(result: Result<T, Error>) -> Response {
    match result {
        Ok(value) => ([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response(),
        Err(error) => rejected(error),
    }
}

fn rejected(error: Error) -> Response {
    let (status, code) = match error {
        Error::InvalidRate => (StatusCode::BAD_REQUEST, "github_invalid_rate"),
        Error::InvalidSelection => (StatusCode::BAD_REQUEST, "github_invalid_selection"),
        Error::InvalidToken => (StatusCode::BAD_REQUEST, "github_invalid_token"),
        Error::Access => (StatusCode::FORBIDDEN, "github_access_not_verified"),
        Error::Reconnect => (StatusCode::CONFLICT, "github_reconnect"),
        Error::Conflict => (StatusCode::CONFLICT, "github_revision_conflict"),
        Error::ExpiredPin => (StatusCode::CONFLICT, "github_expired_pin"),
        Error::Throttled => (StatusCode::TOO_MANY_REQUESTS, "github_throttled"),
        Error::Upstream => (StatusCode::SERVICE_UNAVAILABLE, "github_unavailable"),
        Error::CollectionLimit => (StatusCode::UNPROCESSABLE_ENTITY, "github_collection_limit"),
        Error::Storage => (
            StatusCode::SERVICE_UNAVAILABLE,
            "github_storage_unavailable",
        ),
        Error::Credentials => (
            StatusCode::SERVICE_UNAVAILABLE,
            "github_keychain_unavailable",
        ),
    };
    problem(status, code, error.to_string(), None)
}
