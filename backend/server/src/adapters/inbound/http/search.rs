use axum::extract::{Query, State, rejection::QueryRejection};
use axum::response::Response;
use axum::routing::get;
use axum::{Extension, Router};
use serde::Deserialize;

use super::{
    Api,
    request_trace::RequestTrace,
    response::{failure, read},
    retrieval::retrieval_response,
};
use crate::application::{
    Failure, SearchQuery,
    retrieval::{Mode, RetrievalError},
    telemetry::{Event, Stage},
};

pub(super) fn routes() -> Router<Api> {
    Router::new().route("/v1/search", get(search))
}

async fn search(
    State(api): State<Api>,
    Extension(request): Extension<RequestTrace>,
    input: Result<Query<Search>, QueryRejection>,
) -> Response {
    let input = input.map_err(|_| Failure::InvalidQuery).and_then(
        |Query(Search {
             query,
             mode,
             repository_id,
             branch,
         })| {
            SearchQuery::parse_scoped(&query, repository_id, branch).map(|query| (query, mode))
        },
    );
    request.event(match &input {
        Ok((_, Mode::Bm25)) => Event::Bm25Selected,
        Ok((_, Mode::Hybrid)) => Event::HybridSelected,
        Ok((_, Mode::Semantic)) => Event::SemanticSelected,
        Err(_) => Event::SearchRejected,
    });
    let (query, mode) = match input {
        Ok(value) => value,
        Err(error) => return failure(error),
    };
    let telemetry = api.telemetry.clone();
    let link = request.link();
    if mode != Mode::Bm25 {
        let mut vector_span = telemetry
            .as_ref()
            .map(|observer| observer.start(Stage::VectorRetrieval, link));
        return retrieval_response(move || {
            let ranked = api
                .retrieval
                .ok_or(RetrievalError::NotConfigured)
                .and_then(|retrieval| {
                    retrieval.search_ranked(&query, mode, vector_span.is_some(), false, || None)
                });
            if let Some(mut span) = vector_span.take() {
                if let Ok(result) = &ranked
                    && let Some(ranking) = &result.fusion
                {
                    span.fusion_ranking(ranking);
                }
                span.finish(ranked.is_ok());
            }
            ranked.map(|result| result.original)
        })
        .await;
    }
    read(api, move |store| {
        let lookup = telemetry
            .as_ref()
            .map(|observer| observer.start(Stage::LexicalLookup, link));
        let result = store.search(&query);
        if let Some(lookup) = lookup {
            lookup.finish(result.is_ok());
        }
        result
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Search {
    query: String,
    #[serde(default)]
    mode: Mode,
    repository_id: Option<i64>,
    branch: Option<String>,
}
