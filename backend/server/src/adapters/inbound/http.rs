use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use subtle::ConstantTimeEq;

use crate::application::{
    answers::Answers, github::GitHub, ports::Repository, retrieval::Retrieval, telemetry::Telemetry,
};
use crate::runtime::{model_downloads::ModelDownloads, scheduler::Refresh};

mod answers;
mod fixtures;
mod github;
mod model_downloads;
mod response;
mod retrieval;
mod search;
mod telemetry;
mod workspace;

use response::problem;

#[derive(Clone)]
struct Api {
    repository: Arc<dyn Repository>,
    authorization: Arc<str>,
    telemetry: Option<Arc<dyn Telemetry>>,
    fixture: Option<Refresh>,
    retrieval: Option<Arc<Retrieval>>,
    answers: Option<Arc<Answers>>,
    downloads: Option<Arc<ModelDownloads>>,
    github: Option<Arc<GitHub>>,
}

pub fn router(repository: Arc<dyn Repository>, token: &str) -> Router {
    router_with_services(repository, token, Services::default())
}

#[derive(Default)]
pub struct Services {
    pub telemetry: Option<Arc<dyn Telemetry>>,
    pub fixture: Option<Refresh>,
    pub retrieval: Option<Arc<Retrieval>>,
    pub answers: Option<Arc<Answers>>,
    pub downloads: Option<Arc<ModelDownloads>>,
    pub github: Option<Arc<GitHub>>,
}

pub fn router_with_services(
    repository: Arc<dyn Repository>,
    token: &str,
    services: Services,
) -> Router {
    let state = Api {
        repository,
        authorization: format!("Bearer {token}").into(),
        telemetry: services.telemetry,
        fixture: services.fixture,
        retrieval: services.retrieval,
        answers: services.answers,
        downloads: services.downloads,
        github: services.github,
    };
    let router = Router::new()
        .merge(answers::routes())
        .merge(github::routes())
        .merge(model_downloads::routes())
        .merge(retrieval::routes())
        .merge(search::routes())
        .merge(telemetry::routes())
        .merge(workspace::routes());
    let router = if state.fixture.is_some() {
        router.merge(fixtures::routes())
    } else {
        router
    };
    router
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .layer(DefaultBodyLimit::max(4096))
        .with_state(state)
}

async fn authenticate(State(api): State<Api>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let authenticated = !headers.contains_key(header::ORIGIN)
        && headers.get(header::AUTHORIZATION).is_some_and(|header| {
            bool::from(header.as_bytes().ct_eq(api.authorization.as_bytes()))
        });
    if !authenticated {
        return problem(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "A local session credential is required.".to_owned(),
            None,
        );
    }
    next.run(request).await
}
