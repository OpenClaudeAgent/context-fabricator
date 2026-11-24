use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Extension, Router};

use super::{Api, request_trace::RequestTrace};

pub(super) fn routes() -> Router<Api> {
    Router::new().route("/v1/fixtures/import", post(fixture_import))
}

async fn fixture_import(
    State(api): State<Api>,
    Extension(request): Extension<RequestTrace>,
) -> Response {
    let accepted = api
        .fixture
        .is_some_and(|fixture| fixture.request_traced(request.link()));
    let status = if accepted {
        StatusCode::ACCEPTED
    } else {
        StatusCode::CONFLICT
    };
    status.into_response()
}
