use std::{fmt::Write as _, path::Path, sync::Arc};

use axum::{
    Extension, Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    AnswerRequest, Api, Event, Observer, RequestTrace, SearchQuery, failure, generation_missing,
    present_answer, problem,
};
use crate::{
    adapters::outbound::sqlite::SqliteStore,
    application::{
        Failure,
        ports::SnapshotStore,
        retrieval::{Retrieval, RetrievalError},
    },
    domain::{
        corpus::snapshot::{File, Plan, SnapshotError},
        github::{Error as GitHubError, RepositoryName},
    },
};

pub(super) mod action_downloads;
pub(super) mod actions;
mod cache;
mod reference;

use super::super::retrieval::{retrieval_rejection, retrieval_response};
pub(in crate::adapters::inbound::http) use cache::{Cache, Shared};
use reference::{Loaded, Reference};

pub(super) const MAX_REQUEST_BYTES: usize = 3 * 1024 * 1024;
const MAX_FILES: usize = 64;
const MAX_ACTIVE_CORPORA: usize = 12;
const MAX_TOTAL_TEXT_BYTES: usize = 512 * 1024;
const MAX_FILE_BYTES: usize = 256 * 1024;
const COLLECTION: &str = "benchmark";
const REF: &str = "refs/heads/benchmark";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Input {
    query: String,
    request_id: Option<Uuid>,
    documents: Option<Vec<Document>>,
    reference: Option<Reference>,
    action: Option<actions::Key>,
    #[serde(default)]
    retrieval: SearchMode,
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum SearchMode {
    #[default]
    Bm25,
    Hybrid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    path: String,
    text: String,
}

pub(super) async fn load(
    State(api): State<Api>,
    input: Result<Json<Reference>, JsonRejection>,
) -> Response {
    let Ok(Json(reference)) = input else {
        return invalid_reference();
    };
    let Some(name) = reference.name() else {
        return invalid_reference();
    };
    let mut cache = api.benchmark_corpus.clone().lock_owned().await;
    match cache.get(&reference, false) {
        Ok(Some(loaded)) => return Json(loaded.info.clone()).into_response(),
        Ok(None) => {}
        Err(error) => return load_failed(error),
    }
    let entries = match cache.list() {
        Ok(entries) => entries,
        Err(error) => return load_failed(error),
    };
    if entries.iter().filter(|entry| entry.active).count() >= MAX_ACTIVE_CORPORA {
        return problem(
            StatusCode::CONFLICT,
            "benchmark_corpus_capacity",
            format!(
                "{MAX_ACTIVE_CORPORA} test corpora are loaded. Deactivate a corpus before loading another."
            ),
            None,
        );
    }
    match cache.get(&reference, true) {
        Ok(Some(loaded)) => return Json(loaded.info.clone()).into_response(),
        Ok(None) => {}
        Err(error) => return load_failed(error),
    }
    let Some(github) = &api.github else {
        return load_failed(GitHubError::Storage);
    };
    let fetch = || {
        github.public_snapshot(
            RepositoryName {
                owner: name.owner,
                name: name.name,
            },
            &reference.revision,
        )
    };
    let mut fetched = fetch().await;
    if matches!(fetched, Err(GitHubError::Upstream)) {
        fetched = fetch().await;
    }
    let objects = match fetched {
        Ok(objects) => objects,
        Err(error) => return load_failed(error),
    };
    match tokio::task::spawn_blocking(move || cache.import(reference, objects.as_ref())).await {
        Ok(Ok(info)) => Json(info).into_response(),
        Ok(Err(error)) => load_failed(error),
        Err(_) => load_failed(GitHubError::Storage),
    }
}

pub(super) async fn list(State(api): State<Api>) -> Response {
    match api.benchmark_corpus.lock().await.list() {
        Ok(entries) => Json(entries).into_response(),
        Err(error) => load_failed(error),
    }
}

async fn active(api: &Api, reference: Reference) -> Result<Option<Loaded>, GitHubError> {
    let mut cache = api.benchmark_corpus.clone().lock_owned().await;
    tokio::task::spawn_blocking(move || cache.get(&reference, false).map(Option::<&Loaded>::cloned))
        .await
        .map_err(|_| GitHubError::Storage)?
}

pub(super) async fn unload(
    State(api): State<Api>,
    input: Result<Json<Reference>, JsonRejection>,
) -> Response {
    let Ok(Json(reference)) = input else {
        return invalid_reference();
    };
    if reference.name().is_none() {
        return invalid_reference();
    }
    match api.benchmark_corpus.lock().await.deactivate(&reference) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => load_failed(error),
    }
}

