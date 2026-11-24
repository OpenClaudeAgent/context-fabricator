//! GitHub Actions run metadata, using only the guarded shared transport.
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde::Deserialize;

use super::{
    GitHubRest,
    objects::valid_oid,
    request::{json, json_with_limit},
};
use crate::application::github::{
    GitHubFuture,
    actions::{RunPage, Source},
    history::SourceRequest,
};
use crate::domain::{
    github::{
        Error, RepositoryName,
        actions::{Run, Workflow},
    },
    sync::RequestKind,
};

// The 100-run page includes nested provider metadata that we do not retain.
const RUN_PAGE_LIMIT: usize = 4 * 1024 * 1024;

impl Source for GitHubRest {
    fn workflows<'a>(
        &'a self,
        repository: &'a str,
        request: SourceRequest<'a>,
    ) -> GitHubFuture<'a, Vec<Workflow>> {
        Box::pin(async move {
            RepositoryName::parse(repository)?;
            let path = format!("/repos/{repository}/actions/workflows?per_page=100&page=1");
            let page: Workflows = self
                .get_metadata(
                    request,
                    RequestKind::Pagination,
                    &path,
                    "/repos/{owner}/{repo}/actions/workflows",
                    json,
                )
                .await?;
            if page.total_count > 100 || page.workflows.len() != page.total_count {
                return Err(Error::CollectionLimit);
            }
            let mut ids = std::collections::HashSet::new();
            page.workflows
                .into_iter()
                .map(|workflow| {
                    if workflow.id <= 0
                        || !ids.insert(workflow.id)
                        || !clean(&workflow.name, 256)
                        || !clean(&workflow.state, 64)
                        || !valid_workflow_path(&workflow.path)
                    {
                        return Err(Error::Upstream);
                    }
                    Ok(Workflow {
                        id: workflow.id,
                        name: workflow.name,
                        path: workflow.path,
                        state: workflow.state,
                        definition_blob: None,
                        definition: None,
                    })
                })
                .collect()
        })
    }
    fn runs<'a>(
        &'a self,
        repository: &'a str,
        branch: &'a str,
        request: SourceRequest<'a>,
    ) -> GitHubFuture<'a, RunPage> {
        Box::pin(async move {
            RepositoryName::parse(repository)?;
            let encoded_branch = utf8_percent_encode(branch, NON_ALPHANUMERIC);
            let path = format!(
                "/repos/{repository}/actions/runs?branch={encoded_branch}&per_page=100&page=1"
            );
            let page: Runs = self
                .get_metadata(
                    request,
                    RequestKind::Pagination,
                    &path,
                    "/repos/{owner}/{repo}/actions/runs",
                    json_with_limit::<Runs, RUN_PAGE_LIMIT>,
                )
                .await?;
            if page.workflow_runs.len() > 100
                || page.workflow_runs.len() > page.total_count
                || (page.total_count <= 100 && page.workflow_runs.len() != page.total_count)
            {
                return Err(Error::Upstream);
            }
            let mut ids = std::collections::HashSet::new();
            let runs = page
                .workflow_runs
                .into_iter()
                .map(|r| {
                    if !ids.insert((r.id, r.run_attempt))
                        || r.head_branch
                            .as_deref()
                            .is_some_and(|value| value != branch)
                    {
                        return Err(Error::Upstream);
                    }
                    r.validated()
                })
                .collect::<Result<_, _>>()?;
            Ok(RunPage {
                runs,
                total: page.total_count,
            })
        })
    }
}

fn valid_workflow_path(path: &str) -> bool {
    if !clean(path, 512) || path.contains("..") {
        return false;
    }
    if let Some(file) = path.strip_prefix(".github/workflows/") {
        return !file.is_empty() && file != "." && !file.contains('/');
    }
    path.strip_prefix("dynamic/").is_some_and(|managed_path| {
        managed_path
            .split('/')
            .all(|part| !part.is_empty() && part != ".")
    })
}

