//! Independent Actions metadata collection and guarded publication.
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::{
    GitHubFuture, finish_collection,
    history::{self, SourceRequest},
};
use crate::application::credentials::{CredentialError, CredentialStore};
use crate::application::telemetry::{Stage, TraceContext, TraceLink, TraceOrigin};
use crate::domain::github::{
    Error,
    actions::{Due, Run, Scope, Snapshot, State, Workflow, retain_recent_attempts},
};

pub struct RunPage {
    pub runs: Vec<Run>,
    pub total: usize,
}

#[derive(Clone, Copy)]
pub enum Trigger {
    Manual,
    Scheduled,
}

pub trait Source: history::Source {
    fn workflows<'a>(
        &'a self,
        repository: &'a str,
        request: SourceRequest<'a>,
    ) -> GitHubFuture<'a, Vec<Workflow>>;
    fn runs<'a>(
        &'a self,
        repository: &'a str,
        branch: &'a str,
        request: SourceRequest<'a>,
    ) -> GitHubFuture<'a, RunPage>;
}

pub trait Store: Send + Sync {
    /// # Errors
    /// Rejects an unavailable or unauthorized Actions scope.
    fn actions_scope(&self, id: i64) -> Result<Scope, Error>;
    /// # Errors
    /// Returns an error if the saved observations cannot be read.
    fn actions_state(&self, id: i64) -> Result<State, Error>;
    /// # Errors
    /// Returns an error if the eligible schedule cannot be read.
    fn actions_due(&self, id: Option<i64>) -> Result<Option<Due>, Error>;
    /// # Errors
    /// Rejects an obsolete schedule or an unavailable store.
    fn claim_actions(&self, due: &Due, trace: Option<TraceLink>) -> Result<(), Error>;
    /// # Errors
    /// Rejects a stale schedule or a storage failure.
    fn actions_failed(
        &self,
        due: &Due,
        now_ms: i64,
        error: Error,
        trigger: Trigger,
    ) -> Result<(), Error>;
    /// # Errors
    /// Rejects a stale scope or unavailable storage.
    fn publish_actions(
        &self,
        scope: &Scope,
        due: &Due,
        snapshot: &Snapshot,
        checked_at_ms: i64,
        producer: Option<TraceLink>,
    ) -> Result<(), Error>;
}