pub(super) async fn embed(
    State(api): State<Api>,
    input: Result<Json<Reference>, JsonRejection>,
) -> Response {
    let Ok(Json(reference)) = input else {
        return invalid_reference();
    };
    if reference.name().is_none() {
        return invalid_reference();
    }
    let loaded = match active(&api, reference).await {
        Ok(Some(loaded)) => loaded,
        Ok(None) => return corpus_not_loaded(),
        Err(error) => return load_failed(error),
    };
    let (store, admission) = (loaded.store, loaded.embedding_admission);
    retrieval_response(move || {
        let _batch = admission.try_lock().map_err(|_| RetrievalError::Busy)?;
        benchmark_retrieval(api.retrieval.as_deref(), &store)?.rebuild(8)
    })
    .await
}

fn benchmark_retrieval(
    retrieval: Option<&Retrieval>,
    store: &Arc<SqliteStore>,
) -> Result<Retrieval, RetrievalError> {
    retrieval
        .ok_or(RetrievalError::NotConfigured)?
        .for_corpus(store.clone(), store.clone())
}

pub(super) async fn ask(
    State(api): State<Api>,
    Extension(request): Extension<RequestTrace>,
    input: Result<Json<Input>, JsonRejection>,
) -> Response {
    let Ok(Json(input)) = input else {
        request.event(Event::AnswerFailed);
        return invalid_documents();
    };
    let query = match SearchQuery::parse(&input.query) {
        Ok(query) => query,
        Err(error) => {
            request.event(Event::AnswerFailed);
            return failure(error);
        }
    };
    let corpus = match (input.documents, input.reference, input.action) {
        (Some(mut documents), None, None) if input.retrieval == SearchMode::Bm25 => {
            if !valid(&mut documents) {
                request.event(Event::AnswerFailed);
                return invalid_documents();
            }
            Corpus::Documents(documents)
        }
        (None, Some(reference), None) if reference.name().is_some() => {
            match active(&api, reference).await {
                Ok(Some(loaded)) => Corpus::Loaded(loaded.store),
                Ok(None) => {
                    request.event(Event::AnswerFailed);
                    return corpus_not_loaded();
                }
                Err(error) => {
                    request.event(Event::AnswerFailed);
                    return load_failed(error);
                }
            }
        }
        (None, None, Some(key)) if input.retrieval == SearchMode::Bm25 => match if key.downloaded {
            action_downloads::find(api.benchmark_actions.as_deref(), &key)
        } else {
            actions::find(&key).map_err(|_| GitHubError::Storage)
        } {
            Ok(Some(action)) => Corpus::Action(Box::new(action)),
            Ok(None) => {
                request.event(Event::AnswerFailed);
                return problem(
                    StatusCode::BAD_REQUEST,
                    "benchmark_action_not_found",
                    "Choose a recorded benchmark workflow attempt.".into(),
                    Some("action"),
                );
            }
            Err(_) => return failure(Failure::Storage),
        },
        _ => {
            request.event(Event::AnswerFailed);
            return invalid_documents();
        }
    };
    let id = input.request_id;
    let trace = request.clone();
    let (response, outcome) = tokio::task::spawn_blocking(move || {
        execute(&api, &trace, &query, id, corpus, input.retrieval)
    })
    .await
    .unwrap_or_else(|_| (failure(Failure::Storage), Event::AnswerFailed));
    request.event(outcome);
    response
}

