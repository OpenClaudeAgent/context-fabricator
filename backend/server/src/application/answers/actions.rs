//! Evidence from a single recorded Actions run attempt.

use ctxfab_inference::generation::{Draft, GenerationAttempt, Request};
use serde::Serialize;
use serde_json::json;

use super::evidence_review::{Candidate, EvidenceReview, Pipeline, Source};
use super::{
    Answer, AnswerError, AnswerMode, AnswerRequest, Answers, Citation, GenerationEvidence,
    ModelContext, observe_generation,
};
use crate::application::{
    CodeScope, Failure, SearchHit, SearchQuery,
    telemetry::{Event, Observer, RetrievalCounts, Stage, TraceLink},
};
use crate::domain::github::actions::Run;

pub use super::actions_scope::AttemptTarget;

pub struct Selection<'a> {
    pub scope: &'a CodeScope,
    pub run_id: i64,
    pub attempt: u32,
}

impl<'a> Selection<'a> {
    /// # Errors
    /// Requires a repository, branch, positive run ID and positive attempt number.
    pub fn parse(
        query: &'a SearchQuery,
        run_id: Option<i64>,
        attempt: Option<u32>,
    ) -> Result<Self, Failure> {
        Ok(Self {
            scope: query.scope().ok_or(Failure::InvalidActionsScope)?,
            run_id: run_id
                .filter(|id| *id > 0)
                .ok_or(Failure::InvalidActionsScope)?,
            attempt: attempt
                .filter(|number| *number > 0)
                .ok_or(Failure::InvalidActionsScope)?,
        })
    }
}

pub struct Observation {
    pub retained: bool,
    pub repository: String,
    pub run: Run,
    pub workflow_name: Option<String>,
    pub workflow_path: Option<String>,
}

impl Observation {
    pub(super) fn review_candidate(&self) -> Candidate {
        let mut candidate = Candidate::new(
            Source::GitHubActions,
            format!(
                "{} · run {} · attempt {}",
                self.repository, self.run.id, self.run.attempt
            ),
            Some(self.run.head_sha.clone()),
        );
        candidate.preview(&self.fields());
        candidate
    }

    pub(crate) fn fields(&self) -> String {
        let mut text = render(&self.run);
        if let Some(name) = &self.workflow_name {
            text.push_str("\nObserved workflow name: ");
            text.push_str(name);
        }
        if let Some(path) = &self.workflow_path {
            text.push_str("\nObserved workflow path: ");
            text.push_str(path);
        }
        if let Some(branch) = &self.run.head_branch {
            text.push_str("\nReported head branch: ");
            text.push_str(branch);
        }
        if let Some(repository) = self.run.head_repository_id {
            text.push_str("\nReported head repository ID: ");
            text.push_str(&repository.to_string());
        }
        text
    }
}

pub struct LocatedRun {
    pub scope: CodeScope,
    pub observation: Observation,
}

impl LocatedRun {
    pub(super) fn selection(&self) -> Selection<'_> {
        Selection {
            scope: &self.scope,
            run_id: self.observation.run.id,
            attempt: self.observation.run.attempt,
        }
    }
}

pub trait ActionsStore: Send + Sync {
    /// Find the latest recorded run by creation time in one unambiguously identified scope.
    ///
    /// # Errors
    /// Returns a storage error if the snapshot cannot be read.
    fn latest(
        &self,
        scope: Option<&CodeScope>,
        repository: Option<&str>,
    ) -> Result<Option<LocatedRun>, AnswerError>;

    /// Find matching recorded attempts in local snapshots.
    ///
    /// # Errors
    /// Returns a storage error when a snapshot cannot be read.
    fn discover(
        &self,
        query: &SearchQuery,
        target: Option<&AttemptTarget>,
    ) -> Result<Vec<LocatedRun>, AnswerError>;

