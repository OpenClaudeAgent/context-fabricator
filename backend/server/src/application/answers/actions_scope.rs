//! Recognize a single explicitly named Actions attempt in an ordinary question.

use crate::{application::SearchQuery, domain::github::RepositoryName};

#[derive(Debug, Eq, PartialEq)]
pub struct AttemptTarget {
    pub repository: Option<String>,
    pub run_id: i64,
    pub attempt: u32,
}

pub(super) struct AttemptRequest {
    pub(super) target: Option<AttemptTarget>,
    pub(super) required: bool,
}

impl AttemptRequest {
    pub(super) fn parse(query: &SearchQuery) -> Option<Self> {
        let terms: Vec<_> = query
            .original()
            .split(|c: char| {
                !c.is_alphanumeric() && !matches!(c, '/' | '-' | '_' | '.' | '#' | '+')
            })
            .filter(|token| !token.is_empty())
            .collect();
        let referenced = url_target(query.original());
        let direct_observation = asks_observed_result(&terms, false);
        let named = named_target(&terms, direct_observation);
        let required = direct_observation
            && (!matches!(named, Mention::Absent) || asks_observed_result(&terms, true));
        let target = match (referenced, named) {
            (Mention::Several, _) | (_, Mention::Several) | (Mention::Absent, Mention::Absent) => {
                return None;
            }
            (Mention::One(Some(url)), Mention::One(Some(name)))
                if url.run_id == name.run_id
                    && url.attempt == name.attempt
                    && name
                        .repository
                        .as_ref()
                        .is_none_or(|repo| url.repository.as_ref() == Some(repo)) =>
            {
                Some(url)
            }
            (Mention::One(_), Mention::One(_)) => None,
            (Mention::One(target), _) | (_, Mention::One(target)) => target,
        };
        Some(Self { target, required })
    }
}

enum Mention {
    Absent,
    One(Option<AttemptTarget>),
    Several,
}

fn url_target(text: &str) -> Mention {
    let mut targets = text.split_whitespace().filter_map(|word| {
        let url = word.trim_matches([
            '(', ')', '<', '>', '[', ']', '`', '"', '\'', '?', '!', ',', '.',
        ]);
        let url = url.strip_prefix("https://github.com/")?;
        let (repository, suffix) = url.split_once("/actions/runs/")?;
        let (run_id, attempt) = suffix.split_once("/attempts/")?;
        if attempt.contains('/') {
            return None;
        }
        Some(target(repository_name(repository), run_id, attempt, true))
    });
    merge_mentions(&mut targets)
}

pub(super) fn canonical_attempt_link(url: &str) -> bool {
    matches!(url_target(url), Mention::One(Some(_)))
}

fn other_provider(suffix: &[&str]) -> bool {
    let [connector, name, rest @ ..] = suffix else {
        return false;
    };
    if !["on", "in", "from", "via", "sur", "dans", "chez"]
        .iter()
        .any(|word| connector.eq_ignore_ascii_case(word))
    {
        return false;
    }
    let name = name.trim_end_matches('.');
    ["gitlab", "jenkins", "circleci", "buildkite", "bitbucket"]
        .iter()
        .any(|provider| name.eq_ignore_ascii_case(provider))
        || (name.eq_ignore_ascii_case("azure")
            && rest
                .first()
                .is_some_and(|word| word.trim_end_matches('.').eq_ignore_ascii_case("devops")))
}