enum Corpus {
    Documents(Vec<Document>),
    Loaded(Arc<SqliteStore>),
    Action(Box<actions::Recorded>),
}

fn valid(documents: &mut [Document]) -> bool {
    if documents.is_empty() || documents.len() > MAX_FILES {
        return false;
    }
    documents.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    documents
        .windows(2)
        .all(|pair| pair[0].path != pair[1].path)
        && documents.iter().all(|d| {
            d.path.len() <= 1024
                && !d
                    .path
                    .split('/')
                    .any(|part| matches!(part, "" | "." | ".."))
                && !d.path.chars().any(char::is_control)
                && d.text.len() <= MAX_FILE_BYTES
                && !d.text.trim().is_empty()
                && !d.text.contains('\0')
        })
        && documents.iter().map(|d| d.text.len()).sum::<usize>() <= MAX_TOTAL_TEXT_BYTES
}

fn execute(
    api: &Api,
    trace: &RequestTrace,
    query: &SearchQuery,
    id: Option<Uuid>,
    corpus: Corpus,
    mode: SearchMode,
) -> (Response, Event) {
    let Some(answers) = &api.answers else {
        return generation_missing();
    };
    let metadata = AnswerRequest {
        id,
        observer: api
            .telemetry
            .as_deref()
            .map(|observer| observer as &dyn Observer),
        trace: trace.link(),
        include_model_context: true,
        inspect_pipeline: true,
        record_model: &|model| trace.model_profile(model),
    };
    let store = match corpus {
        Corpus::Action(action) => return present_answer(action.answer(answers, query, &metadata)),
        Corpus::Loaded(store) => store,
        Corpus::Documents(documents) => match build_index(&documents) {
            Ok(store) => Arc::new(store),
            Err(SnapshotError::Invalid) => return (invalid_documents(), Event::AnswerFailed),
            Err(_) => return (failure(Failure::Storage), Event::AnswerFailed),
        },
    };
    let retrieval = match mode {
        SearchMode::Bm25 => None,
        SearchMode::Hybrid => {
            match benchmark_retrieval(api.retrieval.as_deref(), &store).and_then(ready_for_hybrid) {
                Ok(retrieval) => Some(Arc::new(retrieval)),
                Err(error) => return (retrieval_rejection(error), Event::AnswerFailed),
            }
        }
    };
    let answers = answers.for_corpus(store.clone(), store, retrieval);
    present_answer(answers.ask(query, &metadata))
}

fn ready_for_hybrid(retrieval: Retrieval) -> Result<Retrieval, RetrievalError> {
    let progress = retrieval.progress()?;
    if progress.state != "ready" || progress.total == 0 {
        return Err(RetrievalError::Incomplete);
    }
    Ok(retrieval)
}

