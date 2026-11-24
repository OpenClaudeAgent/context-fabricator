use std::sync::Arc;

use axum::Router;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use subtle::ConstantTimeEq;

use crate::application::artifact_storage::StorageReport;
use crate::application::inference_setup::InferenceSetup;
use crate::application::runtime_setup::RuntimeSetup;
use crate::application::{
    answer_history::Journal, answers::Answers, github::GitHub, model_management::ModelManagement,
    ports::Repository, retrieval::Retrieval, telemetry::Telemetry,
};
use crate::runtime::scheduler::Refresh;

mod answer_history;
mod answers;
mod fixtures;
mod github;
mod inference_setup;
mod model_catalog;
mod model_downloads;
mod request_trace;
mod response;
mod retrieval;
mod runtime_setup;
mod search;
mod storage;
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
    answer_history: Option<Arc<dyn Journal>>,
    downloads: Option<Arc<ModelManagement>>,
    runtime_setup: Option<Arc<RuntimeSetup>>,
    artifact_storage: Option<Arc<dyn StorageReport>>,
    inference_setup: Option<Arc<InferenceSetup>>,
    github: Option<Arc<GitHub>>,
    benchmark_actions: Option<std::path::PathBuf>,
    benchmark_corpus: answers::benchmark::Shared,
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
    pub answer_history: Option<Arc<dyn Journal>>,
    pub downloads: Option<Arc<ModelManagement>>,
    pub runtime_setup: Option<Arc<RuntimeSetup>>,
    pub artifact_storage: Option<Arc<dyn StorageReport>>,
    pub inference_setup: Option<Arc<InferenceSetup>>,
    pub github: Option<Arc<GitHub>>,
    pub benchmark_directory: Option<std::path::PathBuf>,
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
        answer_history: services.answer_history,
        downloads: services.downloads,
        runtime_setup: services.runtime_setup,
        artifact_storage: services.artifact_storage,
        inference_setup: services.inference_setup,
        github: services.github,
        benchmark_actions: services
            .benchmark_directory
            .as_ref()
            .map(|root| root.join("actions")),
        benchmark_corpus: answers::benchmark::Cache::shared(services.benchmark_directory),
    };
    let router = Router::new()
        .merge(answer_history::routes())
        .merge(answers::routes())
        .merge(github::routes())
        .merge(inference_setup::routes())
        .merge(model_downloads::routes())
        .merge(model_catalog::routes())
        .merge(retrieval::routes())
        .merge(runtime_setup::routes())
        .merge(search::routes())
        .merge(storage::routes())
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
        .layer(middleware::from_fn_with_state(
            state.clone(),
            request_trace::observe,
        ))
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