fn named_target(terms: &[&str], direct_observation: bool) -> Mention {
    let branded = terms
        .iter()
        .any(|word| word.eq_ignore_ascii_case("github") || word.eq_ignore_ascii_case("actions"));
    if !direct_observation && !branded {
        return Mention::Absent;
    }
    let positions = |markers: &'static [&'static str]| {
        terms.windows(2).enumerate().filter(move |(_, pair)| {
            markers.iter().any(|marker| same_word(pair[0], marker)) && numeric_token(pair[1])
        })
    };
    let mut run_ids = positions(&["run", "exécution"]);
    let mut attempts = positions(&["attempt", "tentative"]);
    let (Some((run_position, run)), Some((attempt_position, attempt))) =
        (run_ids.next(), attempts.next())
    else {
        return Mention::Absent;
    };
    if run_ids.next().is_some()
        || attempts.next().is_some()
        || numeric_continuation(terms, run_position + 2)
        || numeric_continuation(terms, attempt_position + 2)
    {
        return Mention::Several;
    }
    let after_pair = run_position.max(attempt_position) + 2;
    if !related_selectors(terms, run_position, attempt_position)
        || (!branded && other_provider(&terms[after_pair..]))
    {
        return Mention::Absent;
    }
    let mut repository = None;
    for (i, term) in terms.iter().enumerate() {
        if i.abs_diff(run_position) <= 3 || i.abs_diff(attempt_position) <= 3 {
            let Some(name) = repository_name(term) else {
                continue;
            };
            if repository
                .as_ref()
                .is_some_and(|previous: &String| !previous.eq_ignore_ascii_case(&name))
            {
                return Mention::Several;
            }
            repository = Some(name);
        }
    }
    Mention::One(target(repository, run[1], attempt[1], false))
}

fn related_selectors(terms: &[&str], run: usize, attempt: usize) -> bool {
    let connector = |word: &str| {
        ["in", "on", "for", "of", "de", "du", "sur"]
            .iter()
            .any(|part| word.eq_ignore_ascii_case(part))
    };
    let (first, second) = if run < attempt {
        (run, attempt)
    } else {
        (attempt, run)
    };
    match terms.get(first + 2..second) {
        Some([]) => true,
        Some([word]) => connector(word),
        Some([word, qualifier]) => {
            connector(word)
                && (["the", "l", "la", "le"]
                    .iter()
                    .any(|article| qualifier.eq_ignore_ascii_case(article))
                    || repository_name(qualifier).is_some())
        }
        _ => false,
    }
}

fn numeric_continuation(terms: &[&str], from: usize) -> bool {
    terms.get(from).is_some_and(|word| numeric_token(word))
        || terms.get(from..from + 2).is_some_and(|tail| {
            [
                "and", "or", "et", "ou", "vs", "versus", "with", "avec", "to", "à",
            ]
            .iter()
            .any(|connector| tail[0].eq_ignore_ascii_case(connector))
                && numeric_token(tail[1])
        })
}

