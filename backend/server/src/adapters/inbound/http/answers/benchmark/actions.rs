use std::sync::{Arc, LazyLock};

use axum::{
    Json,
    response::{IntoResponse, Response},
};
use serde::Deserialize;

use crate::{
    application::{
        CodeScope, SearchQuery,
        answers::{
            Answer, AnswerError, AnswerRequest, Answers,
            actions::{ActionsStore, AttemptTarget, LocatedRun, Observation, Selection},
        },
    },
    domain::github::actions::{Run, Snapshot, Workflow},
};

const CAPTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../quality/github/benchmark-actions.json"
));

#[derive(Deserialize)]
struct Capture {
    snapshots: Vec<Snapshot>,
}

static RECORDED: LazyLock<Result<Capture, serde_json::Error>> =
    LazyLock::new(|| serde_json::from_str(CAPTURE));

fn snapshots() -> Result<&'static [Snapshot], &'static serde_json::Error> {
    RECORDED
        .as_ref()
        .map(|capture| capture.snapshots.as_slice())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Key {
    repository: String,
    run_id: i64,
    attempt: u32,
    #[serde(default)]
    pub(super) downloaded: bool,
}

pub(in crate::adapters::inbound::http::answers) async fn list() -> Response {
    match snapshots() {
        Ok(snapshots) => Json(snapshots).into_response(),
        Err(_) => super::failure(super::Failure::Storage),
    }
}

pub(super) fn find(key: &Key) -> Result<Option<Recorded>, &'static serde_json::Error> {
    Ok(find_in(key, snapshots()?))
}

pub(super) fn find_in(key: &Key, snapshots: &[Snapshot]) -> Option<Recorded> {
    for snapshot in snapshots {
        if snapshot.repository != key.repository {
            continue;
        }
        if let Some(run) = snapshot
            .runs
            .iter()
            .find(|run| (run.id, run.attempt) == (key.run_id, key.attempt))
        {
            let workflow = snapshot
                .workflows
                .iter()
                .find(|workflow| workflow.id == run.workflow_id)
                .cloned();
            return Some(Recorded {
                repository_id: snapshot.repository_id,
                branch: snapshot.branch.clone(),
                repository: snapshot.repository.clone(),
                run: run.clone(),
                workflow,
            });
        }
    }
    None
}

pub(super) struct Recorded {
    repository_id: i64,
    branch: String,
    repository: String,
    run: Run,
    workflow: Option<Workflow>,
}

impl Recorded {
    pub(super) fn answer(
        self,
        answers: &Answers,
        query: &SearchQuery,
        metadata: &AnswerRequest<'_>,
    ) -> Result<Answer, AnswerError> {
        let scope = CodeScope {
            repository_id: self.repository_id,
            branch: self.branch.clone(),
        };
        let selection = Selection {
            scope: &scope,
            run_id: self.run.id,
            attempt: self.run.attempt,
        };
        answers
            .clone()
            .with_actions(Arc::new(self))
            .ask_actions(query, &selection, metadata)
    }
}

impl ActionsStore for Recorded {
    fn latest(
        &self,
        _: Option<&CodeScope>,
        _: Option<&str>,
    ) -> Result<Option<LocatedRun>, AnswerError> {
        Ok(None)
    }

    fn discover(
        &self,
        _: &SearchQuery,
        _: Option<&AttemptTarget>,
    ) -> Result<Vec<LocatedRun>, AnswerError> {
        Ok(Vec::new())
    }

    fn run(&self, selection: &Selection<'_>) -> Result<Option<Observation>, AnswerError> {
        Ok((selection.scope.repository_id == self.repository_id
            && selection.scope.branch == self.branch
            && selection.run_id == self.run.id
            && selection.attempt == self.run.attempt)
            .then(|| Observation {
                retained: true,
                repository: self.repository.clone(),
                run: self.run.clone(),
                workflow_name: self.workflow.as_ref().map(|workflow| workflow.name.clone()),
                workflow_path: self.workflow.as_ref().map(|workflow| workflow.path.clone()),
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_action_is_bound_to_repository_branch_and_attempt()
    -> Result<(), Box<dyn std::error::Error>> {
        let key = Key {
            repository: "tokio-rs/bytes".into(),
            run_id: 42_437_411_775,
            attempt: 1,
            downloaded: false,
        };
        let recorded = find(&key)?.ok_or("bytes CI attempt")?;
        let scope = CodeScope {
            repository_id: recorded.repository_id,
            branch: "master".into(),
        };
        let lookup = |scope| Selection {
            scope,
            run_id: key.run_id,
            attempt: 1,
        };
        let found = recorded
            .run(&lookup(&scope))?
            .ok_or("exact recorded attempt")?;
        assert_eq!(
            found.run.head_sha,
            "62207901b65e4fc51459b2d558fad8aa882f5d33"
        );
        assert_eq!(found.run.conclusion.as_deref(), Some("failure"));

        let other_branch = CodeScope {
            repository_id: scope.repository_id,
            branch: "main".into(),
        };
        assert!(recorded.run(&lookup(&other_branch))?.is_none());
        assert!(
            recorded
                .run(&Selection {
                    attempt: 2,
                    ..lookup(&scope)
                })?
                .is_none()
        );
        assert!(
            find(&Key {
                repository: "psf/requests".into(),
                ..key
            })?
            .is_none()
        );
        Ok(())
    }
}
