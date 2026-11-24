//! Bounded background discovery of immutable comparison evidence.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::application::{
    github::finish_collection,
    telemetry::{Stage, TraceOrigin},
};

use super::{Collector, CredentialStore, Error, Scope, Snapshot, SourceRequest, TreeEntry};
use crate::domain::github::history::comparison::{self, PlannedChange};

const RECENT_CHILDREN: usize = 8;
// Two modified files in sibling crates can require twelve tree reads, four
// blob reads and two repository checks. Leave room for guarded re-admissions
// while keeping each scheduled comparison bounded by the shared provider gate.
const MAX_COMPARISON_ADMISSIONS: usize = 32;

/// Last attempt time for retryable children; a null value records an immutable collection limit.
pub type Attempts = BTreeMap<String, Option<i64>>;

#[derive(Default)]
pub(super) struct RequestBudget(AtomicUsize);

impl RequestBudget {
    // Executed on guarded HTTP admission, including redirects. Re-admission after
    // a ledger wait may conservatively consume another slot without sending.
    pub(super) fn admit(&self) -> Result<(), Error> {
        if self.0.fetch_add(1, Ordering::Relaxed) < MAX_COMPARISON_ADMISSIONS {
            Ok(())
        } else {
            Err(Error::CollectionLimit)
        }
    }

    fn exhausted(&self) -> bool {
        self.0.load(Ordering::Relaxed) > MAX_COMPARISON_ADMISSIONS
    }
}

impl Collector {
    pub(super) async fn prepare_next(
        &self,
        scope: &Scope,
        snapshot: &Snapshot,
        credentials: &(dyn CredentialStore + Send + Sync),
        origin: Option<TraceOrigin<'_>>,
        expire: impl FnOnce(&str) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let previous = self.store.preparation_attempts(scope)?;
        let mut attempts = previous.clone();
        let recent: Vec<_> = snapshot
            .commits
            .iter()
            .take(RECENT_CHILDREN)
            .filter(|c| !c.parents.is_empty())
            .collect();
        attempts.retain(|child, _| recent.iter().any(|c| c.id == *child));
        let mut ordered = recent;
        ordered.sort_by_key(|c| attempts.get(&c.id).copied().flatten());
        for commit in ordered {
            if attempts.get(&commit.id) == Some(&None)
                || self.evidence(scope.repository_id, &commit.id)?.is_some()
            {
                continue;
            }
            let budget = RequestBudget::default();
            let (trace, span) = origin
                .map(|origin| origin.start(Stage::GitHubHistoryPreparation))
                .unzip();
            let result = self
                .read_comparison(scope, &commit.id, credentials, Some(&budget), trace)
                .await
                .map_err(|error| {
                    // The shared transport maps an admission failure to storage.
                    // Only a denial by this comparison's own budget can restore the cause.
                    if error == Error::Storage && budget.exhausted() {
                        Error::CollectionLimit
                    } else {
                        error
                    }
                })
                .and_then(|comparison| self.publish_evidence(scope, &commit.id, comparison));
            finish_collection(span, &result);
            match result {
                Ok(_) => {
                    attempts.remove(&commit.id);
                }
                Err(Error::Reconnect) => {
                    expire(&scope.credential)?;
                    return Err(Error::Reconnect);
                }
                Err(Error::Conflict) => {
                    return Err(Error::Conflict);
                }
                Err(Error::CollectionLimit) => {
                    attempts.insert(commit.id.clone(), None);
                }
                Err(_) => {
                    attempts.insert(commit.id.clone(), Some((self.now)()));
                }
            }
            break;
        }
        if attempts != previous {
            self.store
                .save_preparation_attempts(scope, &snapshot.tip, &attempts)?;
        }
        Ok(())
    }
}

type Trees = BTreeMap<String, String>;

// A shared subtree identity proves its contents are unchanged. For possible
// renames, the full-tree planner also checks blob uniqueness in unchanged paths.
pub(super) async fn changed_plan(
    collector: &Collector,
    repository: &str,
    parent: Option<&str>,
    child: &str,
    request: SourceRequest<'_>,
) -> Result<Vec<PlannedChange>, Error> {
    let mut pending = vec![(
        String::new(),
        parent.map(str::to_owned),
        Some(child.to_owned()),
    )];
    let (mut old, mut new) = (Vec::new(), Vec::new());
    let mut skipped = false;
    let mut seen = 0;
    while let Some((prefix, before, after)) = pending.pop() {
        if before == after {
            skipped = true;
            continue;
        }
        let [mut before_trees, mut after_trees] = [Trees::new(), Trees::new()];
        for (id, files, trees) in [
            (before, &mut old, &mut before_trees),
            (after, &mut new, &mut after_trees),
        ] {
            if let Some(id) = id {
                for entry in collector.source.tree(repository, &id, request).await? {
                    seen += 1;
                    if seen > 8192 {
                        return Err(Error::CollectionLimit);
                    }
                    match entry {
                        TreeEntry::Tree { path, id } => {
                            let path = format!("{prefix}{path}/");
                            if path.len() > 1024 {
                                return Err(Error::CollectionLimit);
                            }
                            trees.insert(path, id);
                        }
                        TreeEntry::File(mut file) => {
                            file.path = format!("{prefix}{}", file.path);
                            if file.path.len() > 1024 || files.len() >= 4096 {
                                return Err(Error::CollectionLimit);
                            }
                            files.push(file);
                        }
                    }
                }
            }
        }
        let paths: BTreeSet<_> = before_trees
            .keys()
            .chain(after_trees.keys())
            .cloned()
            .collect();
        for path in paths {
            let ids = (before_trees.remove(&path), after_trees.remove(&path));
            pending.push((path, ids.0, ids.1));
        }
    }
    let plan = comparison::plan(old, new)?;
    if skipped
        && plan.iter().any(|(kind, _, _)| *kind == "renamed")
        && let Some(parent) = parent
    {
        return comparison::plan(
            collector.files(repository, parent, request).await?,
            collector.files(repository, child, request).await?,
        );
    }
    Ok(plan)
}
