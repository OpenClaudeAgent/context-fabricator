//! Collection of history lineage, isolated from current-code indexing.

use std::sync::Arc;
use tokio::sync::Mutex;

use super::{CredentialError, CredentialStore, GitHubFuture};
use crate::application::telemetry::{TraceContext, TraceLink};
use crate::domain::github::{
    Error,
    history::{Boundary, Commit, MAX_COMMITS, Pin, Scope, Snapshot, State},
};

mod preparation;
pub use preparation::Attempts;
pub mod schedule;

pub use super::MetadataRequest as SourceRequest;

pub enum TreeEntry {
    Tree { path: String, id: String },
    File(crate::domain::github::history::comparison::File),
}

pub trait ComparisonSource: Send + Sync {
    fn tree<'a>(
        &'a self,
        repository: &'a str,
        id: &'a str,
        request: SourceRequest<'a>,
    ) -> GitHubFuture<'a, Vec<TreeEntry>>;
    fn blob<'a>(
        &'a self,
        repository: &'a str,
        id: &'a str,
        request: SourceRequest<'a>,
    ) -> GitHubFuture<'a, Vec<u8>>;
}

pub trait Source: ComparisonSource + Send + Sync {
    fn verify_repository<'a>(
        &'a self,
        scope: &'a Scope,
        request: SourceRequest<'a>,
    ) -> GitHubFuture<'a, String>;
    fn pin<'a>(
        &'a self,
        scope: &'a Scope,
        repository: &'a str,
        request: SourceRequest<'a>,
    ) -> GitHubFuture<'a, Pin>;
    fn commit<'a>(
        &'a self,
        repository: &'a str,
        id: &'a str,
        request: SourceRequest<'a>,
    ) -> GitHubFuture<'a, Commit>;
}

pub struct Refresh {
    pub snapshot: Snapshot,
    pub origin: Option<TraceLink>,
}

pub struct Reopen<'a> {
    pub repository_id: i64,
    pub branch: &'a str,
    pub child: &'a str,
    pub parent: Option<&'a str>,
    pub change: usize,
    pub hunk: usize,
}

pub struct Reopened {
    pub hunk: crate::domain::github::history::hunks::Hunk,
    pub retained: bool,
    pub adjacent: Vec<crate::domain::github::history::hunks::AlignedLine>,
}

pub trait Store: schedule::ScheduleStore + Send + Sync {
    /// # Errors
    /// Rejects unavailable storage or a repository ineligible for history refresh.
    fn history_scope(&self, id: i64) -> Result<Scope, Error>;

    /// # Errors
    /// Returns a storage error or rejects a repository no longer configured.
    fn history_state(&self, id: i64) -> Result<State, Error>;

    /// # Errors
    /// Rejects any change to the authority or active lineage in one read.
    fn check_comparison_scope(&self, scope: &Scope, snapshot: &str) -> Result<(), Error>;

    /// # Errors
    /// Rejects a stale authority or lineage and immutable identity collisions.
    fn publish_comparison(
        &self,
        scope: &Scope,
        snapshot: &str,
        child: &str,
        parent: Option<&str>,
        document: &crate::domain::github::history::HistoryEvidence,
        blobs: &[(String, Vec<u8>)],
    ) -> Result<(), Error>;
    /// # Errors
    /// Rejects ineligible repositories or unavailable storage.
    fn read_comparison_evidence(
        &self,
        id: i64,
        child: &str,
    ) -> Result<Option<crate::domain::github::history::HistoryEvidence>, Error>;