fn build_index(documents: &[Document]) -> Result<SqliteStore, SnapshotError> {
    let store = SqliteStore::open(Path::new(":memory:")).map_err(|_| SnapshotError::Storage)?;
    let hashes: Vec<_> = documents
        .iter()
        .map(|d| hex(&Sha256::digest(d.text.as_bytes())))
        .collect();
    let mut digest = Sha256::new();
    for (document, hash) in documents.iter().zip(&hashes) {
        digest.update(document.path.as_bytes());
        digest.update([0]);
        digest.update(hash.as_bytes());
    }
    let revision = hex(&digest.finalize());
    let snapshot = store.begin_snapshot(&Plan {
        collection: COLLECTION,
        generation: 0,
        predecessor: None,
        revision: &revision,
        tracked_ref: REF,
    })?;
    let identities: Vec<_> = documents
        .iter()
        .map(|document| format!("benchmark:{}", document.path))
        .collect();
    let files: Vec<_> = documents
        .iter()
        .zip(hashes.iter().zip(&identities))
        .map(|(document, (blob, source))| File {
            source,
            path: &document.path,
            blob,
            file_url: "https://benchmark.invalid/",
            text: Some(&document.text),
        })
        .collect();
    store.append_snapshot(snapshot.id, 0, None, &files)?;
    store.publish_snapshot(snapshot.id)?;
    Ok(store)
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn invalid_documents() -> Response {
    problem(
        StatusCode::BAD_REQUEST,
        "invalid_benchmark_corpus",
        "Use 1 to 64 UTF-8 documents with distinct relative paths, up to 256 KiB per file and 512 KiB in total."
            .into(),
        Some("documents"),
    )
}

fn invalid_reference() -> Response {
    problem(
        StatusCode::BAD_REQUEST,
        "invalid_benchmark_reference",
        "Choose a GitHub repository and a full commit ID.".into(),
        None,
    )
}

fn corpus_not_loaded() -> Response {
    problem(
        StatusCode::CONFLICT,
        "benchmark_corpus_not_loaded",
        "Load the reference corpus before running this question.".into(),
        None,
    )
}

fn load_failed(error: GitHubError) -> Response {
    let message = match error {
        GitHubError::CollectionLimit => {
            "This commit exceeds the benchmark corpus size or file limit.".into()
        }
        GitHubError::ExpiredPin | GitHubError::Access => {
            "GitHub did not provide access to this exact commit.".into()
        }
        GitHubError::Upstream => "GitHub could not deliver this commit. Retry the import.".into(),
        _ => error.to_string(),
    };
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "benchmark_corpus_unavailable",
        message,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctxfab_inference::{DIMENSIONS, Embedder, Embedding, EmbeddingError, Input};

    struct FixtureEmbedder;

    impl Embedder for FixtureEmbedder {
        fn embed(&self, _: Input<'_>) -> Result<Embedding, EmbeddingError> {
            let mut vector = vec![0.0; DIMENSIONS];
            vector[0] = 1.0;
            Embedding::new(vector)
        }
    }

    #[test]
    fn benchmark_hybrid_uses_an_isolated_vector_index() -> Result<(), Box<dyn std::error::Error>> {
        let make_corpus = |path: &str, text: &str| {
            build_index(&[Document {
                path: path.into(),
                text: text.into(),
            }])
            .map(Arc::new)
        };
        let main = make_corpus("connected.rs", "Connected repository cache implementation.")?;
        let test = make_corpus("pinned.rs", "Pinned repository cache implementation.")?;
        let primary = Retrieval::new(main.clone(), main, Arc::new(FixtureEmbedder), "fixture");
        let scratch = benchmark_retrieval(Some(&primary), &test)?;
        assert!(matches!(
            ready_for_hybrid(scratch),
            Err(RetrievalError::Incomplete)
        ));
        let rebuilt = benchmark_retrieval(Some(&primary), &test)?.rebuild(8)?;
        assert_eq!((rebuilt.indexed, rebuilt.total), (1, 1));
        let scratch = ready_for_hybrid(benchmark_retrieval(Some(&primary), &test)?)?;
        let query = SearchQuery::parse("cache implementation")?;
        let ranking = scratch.search_ranked(
            &query,
            crate::application::retrieval::Mode::Hybrid,
            true,
            true,
            || None,
        )?;
        let capture = serde_json::to_value(ranking.embedding.ok_or("query vector")?)?;
        assert_eq!(
            capture["vector"].as_array().ok_or("vector")?.len(),
            DIMENSIONS
        );
        assert_eq!(
            capture["nearest"][0]["evidence"]["source"],
            "benchmark:pinned.rs"
        );
        assert_eq!(primary.progress()?.indexed, 0);
        primary.set_index_enabled(false)?;
        assert!(matches!(
            benchmark_retrieval(Some(&primary), &test),
            Err(RetrievalError::Disabled)
        ));
        Ok(())
    }
}