pub(super) struct Collector {
    source: Arc<dyn Source>,
    store: Arc<dyn Store>,
    active: Mutex<()>,
    now: Box<dyn Fn() -> i64 + Send + Sync>,
}
impl Collector {
    pub(super) fn new(
        source: Arc<dyn Source>,
        store: Arc<dyn Store>,
        now: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Self {
        Self {
            source,
            store,
            active: Mutex::new(()),
            now: Box::new(now),
        }
    }
    pub(super) fn next(&self) -> Result<Option<Due>, Error> {
        self.store.actions_due(None)
    }
    pub(super) fn state(&self, id: i64) -> Result<State, Error> {
        self.store.actions_state(id)
    }
    pub(super) async fn refresh(
        &self,
        id: i64,
        expected: Option<&Due>,
        credentials: &(dyn CredentialStore + Send + Sync),
        trace: Option<TraceOrigin<'_>>,
        expire: impl FnOnce(&str) -> Result<(), Error>,
    ) -> Result<Snapshot, Error> {
        let _active = if expected.is_some() {
            self.active.lock().await
        } else {
            self.active.try_lock().map_err(|_| Error::Conflict)?
        };
        let due = self
            .store
            .actions_due(Some(id))?
            .ok_or(if expected.is_some() {
                Error::Conflict
            } else {
                Error::Access
            })?;
        if expected.is_some_and(|selected| *selected != due || due.at_ms > (self.now)()) {
            return Err(Error::Conflict);
        }
        let scope = self.store.actions_scope(id)?;
        if scope.generation != due.generation {
            return Err(Error::Conflict);
        }
        let (trace, span) = trace
            .map(|origin| origin.start(Stage::GitHubActionsCollection))
            .unzip();
        let attempt = trace.map(|context| context.parent);
        if let Err(error) = self.store.claim_actions(&due, attempt) {
            finish_collection::<()>(span, &Err(error));
            return Err(error);
        }
        let result = self
            .collect(&scope, credentials, trace)
            .await
            .and_then(|snapshot| {
                self.store
                    .publish_actions(&scope, &due, &snapshot, (self.now)(), attempt)?;
                Ok(snapshot)
            });
        finish_collection(span, &result);
        if let Err(error) = result {
            let trigger = if expected.is_some() {
                Trigger::Scheduled
            } else {
                Trigger::Manual
            };
            let failure = self
                .store
                .actions_failed(&due, (self.now)(), error, trigger);
            if error == Error::Reconnect {
                expire(&scope.credential)?;
            }
            failure?;
        }
        result
    }
    async fn collect(
        &self,
        scope: &Scope,
        credentials: &(dyn CredentialStore + Send + Sync),
        trace: Option<TraceContext<'_>>,
    ) -> Result<Snapshot, Error> {
        let guard = || match self.store.actions_scope(scope.repository_id) {
            Ok(current) if current == *scope => Ok(()),
            Ok(_) | Err(Error::Access) => Err(Error::Conflict),
            Err(error) => Err(error),
        };
        let secret = credentials
            .read(&scope.credential)
            .map_err(|error| match error {
                CredentialError::Missing => Error::Reconnect,
                _ => Error::Credentials,
            })?;
        let read_scope = crate::domain::github::history::Scope {
            repository_id: scope.repository_id,
            full_name: scope.full_name.clone(),
            branch: scope.branch.clone(),
            generation: scope.generation,
            credential: scope.credential.clone(),
        };
        let request = SourceRequest {
            secret: &secret,
            guard: &guard,
            trace,
        };
        let repository = self.source.verify_repository(&read_scope, request).await?;
        let pin = self.source.pin(&read_scope, &repository, request).await?;
        let mut workflows = self.source.workflows(&repository, request).await?;
        attach_definitions(
            &*self.source,
            &repository,
            &pin.tree,
            request,
            &mut workflows,
        )
        .await?;
        let page = self
            .source
            .runs(&repository, &scope.branch, request)
            .await?;
        let observations = self
            .merge_runs(scope, &repository, page.runs, request)
            .await?;
        if self.source.verify_repository(&read_scope, request).await? != repository {
            return Err(Error::Conflict);
        }
        let snapshot = Snapshot {
            repository_id: scope.repository_id,
            repository,
            branch: scope.branch.clone(),
            definition_commit: pin.commit,
            workflows,
            workflow_coverage: "providerDefaultBranchList".into(),
            executed_definition: "unknown".into(),
            runs: observations,
            run_coverage: if page.total > 100 {
                "latest100"
            } else {
                "returnedBranchRuns"
            }
            .into(),
            attempt_coverage: "observedLatestAttemptsOnly".into(),
        };
        Ok(snapshot)
    }
    async fn merge_runs(
        &self,
        scope: &Scope,
        repository: &str,
        runs: Vec<Run>,
        request: SourceRequest<'_>,
    ) -> Result<Vec<Run>, Error> {
        let mut observations = BTreeMap::new();
        if let Some(previous) = self
            .state(scope.repository_id)?
            .snapshot
            .filter(|snapshot| snapshot.branch == scope.branch)
        {
            for run in previous.runs {
                observations.insert((run.id, run.attempt), run);
            }
        }
        let mut verified = std::collections::HashMap::<String, String>::new();
        for mut run in runs {
            run.commit_provenance = if run.head_repository_id != Some(scope.repository_id) {
                "forkOrUnknownReportedHead".into()
            } else if let Some(provenance) = verified.get(&run.head_sha) {
                provenance.clone()
            } else if verified.len() >= 16 {
                "verificationLimit".into()
            } else {
                let provenance = match self.source.commit(repository, &run.head_sha, request).await
                {
                    Ok(commit) if commit.id == run.head_sha => "verifiedRepositoryCommit",
                    Ok(_) => return Err(Error::Upstream),
                    Err(Error::ExpiredPin | Error::Access) => "commitUnavailable",
                    Err(error) => return Err(error),
                }
                .to_owned();
                verified.insert(run.head_sha.clone(), provenance.clone());
                provenance
            };
            let key = (run.id, run.attempt);
            if observations.get(&key).is_none_or(|old: &Run| {
                old.updated_at < run.updated_at
                    && !(old.status == "completed" && run.status != "completed")
            }) {
                observations.insert(key, run);
            }
        }
        Ok(retain_recent_attempts(observations.into_values().collect()))
    }
}

async fn attach_definitions(
    source: &dyn Source,
    repository: &str,
    root: &str,
    request: SourceRequest<'_>,
    workflows: &mut [Workflow],
) -> Result<(), Error> {
    use history::TreeEntry;
    use sha1::{Digest, Sha1};
    if workflows
        .iter()
        .all(|workflow| !workflow.path.starts_with(".github/workflows/"))
    {
        return Ok(());
    }
    let mut tree = root.to_owned();
    for name in [".github", "workflows"] {
        let entries = source.tree(repository, &tree, request).await?;
        let Some(id) = entries.into_iter().find_map(|entry| match entry {
            TreeEntry::Tree { path, id } if path == name => Some(id),
            _ => None,
        }) else {
            return Ok(());
        };
        tree = id;
    }
    let files = source.tree(repository, &tree, request).await?;
    let mut total_bytes = 0_u64;
    for workflow in workflows {
        let Some(name) = workflow.path.strip_prefix(".github/workflows/") else {
            continue;
        };
        let Some(file) = files.iter().find_map(|entry| match entry {
            TreeEntry::File(file)
                if file.path == name
                    && matches!(file.mode.as_str(), "100644" | "100755")
                    && file.size.is_some_and(|size| size <= 100_000) =>
            {
                Some(file)
            }
            _ => None,
        }) else {
            continue;
        };
        total_bytes = total_bytes.saturating_add(file.size.unwrap_or(0));
        if total_bytes > 1_000_000 {
            return Err(Error::CollectionLimit);
        }
        let bytes = source.blob(repository, &file.blob, request).await?;
        let mut hash = Sha1::new();
        hash.update(format!("blob {}\0", bytes.len()).as_bytes());
        hash.update(&bytes);
        let mut digest = String::with_capacity(40);
        for byte in hash.finalize() {
            write!(&mut digest, "{byte:02x}").map_err(|_| Error::Storage)?;
        }
        if digest != file.blob || file.size != Some(bytes.len() as u64) {
            return Err(Error::Upstream);
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        workflow.definition_blob = Some(file.blob.clone());
        workflow.definition = Some(text);
    }
    Ok(())
}