    /// # Errors
    /// Rejects corrupt retained bytes or unavailable storage.
    fn reopen_hunk(&self, selection: &Reopen<'_>) -> Result<Option<Reopened>, Error>;

    /// # Errors
    /// Returns a storage error or rejects publication by a stale scope.
    fn publish_history(
        &self,
        scope: &Scope,
        snapshot: &Snapshot,
        expected_due_ms: i64,
        checked: i64,
        producer: Option<TraceLink>,
    ) -> Result<(), Error>;
}

pub(super) struct Collector {
    source: Arc<dyn Source>,
    store: Arc<dyn Store>,
    active: Mutex<()>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
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
            now: Arc::new(now),
        }
    }

    pub(super) fn state(&self, id: i64) -> Result<State, Error> {
        self.store.history_state(id)
    }

    pub(super) async fn comparison(
        &self,
        id: i64,
        child: &str,
        credentials: &(dyn CredentialStore + Send + Sync),
        trace: Option<TraceContext<'_>>,
        expire: impl FnOnce(&str) -> Result<(), Error>,
    ) -> Result<crate::domain::github::history::comparison::Comparison, Error> {
        let scope = self.store.history_scope(id)?;
        let result = self
            .read_comparison(&scope, child, credentials, None, trace)
            .await;
        if matches!(result, Err(Error::Reconnect)) {
            expire(&scope.credential)?;
        }
        result
    }

    pub(super) fn evidence(
        &self,
        id: i64,
        child: &str,
    ) -> Result<Option<crate::domain::github::history::HistoryEvidence>, Error> {
        self.store.read_comparison_evidence(id, child)
    }

    pub(super) async fn materialize(
        &self,
        id: i64,
        child: &str,
        credentials: &(dyn CredentialStore + Send + Sync),
        trace: Option<TraceContext<'_>>,
        expire: impl FnOnce(&str) -> Result<(), Error>,
    ) -> Result<crate::domain::github::history::HistoryEvidence, Error> {
        let scope = self.store.history_scope(id)?;
        let comparison = self
            .comparison(id, child, credentials, trace, expire)
            .await?;
        self.publish_evidence(&scope, child, comparison)
    }

    fn publish_evidence(
        &self,
        scope: &Scope,
        child: &str,
        comparison: crate::domain::github::history::comparison::Comparison,
    ) -> Result<crate::domain::github::history::HistoryEvidence, Error> {
        use crate::domain::github::history::hunks;
        let id = scope.repository_id;
        let snapshot = self.state(id)?.snapshot.ok_or(Error::Conflict)?;
        let selected = snapshot
            .commits
            .iter()
            .find(|c| c.id == child)
            .ok_or(Error::Conflict)?;
        if snapshot.tip != comparison.lineage_tip
            || selected.parents.first() != comparison.parent.as_ref()
        {
            return Err(Error::Conflict);
        }
        let mut changes = Vec::with_capacity(comparison.changes.len());
        let mut blobs = std::collections::BTreeMap::new();
        for mut change in comparison.changes {
            let projection = hunks::project(&change)?;
            for side in [&change.before, &change.after].into_iter().flatten() {
                if let Some(text) = &side.text
                    && let Some(prior) = blobs.insert(side.blob.clone(), text.as_bytes().to_vec())
                    && prior != text.as_bytes()
                {
                    return Err(Error::Upstream);
                }
            }
            if let Some(before) = &mut change.before {
                before.text = None;
            }
            if let Some(after) = &mut change.after {
                after.text = None;
            }
            changes.push(crate::domain::github::history::EvidenceChange { change, projection });
        }
        let document = crate::domain::github::history::HistoryEvidence {
            repository_id: id,
            repository: comparison.repository,
            branch: comparison.branch,
            child: comparison.child,
            parent: comparison.parent.clone(),
            baseline: comparison.baseline,
            coverage: comparison.coverage,
            projection_version: 1,
            changes,
        };
        let snapshot = serde_json::to_string(&snapshot).map_err(|_| Error::Storage)?;
        self.store.publish_comparison(
            scope,
            &snapshot,
            child,
            comparison.parent.as_deref(),
            &document,
            &blobs.into_iter().collect::<Vec<_>>(),
        )?;
        Ok(document)
    }

    async fn read_comparison(
        &self,
        scope: &Scope,
        child: &str,
        credentials: &(dyn CredentialStore + Send + Sync),
        budget: Option<&preparation::RequestBudget>,
        trace: Option<TraceContext<'_>>,
    ) -> Result<crate::domain::github::history::comparison::Comparison, Error> {
        use crate::domain::github::history::comparison::{self, Comparison};
        let snapshot = self
            .state(scope.repository_id)?
            .snapshot
            .ok_or(Error::Conflict)?;
        let commit = snapshot
            .commits
            .iter()
            .find(|item| item.id == child)
            .ok_or(Error::InvalidSelection)?;
        let parent = commit.parents.first().cloned();
        let fingerprint = serde_json::to_string(&snapshot).map_err(|_| Error::Storage)?;
        let guard = || {
            self.store.check_comparison_scope(scope, &fingerprint)?;
            if let Some(budget) = budget {
                budget.admit()?;
            }
            Ok(())
        };
        let secret = credentials.read(&scope.credential).map_err(|e| match e {
            CredentialError::Missing => Error::Reconnect,
            _ => Error::Credentials,
        })?;
        let request = SourceRequest {
            secret: &secret,
            guard: &guard,
            trace,
        };
        let repository = self.source.verify_repository(scope, request).await?;
        let base = if let Some(parent_id) = &parent {
            let base = if let Some(base) = snapshot.commits.iter().find(|c| &c.id == parent_id) {
                base.clone()
            } else {
                self.source.commit(&repository, parent_id, request).await?
            };
            if base.id != *parent_id {
                return Err(Error::Upstream);
            }
            Some(base.tree)
        } else {
            None
        };
        let plan = if budget.is_some() {
            preparation::changed_plan(self, &repository, base.as_deref(), &commit.tree, request)
                .await?
        } else {
            let old = match base {
                Some(tree) => self.files(&repository, &tree, request).await?,
                None => Vec::new(),
            };
            comparison::plan(old, self.files(&repository, &commit.tree, request).await?)?
        };
        let changes = self.comparison_changes(plan, &repository, request).await?;
        if self.source.verify_repository(scope, request).await? != repository {
            return Err(Error::Conflict);
        }
        self.store.check_comparison_scope(scope, &fingerprint)?;
        let partial = has_partial_content(&changes);
        Ok(Comparison {
            repository_id: scope.repository_id,
            repository,
            branch: scope.branch.clone(),
            lineage_tip: snapshot.tip,
            child: child.to_owned(),
            baseline: if parent.is_some() {
                "firstParent"
            } else {
                "emptyTree"
            }
            .to_owned(),
            parent,
            coverage: if partial {
                "partialContent"
            } else {
                "completeTreesFullBlobs"
            }
            .to_owned(),
            changes,
        })
    }

    async fn comparison_changes(
        &self,
        plan: Vec<crate::domain::github::history::comparison::PlannedChange>,
        repository: &str,
        request: SourceRequest<'_>,
    ) -> Result<Vec<crate::domain::github::history::comparison::Change>, Error> {
        use crate::domain::github::history::comparison::{self, Change};
        let size: u64 = plan
            .iter()
            .flat_map(|(_, a, b)| [a, b])
            .flatten()
            .filter(|f| comparison::readable(f))
            .map(|f| f.size.unwrap_or(0))
            .sum();
        if size > comparison::MAX_TOTAL_BYTES as u64 {
            return Err(Error::CollectionLimit);
        }
        let mut changes = Vec::new();
        for (kind, before, after) in plan {
            let before = self
                .comparison_side(repository, before.as_ref(), request)
                .await?;
            let after = self
                .comparison_side(repository, after.as_ref(), request)
                .await?;
            changes.push(Change {
                kind: kind.to_owned(),
                before,
                after,
            });
        }
        Ok(changes)
    }

    async fn comparison_side(
        &self,
        repository: &str,
        file: Option<&crate::domain::github::history::comparison::File>,
        request: SourceRequest<'_>,
    ) -> Result<Option<crate::domain::github::history::comparison::Side>, Error> {
        use crate::domain::github::history::comparison;
        let Some(file) = file else {
            return Ok(None);
        };
        if !comparison::readable(file) {
            return Ok(Some(comparison::unavailable(file)));
        }
        Ok(Some(comparison::side(
            file,
            self.source.blob(repository, &file.blob, request).await?,
        )?))
    }

    async fn files(
        &self,
        repository: &str,
        tree: &str,
        request: SourceRequest<'_>,
    ) -> Result<Vec<crate::domain::github::history::comparison::File>, Error> {
        let mut pending = vec![(String::new(), tree.to_owned())];
        let mut files = Vec::new();
        let mut count = 0;
        let mut entries_seen = 0;
        while let Some((prefix, id)) = pending.pop() {
            count += 1;
            if count > 256 {
                return Err(Error::CollectionLimit);
            }
            for entry in self.source.tree(repository, &id, request).await? {
                entries_seen += 1;
                if entries_seen > 4096 {
                    return Err(Error::CollectionLimit);
                }
                match entry {
                    TreeEntry::Tree { path, id } => {
                        let next = format!("{prefix}{path}/");
                        if next.len() > 1024 || count + pending.len() >= 256 {
                            return Err(Error::CollectionLimit);
                        }
                        pending.push((next, id));
                    }
                    TreeEntry::File(mut file) => {
                        file.path = format!("{prefix}{}", file.path);
                        if file.path.len() > 1024 {
                            return Err(Error::CollectionLimit);
                        }
                        files.push(file);
                        if files.len() > 4096 {
                            return Err(Error::CollectionLimit);
                        }
                    }
                }
            }
        }
        Ok(files)
    }

    async fn collect(
        &self,
        scope: &Scope,
        expected_due_ms: i64,
        credentials: &(dyn CredentialStore + Send + Sync),
        trace: Option<TraceContext<'_>>,
    ) -> Result<Snapshot, Error> {
        let guard = || match self.store.history_scope(scope.repository_id) {
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
        let previous = self
            .state(scope.repository_id)?
            .snapshot
            .filter(|data| data.branch == scope.branch);
        let request = SourceRequest {
            secret: &secret,
            guard: &guard,
            trace,
        };
        let repository = self.source.verify_repository(scope, request).await?;
        let pin = self.source.pin(scope, &repository, request).await?;
        let mut commits = Vec::<Commit>::new();
        let mut cursor = Some(pin.commit.clone());
        while commits.len() < MAX_COMMITS {
            let Some(id) = cursor.take() else { break };
            if commits.iter().any(|item| item.id == id) {
                return Err(Error::Upstream);
            }
            let cached = previous
                .as_ref()
                .and_then(|data| data.commits.iter().find(|item| item.id == id));
            let commit = match cached {
                Some(commit) => commit.clone(),
                None => self.source.commit(&repository, &id, request).await?,
            };
            if commit.id != id || (commits.is_empty() && commit.tree != pin.tree) {
                return Err(Error::Upstream);
            }
            cursor = commit.parents.first().cloned();
            commits.push(commit);
        }
        let snapshot = Snapshot {
            branch: scope.branch.clone(),
            tip: pin.commit,
            boundary: if cursor.is_some() {
                Boundary::CommitLimit
            } else {
                Boundary::Root
            },
            commits,
        };
        if self.source.verify_repository(scope, request).await? != repository {
            return Err(Error::Conflict);
        }
        self.store.publish_history(
            scope,
            &snapshot,
            expected_due_ms,
            (self.now)(),
            trace.map(|context| context.parent),
        )?;
        Ok(snapshot)
    }
}

fn has_partial_content(changes: &[crate::domain::github::history::comparison::Change]) -> bool {
    changes
        .iter()
        .flat_map(|change| [change.before.as_ref(), change.after.as_ref()])
        .flatten()
        .any(|side| side.coverage != "fullBlob")
}