fn numeric_token(word: &str) -> bool {
    let word = word.trim_end_matches('.');
    let digits = word.strip_prefix(['#', '-', '+']).unwrap_or(word);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn target(
    repository: Option<String>,
    run: &str,
    attempt: &str,
    canonical: bool,
) -> Option<AttemptTarget> {
    let digits = |text: &str| -> Option<(u64, bool)> {
        let value = if canonical {
            text
        } else {
            text.trim_end_matches('.').trim_start_matches('#')
        };
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some((value.parse().ok()?, value.starts_with('0')))
    };
    let (run, run_has_zero) = digits(run)?;
    let (attempt, attempt_has_zero) = digits(attempt)?;
    if run == 0
        || attempt == 0
        || (canonical && (run_has_zero || attempt_has_zero || repository.is_none()))
    {
        return None;
    }
    Some(AttemptTarget {
        repository,
        run_id: i64::try_from(run).ok()?,
        attempt: u32::try_from(attempt).ok()?,
    })
}

pub(super) fn repository_name(text: &str) -> Option<String> {
    let text = text.trim_end_matches('.');
    let name = RepositoryName::parse(text).ok()?;
    let full = format!("{}/{}", name.owner, name.name);
    (full == text).then(|| full.to_ascii_lowercase())
}

fn merge_mentions(targets: &mut impl Iterator<Item = Option<AttemptTarget>>) -> Mention {
    let Some(first) = targets.next() else {
        return Mention::Absent;
    };
    if targets.any(|other| other != first) {
        return Mention::Several;
    }
    Mention::One(first)
}

fn same_word(word: &str, expected: &str) -> bool {
    word.eq_ignore_ascii_case(expected) || (!expected.is_ascii() && word.to_lowercase() == expected)
}

fn asks_observed_result(terms: &[&str], url_only: bool) -> bool {
    const FORMS: &[&[&str]] = &[
        &["what", "did", "github", "report"],
        &["what", "was", "the", "conclusion"],
        &["what", "was", "the", "result"],
        &["what", "was", "the", "status"],
        &["what", "was", "the", "outcome"],
        &["what", "conclusion", "did", "github", "report"],
        &["what", "conclusion", "is", "recorded"],
        &["what", "status", "is", "recorded"],
        &["what", "outcome", "is", "recorded"],
        &["what", "result", "is", "recorded"],
        &["what", "happened"],
        &["quel", "est", "le", "statut"],
        &["quel", "était", "le", "statut"],
        &["quel", "est", "le", "résultat"],
        &["quel", "était", "le", "résultat"],
        &["quelle", "est", "la", "conclusion"],
        &["quelle", "était", "la", "conclusion"],
        &["quelle", "conclusion", "est", "enregistrée"],
        &["quel", "résultat", "est", "enregistré"],
        &["quel", "statut", "est", "enregistré"],
        &["quel", "résultat", "github"],
        &["quel", "statut", "github"],
        &["quelle", "conclusion", "github"],
    ];
    FORMS.iter().any(|form| {
        let matches = terms.get(..form.len()).is_some_and(|head| {
            head.iter()
                .zip(*form)
                .all(|(word, expected)| same_word(word, expected))
        });
        matches && observed_subject(&terms[form.len()..], url_only)
    })
}

// A test or parser can mention exact Actions identity without asking about the observation.
fn observed_subject(mut words: &[&str], url_only: bool) -> bool {
    let take = |words: &[&str], accepted: &[&str]| {
        usize::from(
            words
                .first()
                .is_some_and(|head| accepted.iter().any(|word| same_word(head, word))),
        )
    };
    if matches!(words, ["a-t-il", "enregistré", ..]) {
        words = &words[2..];
    }
    let connector = take(
        words,
        &[
            "of", "for", "in", "at", "on", "from", "de", "du", "dans", "sur", "pour",
        ],
    );
    if connector == 0 {
        return false;
    }
    words = &words[connector..];
    if matches!(words, ["https", host, ..] if host.starts_with("//github.com/")) {
        return true;
    }
    if url_only {
        return false;
    }
    words = &words[take(words, &["the", "l", "la", "le", "this"])..];
    words = &words[take(words, &["github"])..];
    words = &words[take(words, &["actions"])..];
    words = &words[take(words, &["workflow"])..];
    if words
        .first()
        .is_some_and(|word| repository_name(word).is_some())
    {
        words = &words[1..];
    }
    take(words, &["run", "exécution", "attempt", "tentative"]) == 1
        && words.get(1).is_some_and(|number| numeric_token(number))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_targets_use_positive_bounded_numbers_and_one_repository()
    -> Result<(), Box<dyn std::error::Error>> {
        for question in [
            "What did GitHub report for astral-sh/uv run 701 attempt 0?",
            "What did GitHub report for astral-sh/uv run 701 attempt -2?",
            "What did GitHub report for astral-sh/uv run 701 attempt 4294967296?",
            "What did GitHub report for astral-sh/uv run 9223372036854775808 attempt 2?",
            "What was the conclusion of https://github.com/astral-sh/uv/actions/runs/701/attempts/02?",
        ] {
            let query = SearchQuery::parse(question)?;
            let request = AttemptRequest::parse(&query).ok_or("invalid explicit selection")?;
            assert!(request.required && request.target.is_none(), "{question}");
        }
        let query = SearchQuery::parse(
            "Quel résultat GitHub a-t-il enregistré pour la tentative 8 du run 701 d’astral-sh/uv ?",
        )?;
        let request = AttemptRequest::parse(&query).ok_or("explicit French selection")?;
        assert!(request.required);
        assert_eq!(
            request.target,
            Some(AttemptTarget {
                repository: Some("astral-sh/uv".into()),
                run_id: 701,
                attempt: 8,
            })
        );
        Ok(())
    }

    #[test]
    fn comparisons_and_code_references_do_not_require_one_attempt()
    -> Result<(), Box<dyn std::error::Error>> {
        for question in [
            "Compare GitHub Actions run 701 attempt 1 and 2",
            "Compare GitHub Actions run 701 attempt 1 and attempt 2",
            "Which regex parses a link like https://github.com.evil.test/org/repo/actions/runs/701/attempts/2?",
            "Where does code called attempt 2 appear in the parser for GitHub Actions run 701?",
        ] {
            assert!(
                AttemptRequest::parse(&SearchQuery::parse(question)?).is_none(),
                "{question}"
            );
        }
        let query = SearchQuery::parse(
            "Which regex parses https://github.com/org/repo/actions/runs/701/attempts/2?",
        )?;
        let referenced = AttemptRequest::parse(&query).ok_or("canonical URL")?;
        assert!(!referenced.required);
        assert!(referenced.target.is_some());
        let parser_test = SearchQuery::parse(
            "What was the result of the parser test for https://github.com/org/repo/actions/runs/701/attempts/2?",
        )?;
        assert!(
            !AttemptRequest::parse(&parser_test)
                .ok_or("canonical URL")?
                .required
        );
        let parser_test = SearchQuery::parse(
            "What was the result of the parser test for GitHub Actions run 701 attempt 8?",
        )?;
        assert!(
            !AttemptRequest::parse(&parser_test)
                .ok_or("labelled reference")?
                .required
        );
        let parser_test = SearchQuery::parse(
            "What was the result of the run parser test for GitHub Actions run 701 attempt 8?",
        )?;
        assert!(
            !AttemptRequest::parse(&parser_test)
                .ok_or("labelled reference")?
                .required
        );
        Ok(())
    }

    #[test]
    fn unbranded_identity_requires_a_direct_observation_question()
    -> Result<(), Box<dyn std::error::Error>> {
        for question in [
            "What was the outcome of run 9431 attempt 2?",
            "What conclusion is recorded for run 9431, attempt 2?",
            "Quelle conclusion est enregistrée pour l’exécution 9431, tentative 2 ?",
            "What conclusion is recorded for run 9431 attempt 2 despite Jenkins logs?",
            "Quelle était la conclusion de la tentative 2 de l'exécution 9431 ?",
            "QUEL EST LE STATUT DE L’EXÉCUTION #9431, TENTATIVE #2 ?",
        ] {
            let query = SearchQuery::parse(question)?;
            let request = AttemptRequest::parse(&query).ok_or("direct observed target")?;
            assert!(request.required, "{question}");
            assert_eq!(
                request.target,
                Some(AttemptTarget {
                    repository: None,
                    run_id: 9431,
                    attempt: 2
                }),
                "{question}"
            );
        }
        for question in [
            "What was the result of the parcel parser for run 9431 attempt 2?",
            "What conclusion is recorded for the parcel parser in run 9431 attempt 2?",
            "Quelle conclusion est enregistrée pour le test de l’exécution 9431, tentative 2 ?",
            "Quelle est la conclusion du test pour l’exécution 9431, tentative 2 ?",
            "What was the result of GitLab run 9431 attempt 2?",
            "What was the result of run 9431 attempt 2 on Jenkins.",
            "What was the result of run 9431 attempt 2 on Azure DevOps?",
            "What was the outcome of run 9431 attempt 1 and 2?",
            "Compare l’exécution 9431, tentative 1 et tentative 2",
            "What was the result of 9431 2?",
        ] {
            let query = SearchQuery::parse(question)?;
            assert!(AttemptRequest::parse(&query).is_none(), "{question}");
        }
        Ok(())
    }

    #[test]
    fn direct_links_supply_the_repository_and_conflicting_identifiers_fail_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        for question in [
            "Quel est le statut de https://github.com/org/repo/actions/runs/701/attempts/2 ?",
            "What happened in https://github.com/org/repo/actions/runs/701/attempts/2?",
            "What was the status of GitHub Actions run 701 attempt 2 (https://github.com/org/repo/actions/runs/701/attempts/2)?",
        ] {
            let query = SearchQuery::parse(question)?;
            let request = AttemptRequest::parse(&query).ok_or("canonical target")?;
            assert!(request.required, "{question}");
            assert_eq!(
                request
                    .target
                    .and_then(|target| target.repository)
                    .as_deref(),
                Some("org/repo")
            );
        }
        let query = SearchQuery::parse(
            "What was the status of GitHub Actions run 701 attempt 3 (https://github.com/org/repo/actions/runs/701/attempts/2)?",
        )?;
        let request = AttemptRequest::parse(&query).ok_or("conflicting selectors")?;
        assert!(request.required && request.target.is_none());
        Ok(())
    }
}
