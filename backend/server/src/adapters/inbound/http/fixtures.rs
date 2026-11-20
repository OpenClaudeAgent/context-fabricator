use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;

use super::{Api, response::traced};
use crate::application::telemetry::Stage;

pub(super) fn routes() -> Router<Api> {
    Router::new().route("/v1/fixtures/import", post(fixture_import))
}

async fn fixture_import(State(api): State<Api>) -> Response {
    let span = api
        .telemetry
        .as_ref()
        .map(|telemetry| telemetry.start(Stage::ApiImport, None));
    let accepted = api
        .fixture
        .is_some_and(|fixture| fixture.request_traced(span.as_ref().map(|span| span.link())));
    let status = if accepted {
        StatusCode::ACCEPTED
    } else {
        StatusCode::CONFLICT
    };
    traced(status.into_response(), span)
}
