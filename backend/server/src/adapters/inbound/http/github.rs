use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};

use crate::application::telemetry::{Observer, TraceContext, TraceLink, TraceOrigin};
use crate::domain::github::{Error, Selection};

use super::{
    Api,
    request_trace::{RequestTrace, WorkTrace},
    response::problem,
};

pub(super) fn routes() -> Router<Api> {
    Router::new()
        .route("/v1/github", get(snapshot))
        .route("/v1/github/rate-limit", put(set_rate))
        .route(
            "/v1/github/storage",
            get(storage_budget).put(set_storage_budget),
        )
        .route("/v1/github/credential", put(authorize).delete(disconnect))
        .route("/v1/github/repositories", get(discover).put(save))
        .route("/v1/github/repositories/{id}", delete(remove))
        .route("/v1/github/repositories/{id}/access", post(check_access))
        .route("/v1/github/repositories/{id}/branches", get(branches))
        .route(
            "/v1/github/repositories/{id}/data",
            get(data_state).delete(purge_data),
        )
        .route("/v1/github/repositories/{id}/refresh", post(refresh))
        .route("/v1/github/repositories/{id}/graph", get(graph))
        .route(
            "/v1/github/repositories/{id}/actions",
            get(actions).post(refresh_actions),
        )
        .route(
            "/v1/github/repositories/{id}/history/comparisons/{child}",
            get(comparison).post(materialize_comparison),
        )
        .route(
            "/v1/github/repositories/{id}/history/evidence/{child}",
            get(history_evidence),
        )
        .route(
            "/v1/github/repositories/{id}/history",
            get(history).post(refresh_history),
        )
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StorageRequest {
    revision: u32,
    limit_gb: u16,
}

async fn storage_budget(State(api): State<Api>) -> Response {
    reply(
        api.github
            .ok_or(Error::Storage)
            .and_then(|github| github.storage_budget()),
    )
}

async fn set_storage_budget(
    State(api): State<Api>,
    input: Result<Json<StorageRequest>, JsonRejection>,
) -> Response {
    let Ok(Json(input)) = input else {
        return rejected(Error::InvalidStorageLimit);
    };
    reply(
        api.github
            .ok_or(Error::Storage)
            .and_then(|github| github.set_storage_budget(input.revision, input.limit_gb)),
    )
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

async fn check_access(
    State(api): State<Api>,
    id: Result<Path<i64>, PathRejection>,
    query: Result<Query<Revision>, QueryRejection>,
) -> Response {
    let (Ok(Path(id)), Ok(Query(input))) = (id, query) else {
        return rejected(Error::InvalidSelection);
    };
    let Some(github) = api.github else {
        return rejected(Error::Storage);
    };
    reply(github.check_access(input.revision, id).await)
}

async fn data_state(State(api): State<Api>, id: Result<Path<i64>, PathRejection>) -> Response {
    let Ok(Path(id)) = id else {
        return rejected(Error::InvalidSelection);
    };
    reply(
        api.github
            .ok_or(Error::Storage)
            .and_then(|github| github.data_state(id)),
    )
}

async fn purge_data(State(api): State<Api>, id: Result<Path<i64>, PathRejection>) -> Response {
    let Ok(Path(id)) = id else {
        return rejected(Error::InvalidSelection);
    };
    reply(
        api.github
            .ok_or(Error::Storage)
            .and_then(|github| github.purge_data(id)),
    )
}

#[derive(Deserialize)]
struct RefreshRequest {
    #[serde(default)]
    background: bool,
}

async fn refresh(
    State(api): State<Api>,
    Extension(request): Extension<RequestTrace>,
    id: Result<Path<i64>, PathRejection>,
    input: Result<Query<RefreshRequest>, QueryRejection>,
) -> Response {
    let (Ok(Path(id)), Ok(Query(input))) = (id, input) else {
        return rejected(Error::InvalidSelection);
    };
    let Some(github) = api.github else {
        return rejected(Error::Storage);
    };
    let trigger = request.link();
    if input.background {
        return match github.queue_current_code(id, trigger) {
            Ok((state, origin)) => {
                with_work_trace((StatusCode::ACCEPTED, Json(state)).into_response(), origin)
            }
            Err(error) => rejected(error),
        };
    }
    let observer = api
        .telemetry
        .as_deref()
        .map(|telemetry| telemetry as &dyn Observer);
    match github.refresh_current_code(id, observer, trigger).await {
        Ok(refresh) => with_work_trace(reply(refresh.result), refresh.origin),
        Err(error) => rejected(error),
    }
}

fn with_work_trace(mut response: Response, origin: Option<TraceLink>) -> Response {
    if let Some(link) = origin {
        let id = uuid::Uuid::from_bytes(link.trace).simple().to_string();
        if let Ok(value) = HeaderValue::from_str(&id) {
            response
                .headers_mut()
                .insert("x-context-work-trace-id", value);
        }
        response.extensions_mut().insert(WorkTrace(link));
    }
    response
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BranchPage {
    revision: u32,
    page: Option<u16>,
}

async fn branches(
    State(api): State<Api>,
    Extension(request): Extension<RequestTrace>,
    id: Result<Path<i64>, PathRejection>,
    query: Result<Query<BranchPage>, QueryRejection>,
) -> Response {
    let (Ok(Path(id)), Ok(Query(query))) = (id, query) else {
        return rejected(Error::InvalidSelection);
    };
    let Some(github) = &api.github else {
        return rejected(Error::Storage);
    };
    reply(
        github
            .branches(
                id,
                query.revision,
                query.page.unwrap_or(1),
                metadata_trace(&api, &request),
            )
            .await,
    )
}

async fn actions(State(api): State<Api>, id: Result<Path<i64>, PathRejection>) -> Response {
    let Ok(Path(id)) = id else {
        return rejected(Error::InvalidSelection);
    };
    reply(
        api.github
            .ok_or(Error::Storage)
            .and_then(|github| github.actions_state(id)),
    )
}

async fn refresh_actions(
    State(api): State<Api>,
    Extension(request): Extension<RequestTrace>,
    id: Result<Path<i64>, PathRejection>,
) -> Response {
    let Ok(Path(id)) = id else {
        return rejected(Error::InvalidSelection);
    };
    let trace = metadata_trace(&api, &request);
    let Some(github) = &api.github else {
        return rejected(Error::Storage);
    };
    if let Err(error) = github.resume_data(id, "actions") {
        return rejected(error);
    }
    reply(
        github
            .refresh_actions(id, trace.map(TraceOrigin::from))
            .await,
    )
}

async fn history(State(api): State<Api>, id: Result<Path<i64>, PathRejection>) -> Response {
    let Ok(Path(id)) = id else {
        return rejected(Error::InvalidSelection);
    };
    reply(
        api.github
            .ok_or(Error::Storage)
            .and_then(|github| github.history_state(id)),
    )
}

async fn graph(
    State(api): State<Api>,
    id: Result<Path<i64>, PathRejection>,
    input: Result<Query<crate::application::github::graph::Query>, QueryRejection>,
) -> Response {
    let (Ok(Path(id)), Ok(Query(input))) = (id, input) else {
        return rejected(Error::InvalidSelection);
    };
    let Some(github) = api.github else {
        return rejected(Error::Storage);
    };
    reply(github.graph(id, &input))
}

async fn comparison(
    State(api): State<Api>,
    Extension(request): Extension<RequestTrace>,
    input: Result<Path<(i64, String)>, PathRejection>,
) -> Response {
    let Ok(Path((id, child))) = input else {
        return rejected(Error::InvalidSelection);
    };
    let trace = metadata_trace(&api, &request);
    let Some(github) = &api.github else {
        return rejected(Error::Storage);
    };
    reply(github.history_comparison(id, &child, trace).await)
}

async fn materialize_comparison(
    State(api): State<Api>,
    Extension(request): Extension<RequestTrace>,
    input: Result<Path<(i64, String)>, PathRejection>,
) -> Response {
    let Ok(Path((id, child))) = input else {
        return rejected(Error::InvalidSelection);
    };
    let trace = metadata_trace(&api, &request);
    let Some(github) = &api.github else {
        return rejected(Error::Storage);
    };
    reply(github.materialize_history(id, &child, trace).await)
}

async fn history_evidence(
    State(api): State<Api>,
    input: Result<Path<(i64, String)>, PathRejection>,
) -> Response {
    let Ok(Path((id, child))) = input else {
        return rejected(Error::InvalidSelection);
    };
    reply(
        api.github
            .ok_or(Error::Storage)
            .and_then(|github| github.history_evidence(id, &child)),
    )
}

async fn refresh_history(
    State(api): State<Api>,
    Extension(request): Extension<RequestTrace>,
    id: Result<Path<i64>, PathRejection>,
) -> Response {
    let Ok(Path(id)) = id else {
        return rejected(Error::InvalidSelection);
    };
    let trace = metadata_trace(&api, &request);
    let Some(github) = &api.github else {
        return rejected(Error::Storage);
    };
    if let Err(error) = github.resume_data(id, "history") {
        return rejected(error);
    }
    match github
        .refresh_history(id, trace.map(TraceOrigin::from))
        .await
    {
        Ok(refresh) => with_work_trace(reply(Ok(refresh.snapshot)), refresh.origin),
        Err(error) => rejected(error),
    }
}

fn metadata_trace<'a>(api: &'a Api, request: &RequestTrace) -> Option<TraceContext<'a>> {
    Some(TraceContext {
        observer: api.telemetry.as_deref()?,
        parent: request.link()?,
    })
}

fn reply<T: Serialize>(result: Result<T, Error>) -> Response {
    match result {
        Ok(value) => ([(header::CACHE_CONTROL, "no-store")], Json(value)).into_response(),
        Err(error) => rejected(error),
    }
}

fn rejected(error: Error) -> Response {
    let (status, code) = match error {
        Error::InvalidStorageLimit => (StatusCode::BAD_REQUEST, "github_invalid_storage_limit"),
        Error::StorageLimit => (StatusCode::UNPROCESSABLE_ENTITY, "github_storage_limit"),
        Error::InvalidRate => (StatusCode::BAD_REQUEST, "github_invalid_rate"),
        Error::InvalidInterval => (StatusCode::BAD_REQUEST, "github_invalid_interval"),
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
    let field = (error == Error::InvalidInterval).then_some("intervalMinutes");
    problem(status, code, error.to_string(), field)
}
