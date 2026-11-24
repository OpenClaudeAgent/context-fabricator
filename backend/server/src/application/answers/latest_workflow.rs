//! Direct answers for an unfiltered latest workflow run in one repository scope.

use ctxfab_inference::generation::Request;

use crate::application::{AnswerLanguage, SearchQuery, telemetry::RetrievalCounts};

use super::{
    Answer, AnswerError, AnswerMode, AnswerRequest, Answers, Event, Stage, actions,
    evidence_review::{EvidenceReview, Source},
};

struct LatestQuery {
    repository: Option<String>,
    language: AnswerLanguage,
}

const FORMS: &[(&str, &str, AnswerLanguage)] = &[
    ("latest github actions run", " for ", AnswerLanguage::En),
    (
        "what was the latest github actions run",
        " for ",
        AnswerLanguage::En,
    ),
    (
        "what was the latest github actions run",
        " in ",
        AnswerLanguage::En,
    ),
    (
        "dernière exécution github actions",
        " sur ",
        AnswerLanguage::Fr,
    ),
    (
        "quelle est la dernière exécution github actions",
        " sur ",
        AnswerLanguage::Fr,
    ),
    (
        "c'est quoi la dernière github action qui a run",
        " sur ",
        AnswerLanguage::Fr,
    ),
];

impl LatestQuery {
    fn parse(query: &SearchQuery) -> Option<Self> {
        let text = query
            .original()
            .trim()
            .to_lowercase()
            .replace("derniere", "dernière")
            .replace('’', "'");
        let text = text.trim_end_matches(['?', '!']).trim_end();
        FORMS.iter().find_map(|&(form, separator, language)| {
            let suffix = text.strip_prefix(form)?;
            let repository = if suffix.is_empty() {
                None
            } else {
                let value = suffix.strip_prefix(separator)?;
                let full_name = if value.contains('/') {
                    value.to_owned()
                } else {
                    format!("repo/{value}")
                };
                let name = crate::domain::github::RepositoryName::parse(&full_name).ok()?;
                // Only accept an exact repository identifier in the question.
                if full_name != format!("{}/{}", name.owner, name.name) {
                    return None;
                }
                Some(value.to_owned())
            };
            Some(Self {
                repository,
                language,
            })
        })
    }
}

impl Answers {
    pub(super) fn answer_latest_workflow(
        &self,
        query: &SearchQuery,
        request: &Request<'_>,
        metadata: &AnswerRequest<'_>,
    ) -> Result<Option<Answer>, AnswerError> {
        let (Some(latest), Some(store)) = (LatestQuery::parse(query), self.actions.as_deref())
        else {
            return Ok(None);
        };
        let lookup = metadata
            .observer
            .map(|observer| observer.start(Stage::AnswerRetrieval, metadata.trace));
        let observed = store.latest(query.scope(), latest.repository.as_deref());
        if let Some(mut lookup) = lookup {
            if let Ok(observed) = &observed {
                let count = u16::from(observed.is_some());
                lookup.retrieval_counts(RetrievalCounts {
                    passage_candidates: None,
                    history_pairs: None,
                    workflow_attempts: Some(count),
                    proposed_sources: count,
                });
                lookup.event(if observed.is_some() {
                    Event::LatestActionsRunSelected
                } else {
                    Event::NoEvidence
                });
            }
            lookup.finish(observed.is_ok());
        }
        let Some(observed) = observed? else {
            let review = request.captures_context().then(|| {
                let mut review = EvidenceReview::new(query);
                review.search(Source::GitHubActions, 0, latest.repository);
                review
            });
            return Ok(Some(Answer::withheld(
                request,
                AnswerMode::Selected,
                review,
                Vec::new(),
            )));
        };

        let configured = self
            .repository
            .settings()
            .map_err(|_| AnswerError::Storage)?
            .preferred_answer_language();
        let language = if configured == AnswerLanguage::Auto {
            latest.language
        } else {
            configured
        };
        let owners = vec![observed.scope.repository_id];
        let observation = &observed.observation;
        Ok(Some(Answer {
            outcome: "answered",
            text: Some(run_summary(&observed, language)),
            citations: vec![actions::citation(
                &observed.selection(),
                observation,
                &observation.fields(),
            )],
            evidence_repository_ids: owners.clone(),
            ..Answer::withheld(request, AnswerMode::Selected, None, owners)
        }))
    }
}

fn run_summary(observed: &actions::LocatedRun, language: AnswerLanguage) -> String {
    let observation = &observed.observation;
    let run = &observation.run;
    let repository = &observation.repository;
    let branch = &observed.scope.branch;
    let workflow = observation
        .workflow_name
        .clone()
        .unwrap_or_else(|| format!("Workflow #{}", run.workflow_id));
    let result = run.conclusion.as_deref().unwrap_or(&run.status);
    let (id, attempt, created) = (run.id, run.attempt, &run.created_at);
    match language {
        AnswerLanguage::Fr => format!(
            "Dernière exécution observée sur {repository} ({branch}) : {workflow}, run #{id}, tentative {attempt}. Créée le {created}. Résultat GitHub : {result}."
        ),
        AnswerLanguage::De => format!(
            "Letzter beobachteter Lauf in {repository} ({branch}): {workflow}, Lauf #{id}, Versuch {attempt}. Erstellt am {created}. GitHub-Ergebnis: {result}."
        ),
        AnswerLanguage::Es => format!(
            "Última ejecución observada en {repository} ({branch}): {workflow}, ejecución n.º {id}, intento {attempt}. Creada el {created}. Resultado de GitHub: {result}."
        ),
        AnswerLanguage::It => format!(
            "Ultima esecuzione osservata in {repository} ({branch}): {workflow}, esecuzione n. {id}, tentativo {attempt}. Creata il {created}. Risultato GitHub: {result}."
        ),
        AnswerLanguage::Pt => format!(
            "Última execução observada em {repository} ({branch}): {workflow}, execução n.º {id}, tentativa {attempt}. Criada em {created}. Resultado do GitHub: {result}."
        ),
        AnswerLanguage::Ja => format!(
            "{repository}（{branch}）で確認済みの最新実行: {workflow}、実行 #{id}、試行 {attempt}。作成日時: {created}。GitHub での結果: {result}。"
        ),
        AnswerLanguage::Zh => format!(
            "{repository}（{branch}）上最近一次已观测到的运行：{workflow}、运行 #{id}、第 {attempt} 次尝试。创建于 {created}。GitHub 结果：{result}。"
        ),
        AnswerLanguage::Auto | AnswerLanguage::En => format!(
            "Latest observed run on {repository} ({branch}): {workflow}, run #{id}, attempt {attempt}. Created at {created}. GitHub result: {result}."
        ),
    }
}