fn clean(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn timestamp(value: &str) -> bool {
    value.len() == 20
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value.as_bytes()[10] == b'T'
        && value.as_bytes()[13] == b':'
        && value.as_bytes()[16] == b':'
        && value.ends_with('Z')
        && value
            .bytes()
            .enumerate()
            .all(|(i, b)| matches!(i, 4 | 7 | 10 | 13 | 16 | 19) || b.is_ascii_digit())
}
#[derive(Deserialize)]
struct Workflows {
    total_count: usize,
    workflows: Vec<WireWorkflow>,
}
#[derive(Deserialize)]
struct WireWorkflow {
    id: i64,
    name: String,
    path: String,
    state: String,
}
#[derive(Deserialize)]
struct Runs {
    total_count: usize,
    workflow_runs: Vec<WireRun>,
}
#[derive(Deserialize)]
struct WireRun {
    id: i64,
    workflow_id: i64,
    run_attempt: u32,
    event: String,
    head_sha: String,
    head_branch: Option<String>,
    head_repository: Option<WireRepository>,
    repository: Option<PublicRepository>,
    name: Option<String>,
    path: Option<String>,
    created_at: String,
    updated_at: String,
    status: String,
    conclusion: Option<String>,
}
#[derive(Deserialize)]
struct WireRepository {
    id: i64,
}

#[derive(Deserialize)]
struct PublicRepository {
    id: i64,
    full_name: String,
    private: bool,
}

impl WireRun {
    fn validated(self) -> Result<Run, Error> {
        let r = self;
        if r.id <= 0
            || r.workflow_id <= 0
            || r.run_attempt == 0
            || !valid_oid(&r.head_sha)
            || !clean(&r.event, 64)
            || !clean(&r.status, 64)
            || !timestamp(&r.created_at)
            || !timestamp(&r.updated_at)
            || r.head_branch.as_ref().is_none_or(|s| !clean(s, 256))
            || r.conclusion.as_ref().is_some_and(|s| !clean(s, 64))
            || r.head_repository.as_ref().is_some_and(|repo| repo.id <= 0)
        {
            return Err(Error::Upstream);
        }
        Ok(Run {
            id: r.id,
            attempt: r.run_attempt,
            workflow_id: r.workflow_id,
            event: r.event,
            head_sha: r.head_sha,
            head_branch: r.head_branch,
            head_repository_id: r.head_repository.map(|repo| repo.id),
            commit_provenance: "reportedOnly".into(),
            created_at: r.created_at,
            updated_at: r.updated_at,
            status: r.status,
            conclusion: r.conclusion,
        })
    }
}

impl GitHubRest {
    pub(super) async fn actions_for_commit(
        &self,
        repository: &str,
        revision: &str,
    ) -> Result<Vec<crate::domain::github::actions::Snapshot>, Error> {
        use crate::domain::github::actions::Snapshot;
        use std::collections::{BTreeMap, HashSet};

        RepositoryName::parse(repository)?;
        if !valid_oid(revision) {
            return Err(Error::InvalidSelection);
        }
        let mut branches = BTreeMap::<String, Snapshot>::new();
        let mut ids = HashSet::new();
        for page in 1..=10 {
            let path = format!(
                "/repos/{repository}/actions/runs?head_sha={revision}&per_page=50&page={page}"
            );
            let response: Runs = self
                .get(
                    &[],
                    RequestKind::Pagination,
                    &path,
                    json_with_limit::<Runs, RUN_PAGE_LIMIT>,
                )
                .await?;
            if response.total_count > 500 {
                return Err(Error::CollectionLimit);
            }
            let remaining = response.total_count.saturating_sub((page - 1) * 50);
            if response.workflow_runs.len() != remaining.min(50) {
                return Err(Error::Upstream);
            }
            for wire in response.workflow_runs {
                let authority = wire
                    .repository
                    .as_ref()
                    .filter(|repo| {
                        !repo.private
                            && repo.id > 0
                            && repo.full_name.eq_ignore_ascii_case(repository)
                    })
                    .ok_or(Error::Upstream)?;
                let branch = wire
                    .head_branch
                    .as_deref()
                    .filter(|name| clean(name, 250))
                    .ok_or(Error::Upstream)?;
                if wire.head_sha != revision || !ids.insert((wire.id, wire.run_attempt)) {
                    return Err(Error::Upstream);
                }
                let snapshot = branches
                    .entry(branch.to_owned())
                    .or_insert_with(|| Snapshot {
                        repository_id: authority.id,
                        repository: repository.into(),
                        branch: branch.into(),
                        definition_commit: revision.into(),
                        workflows: Vec::new(),
                        workflow_coverage: "observedRunMetadata".into(),
                        executed_definition: "unknown".into(),
                        runs: Vec::new(),
                        run_coverage: "returnedCommitRuns".into(),
                        attempt_coverage: "observedLatestAttemptsOnly".into(),
                    });
                if snapshot.repository_id != authority.id {
                    return Err(Error::Upstream);
                }
                if !snapshot
                    .workflows
                    .iter()
                    .any(|item| item.id == wire.workflow_id)
                    && let (Some(name), Some(path)) = (&wire.name, &wire.path)
                {
                    let path = path.split('@').next().unwrap_or_default();
                    if clean(name, 256) && valid_workflow_path(path) {
                        snapshot.workflows.push(Workflow {
                            id: wire.workflow_id,
                            name: name.clone(),
                            path: path.into(),
                            state: "unknown".into(),
                            definition_blob: None,
                            definition: None,
                        });
                    }
                }
                snapshot.runs.push(wire.validated()?);
            }
            if remaining <= 50 {
                return Ok(branches.into_values().collect());
            }
        }
        Err(Error::CollectionLimit)
    }
}
