use axum::Router;
use axum::extract::{Query, State, rejection::QueryRejection};
use axum::response::Response;
use axum::routing::get;
use serde::Deserialize;

use super::{
    Api,
    response::{failure, read, traced},
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

async fn search(State(api): State<Api>, input: Result<Query<Search>, QueryRejection>) -> Response {
    let mut span = api
        .telemetry
        .as_ref()
        .map(|telemetry| telemetry.start(Stage::ApiQuery, None));
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
    if let Some(span) = span.as_mut() {
        span.event(match &input {
            Ok((_, Mode::Bm25)) => Event::Bm25Selected,
            Ok((_, Mode::Hybrid)) => Event::HybridSelected,
            Ok((_, Mode::Semantic)) => Event::SemanticSelected,
            Err(_) => Event::SearchRejected,
        });
    }
    let (query, mode) = match input {
        Ok(value) => value,
        Err(error) => return traced(failure(error), span),
    };
    let telemetry = api.telemetry.clone();
    let link = span.as_ref().map(|span| span.link());
    if mode != Mode::Bm25 {
        let vector_span = telemetry
            .as_ref()
            .map(|observer| observer.start(Stage::VectorRetrieval, link));
        let result = retrieval_response(move || {
            api.retrieval
                .ok_or(RetrievalError::NotConfigured)?
                .search(&query, mode)
        })
        .await;
        return traced(traced(result, vector_span), span);
    }
    let response = read(api, move |store| {
        let lookup = telemetry
            .as_ref()
            .map(|observer| observer.start(Stage::LexicalLookup, link));
        let result = store.search(&query);
        if let Some(lookup) = lookup {
            lookup.finish(result.is_ok());
        }
        result
    })
    .await;
    traced(response, span)
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