    /// Reopen the exact recorded run and attempt in the selected repository and branch.
    ///
    /// # Errors
    /// Returns a storage error if the observation cannot be read coherently.
    fn run(&self, selection: &Selection<'_>) -> Result<Option<Observation>, AnswerError>;
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCitation {
    repository_id: i64,
    workflow_id: i64,
    run_id: i64,
    attempt: u32,
    run_url: String,
    reported_head: String,
    commit_provenance: String,
}

impl Answers {
    /// # Errors
    /// Returns a typed failure if the local store or inference worker cannot complete.
    pub fn ask_actions(
        &self,
        query: &SearchQuery,
        selection: &Selection<'_>,
        metadata: &AnswerRequest<'_>,
    ) -> Result<Answer, AnswerError> {
        let request = self.begin_answer(metadata)?;
        let result = if super::actions_capability::needs_job_observation(query, true) {
            Ok(Answer::withheld(
                &request,
                AnswerMode::Selected,
                None,
                Vec::new(),
            ))
        } else {
            self.actions_answer(query, selection, &request, metadata)
        };
        request.finish()?;
        result
    }

    fn actions_answer(
        &self,
        query: &SearchQuery,
        selection: &Selection<'_>,
        request: &Request<'_>,
        metadata: &AnswerRequest<'_>,
    ) -> Result<Answer, AnswerError> {
        let (observer, trace) = (metadata.observer, metadata.trace);
        let actions = self.actions.as_deref().ok_or(AnswerError::Storage)?;
        let evidence = find_attempt(actions, selection, observer, trace)?;
        let mut answer = Answer::withheld(
            request,
            AnswerMode::Selected,
            request.captures_context().then(|| {
                let mut review = EvidenceReview::new(query);
                review.search(
                    Source::GitHubActions,
                    usize::from(evidence.is_some()),
                    Some(format!(
                        "Run {} · attempt {}",
                        selection.run_id, selection.attempt
                    )),
                );
                if let Some(evidence) = &evidence {
                    let mut candidate = evidence.review_candidate();
                    candidate.propose(1);
                    review.add(candidate);
                }
                review.pipeline = metadata.inspect_pipeline.then(Pipeline::default);
                review
            }),
            Vec::new(),
        );
        let Some(evidence) = evidence else {
            return Ok(answer);
        };

        let text = evidence.fields();
        let language = self
            .repository
            .settings()
            .map_err(|_| AnswerError::Storage)?
            .preferred_answer_language();
        let generated = observe_generation(
            observer,
            trace,
            GenerationEvidence::WorkflowAttempt,
            |record| {
                request.generate(1, false, false, record, |_| {
                    json!({
                        "question": query.original(),
                        "answerLanguage": super::answer_language(language),
                        "evidenceFamily": "githubActions",
                        "passages": [{"id": 1, "kind": "workflow", "text": text}],
                    })
                    .to_string()
                    .replace('<', "\\u003c")
                    .replace('>', "\\u003e")
                })
            },
        )?;
        let result = match generated {
            GenerationAttempt::Completed(result) => result,
            GenerationAttempt::WithoutDraft(stop) => return Ok(answer.stopped(stop)),
        };
        let draft = result.draft;
        if let Some(capture) = answer
            .evidence_review
            .as_mut()
            .and_then(|review| review.pipeline.as_mut())
        {
            answer.model_context =
                ModelContext::from_completion(result.completion_prompt.as_deref());
            capture
                .completion_input
                .clone_from(&result.completion_prompt);
            capture.draft = Some(draft.clone());
        }
        if let Some(citations) =
            verify_with_trace(actions, selection, &evidence, &text, &draft, metadata)?
        {
            answer.outcome = "answered";
            answer.text = Some(draft.answer.trim().to_owned());
            answer.citations = citations;
            if let Some(review) = answer
                .evidence_review
                .as_mut()
                .filter(|review| review.capturing())
            {
                review.verified(result.evidence_count, &draft);
            } else {
                answer.evidence_review = None;
            }
            answer.model_context =
                ModelContext::from_completion(result.completion_prompt.as_deref());
            answer
                .evidence_repository_ids
                .push(selection.scope.repository_id);
        } else if let Some(review) = &mut answer.evidence_review {
            review.unverified(
                result.evidence_count,
                &draft.citations.iter().map(|citation| citation.passage),
            );
        }
        Ok(answer)
    }
}

fn find_attempt(
    actions: &dyn ActionsStore,
    selection: &Selection<'_>,
    observer: Option<&dyn Observer>,
    trace: Option<TraceLink>,
) -> Result<Option<Observation>, AnswerError> {
    let lookup = observer.map(|observer| observer.start(Stage::AnswerRetrieval, trace));
    let evidence = actions.run(selection);
    if let Some(mut lookup) = lookup {
        lookup.event(Event::ActionsAttemptSelected);
        if let Ok(evidence) = &evidence {
            let count = u16::from(evidence.is_some());
            lookup.retrieval_counts(RetrievalCounts {
                passage_candidates: None,
                history_pairs: None,
                workflow_attempts: Some(count),
                proposed_sources: count,
            });
            if evidence.is_none() {
                lookup.event(Event::NoEvidence);
            }
        }
        lookup.finish(evidence.is_ok());
    }
    evidence
}

pub(crate) fn render(run: &Run) -> String {
    format!(
        "Workflow ID: {}\nRun ID: {}\nAttempt: {}\nEvent: {}\nStatus: {}\nConclusion: {}\n\
         Created: {}\nUpdated: {}\nReported head commit: {}\nHead provenance: {}",
        run.workflow_id,
        run.id,
        run.attempt,
        run.event,
        run.status,
        run.conclusion.as_deref().unwrap_or("not recorded"),
        run.created_at,
        run.updated_at,
        run.head_sha,
        run.commit_provenance,
    )
}

fn verify_with_trace(
    store: &dyn ActionsStore,
    selection: &Selection<'_>,
    evidence: &Observation,
    text: &str,
    draft: &Draft,
    metadata: &AnswerRequest<'_>,
) -> Result<Option<Vec<Citation>>, AnswerError> {
    let verification = metadata
        .observer
        .map(|observer| observer.start(Stage::AnswerValidation, metadata.trace));
    let citations = verify(store, selection, evidence, text, draft);
    if let Some(mut verification) = verification {
        if let Ok(citations) = &citations {
            verification.event(if citations.is_some() {
                Event::CitationsVerified
            } else {
                Event::CitationsRejected
            });
        }
        verification.finish(matches!(citations, Ok(Some(_))));
    }
    citations
}

fn verify(
    store: &dyn ActionsStore,
    selection: &Selection<'_>,
    evidence: &Observation,
    text: &str,
    draft: &Draft,
) -> Result<Option<Vec<Citation>>, AnswerError> {
    if draft.answer.trim().is_empty()
        || draft.answer.len() > 2_048
        || draft.citations.is_empty()
        || draft.citations.len() > super::MAX_SOURCES
    {
        return Ok(None);
    }
    if draft.citations.iter().any(|r| r.passage != 1) {
        return Ok(None);
    }
    let quotes = draft
        .citations
        .iter()
        .map(|r| r.quote.trim())
        .collect::<Vec<_>>();
    reopen_citations(store, selection, evidence, text, &quotes)
}

pub(super) fn reopen_citations(
    store: &dyn ActionsStore,
    selection: &Selection<'_>,
    evidence: &Observation,
    text: &str,
    references: &[&str],
) -> Result<Option<Vec<Citation>>, AnswerError> {
    let Some(current) = store.run(selection)? else {
        return Ok(None);
    };
    if current.run.id != selection.run_id
        || current.run.attempt != selection.attempt
        || current.repository != evidence.repository
        || current.fields() != text
    {
        return Ok(None);
    }
    let recorded: Vec<_> = text.split('\n').collect();
    let mut quotes = Vec::new();
    for &reference in references {
        let lines: Vec<_> = reference.split('\n').take(super::MAX_SOURCES + 1).collect();
        if lines.len() > super::MAX_SOURCES
            || lines.iter().any(|line| line.len() < super::MIN_QUOTE_BYTES)
        {
            return Ok(None);
        }
        let Some(original) = recorded
            .windows(lines.len())
            .find(|window| *window == lines)
        else {
            return Ok(None);
        };
        for &line in original {
            if !quotes.contains(&line) {
                quotes.push(line);
            }
        }
        if quotes.len() > super::MAX_SOURCES {
            return Ok(None);
        }
    }
    Ok(Some(
        quotes
            .into_iter()
            .map(|quote| citation(selection, &current, quote))
            .collect(),
    ))
}

pub(super) fn citation(selection: &Selection<'_>, evidence: &Observation, quote: &str) -> Citation {
    let run = &evidence.run;
    let repository = &evidence.repository;
    let repo_id = selection.scope.repository_id;
    Citation {
        evidence: SearchHit {
            source: format!("github:{repo_id}:actions:{}:{}", run.id, run.attempt),
            revision: format!("run-{}-attempt-{}", run.id, run.attempt),
            kind: "workflow".into(),
            tracked_ref: None,
            title: format!("Workflow run {} · Attempt {}", run.id, run.attempt),
            access: if evidence.retained {
                "retained"
            } else {
                "available"
            },
            location: None,
        },
        quote: quote.into(),
        line_url: None,
        comparison: None,
        workflow_run: Some(RunCitation {
            repository_id: repo_id,
            workflow_id: run.workflow_id,
            run_id: run.id,
            attempt: run.attempt,
            run_url: format!(
                "https://github.com/{repository}/actions/runs/{}/attempts/{}",
                run.id, run.attempt
            ),
            reported_head: run.head_sha.clone(),
            commit_provenance: run.commit_provenance.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use ctxfab_inference::generation::Reference;

    use super::{
        ActionsStore, AnswerError, Draft, Observation, Run, SearchQuery, Selection, verify,
    };

    fn observation(conclusion: &str) -> Observation {
        Observation {
            retained: false,
            workflow_name: None,
            workflow_path: None,
            repository: "owner/repo".into(),
            run: Run {
                id: 789,
                attempt: 3,
                workflow_id: 101,
                event: "push".into(),
                head_sha: "a".repeat(40),
                head_branch: Some("main".into()),
                head_repository_id: Some(41),
                commit_provenance: "verifiedRepositoryCommit".into(),
                created_at: "2026-11-20T10:00:00Z".into(),
                updated_at: "2026-11-20T10:00:10Z".into(),
                status: "completed".into(),
                conclusion: Some(conclusion.into()),
            },
        }
    }

    struct Store(Observation);

    impl Store {
        fn with_conclusion(conclusion: &str) -> Self {
            Self(observation(conclusion))
        }
    }

    impl ActionsStore for Store {
        fn latest(
            &self,
            _: Option<&crate::application::CodeScope>,
            _: Option<&str>,
        ) -> Result<Option<super::LocatedRun>, AnswerError> {
            Ok(None)
        }

        fn discover(
            &self,
            _: &SearchQuery,
            _: Option<&super::AttemptTarget>,
        ) -> Result<Vec<super::LocatedRun>, AnswerError> {
            Ok(Vec::new())
        }

        fn run(&self, selection: &Selection<'_>) -> Result<Option<Observation>, AnswerError> {
            assert_eq!(
                (
                    selection.scope.repository_id,
                    selection.run_id,
                    selection.attempt
                ),
                (41, 789, 3)
            );
            Ok(Some(Observation {
                retained: self.0.retained,
                repository: self.0.repository.clone(),
                run: self.0.run.clone(),
                workflow_name: self.0.workflow_name.clone(),
                workflow_path: self.0.workflow_path.clone(),
            }))
        }
    }

    #[test]
    fn actions_citations_reopen_the_selected_attempt_and_reject_changed_or_invented_fields()
    -> Result<(), Box<dyn std::error::Error>> {
        let query =
            SearchQuery::parse_scoped("What was the conclusion?", Some(41), Some("main".into()))?;
        let selection = Selection::parse(&query, Some(789), Some(3))?;
        let evidence = observation("cancelled");
        let text = evidence.fields();
        let mut draft = Draft {
            answer: "The attempt was cancelled.".into(),
            citations: vec![Reference {
                passage: 1,
                quote: "Conclusion: cancelled".into(),
            }],
        };
        let citations = verify(
            &Store::with_conclusion("cancelled"),
            &selection,
            &evidence,
            &text,
            &draft,
        )?
        .ok_or("expected a stable citation")?;
        assert_eq!(citations[0].quote, "Conclusion: cancelled");
        assert_eq!(
            citations[0]
                .workflow_run
                .as_ref()
                .ok_or("expected an attempt locator")?
                .run_url,
            "https://github.com/owner/repo/actions/runs/789/attempts/3"
        );
        assert!(
            verify(
                &Store::with_conclusion("success"),
                &selection,
                &evidence,
                &text,
                &draft
            )?
            .is_none()
        );
        draft.citations[0].quote = "Conclusion: failure".into();
        assert!(
            verify(
                &Store::with_conclusion("cancelled"),
                &selection,
                &evidence,
                &text,
                &draft
            )?
            .is_none()
        );
        Ok(())
    }

    #[test]
    fn consecutive_actions_fields_reopen_as_individual_exact_citations()
    -> Result<(), Box<dyn std::error::Error>> {
        let query =
            SearchQuery::parse_scoped("What triggered the run?", Some(41), Some("main".into()))?;
        let selection = Selection::parse(&query, Some(789), Some(3))?;
        let evidence = observation("success");
        let text = evidence.fields();
        let draft = |quote: &str| Draft {
            answer: "A push triggered the run and it succeeded.".into(),
            citations: vec![Reference {
                passage: 1,
                quote: quote.into(),
            }],
        };
        let exact = draft("Event: push\nStatus: completed\nConclusion: success");
        let citations = verify(
            &Store::with_conclusion("success"),
            &selection,
            &evidence,
            &text,
            &exact,
        )?
        .ok_or("consecutive recorded fields")?;
        assert_eq!(
            citations
                .iter()
                .map(|citation| citation.quote.as_str())
                .collect::<Vec<_>>(),
            ["Event: push", "Status: completed", "Conclusion: success"],
        );
        assert!(
            citations.iter().all(
                |citation| citation.workflow_run.as_ref().is_some_and(|run| {
                    run.run_url == "https://github.com/owner/repo/actions/runs/789/attempts/3"
                })
            )
        );
        for quote in [
            "Event: push\nConclusion: success",
            "Conclusion: success\nEvent: push",
            "Event: push Status: completed",
            "Event: push\nStatus: completed\nConclusion: failure",
            "A push triggered this attempt.\nStatus: completed",
            "Event: push\nObserved workflow path: .github/workflows/ci.yml",
        ] {
            assert!(
                verify(
                    &Store::with_conclusion("success"),
                    &selection,
                    &evidence,
                    &text,
                    &draft(quote)
                )?
                .is_none()
            );
        }
        let mut too_many = exact;
        too_many.citations.push(Reference {
            passage: 1,
            quote: "Workflow ID: 101\nRun ID: 789\nAttempt: 3".into(),
        });
        assert!(
            verify(
                &Store::with_conclusion("success"),
                &selection,
                &evidence,
                &text,
                &too_many
            )?
            .is_none()
        );
        let mut other = evidence;
        other.run.attempt = 8;
        let fabricated = draft("Attempt: 8\nEvent: push");
        assert!(
            verify(
                &Store::with_conclusion("success"),
                &selection,
                &other,
                &other.fields(),
                &fabricated
            )?
            .is_none()
        );
        Ok(())
    }

    #[test]
    fn reported_head_citations_keep_the_observed_values_and_reopen_the_attempt()
    -> Result<(), Box<dyn std::error::Error>> {
        let query = SearchQuery::parse_scoped(
            "What head did GitHub report?",
            Some(41),
            Some("main".into()),
        )?;
        let selection = Selection::parse(&query, Some(789), Some(3))?;
        let fork_observation = || {
            let mut observed = observation("success");
            observed.run.head_branch = Some("topic/fork-head".into());
            observed.run.head_repository_id = Some(99);
            observed.run.commit_provenance = "forkOrUnknownReportedHead".into();
            observed.workflow_name = Some("CI".into());
            observed.workflow_path = Some(".github/workflows/ci.yml".into());
            observed
        };
        let evidence = fork_observation();
        let text = evidence.fields();
        let draft = Draft {
            answer: "GitHub reported topic/fork-head from repository 99.".into(),
            citations: vec![Reference {
                passage: 1,
                quote: "Reported head branch: topic/fork-head\nReported head repository ID: 99"
                    .into(),
            }],
        };
        let citations = verify(
            &Store(fork_observation()),
            &selection,
            &evidence,
            &text,
            &draft,
        )?
        .ok_or("expected same-attempt head evidence")?;
        assert_eq!(
            citations
                .iter()
                .map(|citation| citation.quote.as_str())
                .collect::<Vec<_>>(),
            [
                "Reported head branch: topic/fork-head",
                "Reported head repository ID: 99"
            ],
        );
        for citation in citations {
            let run = citation.workflow_run.ok_or("attempt provenance")?;
            assert_eq!(run.repository_id, 41);
            assert_eq!(
                run.run_url,
                "https://github.com/owner/repo/actions/runs/789/attempts/3"
            );
            assert_eq!(run.commit_provenance, "forkOrUnknownReportedHead");
        }
        assert!(text.ends_with(
            "Observed workflow path: .github/workflows/ci.yml\nReported head branch: topic/fork-head\nReported head repository ID: 99",
        ));
        let mut updated = fork_observation();
        updated.run.head_repository_id = Some(98);
        assert!(verify(&Store(updated), &selection, &evidence, &text, &draft)?.is_none());
        let mut updated = fork_observation();
        updated.run.head_branch = Some("topic/updated".into());
        assert!(verify(&Store(updated), &selection, &evidence, &text, &draft)?.is_none());
        Ok(())
    }

    #[test]
    fn absent_reported_head_fields_supply_neither_a_value_nor_a_configured_fallback()
    -> Result<(), Box<dyn std::error::Error>> {
        let query = SearchQuery::parse_scoped(
            "What head did GitHub report?",
            Some(41),
            Some("main".into()),
        )?;
        let selection = Selection::parse(&query, Some(789), Some(3))?;
        let without_head = || {
            let mut observed = observation("success");
            observed.run.head_branch = None;
            observed.run.head_repository_id = None;
            observed.run.commit_provenance = "forkOrUnknownReportedHead".into();
            observed
        };
        let evidence = without_head();
        let text = evidence.fields();
        assert!(!text.contains("Reported head branch:"));
        assert!(!text.contains("Reported head repository ID:"));
        for quote in [
            "Reported head branch: main",
            "Reported head repository ID: 41",
        ] {
            let draft = Draft {
                answer: "The head matches the configured repository.".into(),
                citations: vec![Reference {
                    passage: 1,
                    quote: quote.into(),
                }],
            };
            assert!(
                verify(&Store(without_head()), &selection, &evidence, &text, &draft)?.is_none()
            );
        }
        Ok(())
    }
}
