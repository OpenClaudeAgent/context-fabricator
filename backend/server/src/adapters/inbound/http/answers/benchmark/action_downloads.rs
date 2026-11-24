//! Public commit-scoped observations downloaded explicitly by the user.
use std::{
    fs,
    io::{ErrorKind, Write},
    path::Path,
};

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use super::{Api, GitHubError, Reference, actions, cache, invalid_reference, load_failed};
use crate::domain::github::actions::Snapshot;

#[derive(Serialize, Deserialize)]
pub(super) struct Download {
    reference: Reference,
    snapshots: Vec<Snapshot>,
}

pub(in crate::adapters::inbound::http::answers) async fn list(State(api): State<Api>) -> Response {
    match saved(api.benchmark_actions.as_deref()) {
        Ok(downloads) => Json(downloads).into_response(),
        Err(error) => load_failed(error),
    }
}

pub(in crate::adapters::inbound::http::answers) async fn download(
    State(api): State<Api>,
    input: Result<Json<Reference>, JsonRejection>,
) -> Response {
    let Ok(Json(reference)) = input else {
        return invalid_reference();
    };
    if reference.name().is_none() {
        return invalid_reference();
    }
    let (Some(root), Some(github)) = (api.benchmark_actions.as_deref(), &api.github) else {
        return load_failed(GitHubError::Storage);
    };
    let (repository, revision) = reference.sort_key();
    let snapshots = match github.public_actions(repository, revision).await {
        Ok(snapshots) => snapshots,
        Err(error) => return load_failed(error),
    };
    let result = Download {
        reference,
        snapshots,
    };
    match persist(root, &result) {
        Ok(()) => Json(result).into_response(),
        Err(error) => load_failed(error),
    }
}

pub(super) fn find(
    root: Option<&Path>,
    key: &actions::Key,
) -> Result<Option<actions::Recorded>, GitHubError> {
    Ok(saved(root)?
        .into_iter()
        .find_map(|download| actions::find_in(key, &download.snapshots)))
}

fn saved(root: Option<&Path>) -> Result<Vec<Download>, GitHubError> {
    let Some(root) = root else {
        return Ok(Vec::new());
    };
    let files = match fs::read_dir(root) {
        Ok(files) => files,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(GitHubError::Storage),
    };
    let mut downloads = Vec::new();
    for file in files {
        let file = file.map_err(|_| GitHubError::Storage)?.path();
        if file.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let download = fs::read(&file).map_err(|_| GitHubError::Storage)?;
        downloads
            .push(serde_json::from_slice::<Download>(&download).map_err(|_| GitHubError::Storage)?);
    }
    downloads.sort_by(|a, b| a.reference.sort_key().cmp(&b.reference.sort_key()));
    Ok(downloads)
}

fn persist(root: &Path, download: &Download) -> Result<(), GitHubError> {
    fs::create_dir_all(root).map_err(|_| GitHubError::Storage)?;
    let path = root.join(format!("{}.json", cache::key(&download.reference)));
    let mut staging = tempfile::NamedTempFile::new_in(root).map_err(|_| GitHubError::Storage)?;
    serde_json::to_writer(&mut staging, download).map_err(|_| GitHubError::Storage)?;
    staging.flush().map_err(|_| GitHubError::Storage)?;
    staging
        .as_file()
        .sync_all()
        .map_err(|_| GitHubError::Storage)?;
    staging.persist(path).map_err(|_| GitHubError::Storage)?;
    Ok(())
}
