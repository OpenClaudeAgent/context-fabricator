use uuid::Uuid;

use super::{Answer, AnswerError, AnswerRequest, Answers, actions, history};
use crate::application::{
    Failure, SearchQuery,
    answer_history::{Journal, Record, Scope},
};

/// A validated answer target and the query whose scope it uses.
pub struct Submission<'a> {
    query: &'a SearchQuery,
    target: Target<'a>,
}

enum Target<'a> {
    Current,
    History(history::Selection<'a>),
    Actions(actions::Selection<'a>),
}

impl<'a> Submission<'a> {
    /// # Errors
    /// Rejects conflicting targets and selectors outside the query's repository and branch scope.
    pub fn parse(
        query: &'a SearchQuery,
        child: Option<&'a str>,
        run: Option<i64>,
        attempt: Option<u32>,
    ) -> Result<Self, Failure> {
        let target = match (child, run, attempt) {
            (None, None, None) => Target::Current,
            (Some(child), None, None) => Target::History(history::Selection::parse(query, child)?),
            (None, run, attempt) => {
                Target::Actions(actions::Selection::parse(query, run, attempt)?)
            }
            (Some(_), _, _) => return Err(Failure::ConflictingAnswerScopes),
        };
        Ok(Self { query, target })
    }

    /// Dispatch the validated target and journal a completed answer when capture is enabled.
    ///
    /// # Errors
    /// Returns an error when the requested answer cannot be completed.
    pub fn execute(
        &self,
        answers: &Answers,
        metadata: &AnswerRequest<'_>,
        journal: Option<&dyn Journal>,
        now: impl FnOnce() -> i64,
    ) -> Result<Answer, AnswerError> {
        let capture = journal.and_then(|journal| {
            journal
                .begin()
                .unwrap_or_else(|_| {
                    tracing::warn!(
                        code = "answer_history_capture_unavailable",
                        "Could not begin local answer history capture"
                    );
                    None
                })
                .map(|capture| (journal, capture))
        });
        let answer = match &self.target {
            Target::Current => answers.ask(self.query, metadata),
            Target::History(selection) => answers.ask_history(self.query, *selection, metadata),
            Target::Actions(selection) => answers.ask_actions(self.query, selection, metadata),
        }?;
        if let Some((journal, capture)) = capture {
            let record = Record {
                question: self.query.original(),
                scope: &self.scope(),
                answer: &answer,
                completed_at_ms: now(),
                trace_id: metadata.trace.map(|link| Uuid::from_bytes(link.trace)),
            };
            if journal.save(capture, record).is_err() {
                tracing::warn!(
                    code = "answer_history_save_unavailable",
                    "Could not store a local answer history entry"
                );
            }
        }
        Ok(answer)
    }

    fn scope(&self) -> Scope {
        let mut scope = Scope {
            repository_id: self.query.scope().map(|scope| scope.repository_id),
            branch: self.query.scope().map(|scope| scope.branch.clone()),
            ..Scope::default()
        };
        match &self.target {
            Target::Current => {}
            Target::History(selection) => scope.history_commit = Some(selection.child.to_owned()),
            Target::Actions(selection) => {
                scope.actions_run_id = Some(selection.run_id);
                scope.actions_run_attempt = Some(selection.attempt);
            }
        }
        scope
    }
}
