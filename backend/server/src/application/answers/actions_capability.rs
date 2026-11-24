//! The Actions capture contains run attempts, with no job or step observations.

use crate::application::SearchQuery;

use super::actions_scope::{canonical_attempt_link, repository_name};

pub(super) fn needs_job_observation(query: &SearchQuery, selected: bool) -> bool {
    let text = query
        .original()
        .to_lowercase()
        .replace(['’', '‘'], "'")
        .replace(['“', '”', '«', '»'], "\"");
    let (prefixed_run, body) =
        text.rsplit_once(',')
            .map_or((false, text.as_str()), |(prefix, suffix)| {
                let terms = words(prefix);
                if run_context(&terms) {
                    return (true, suffix);
                }
                match terms.as_slice() {
                    ["using", "only", "that" | "the", "run", "record"] => (selected, suffix),
                    ["using", "only", rest @ ..] if run_reference(rest) => (true, suffix),
                    _ => (false, text.as_str()),
                }
            });
    let words = words(body);
    let Some((subject, outcome_after)) = outcome_subject(&words) else {
        return false;
    };
    let Some(after_subject) = after_job_or_step(subject) else {
        return false;
    };
    let tail = match outcome_after {
        OutcomeAfter::None => after_subject,
        OutcomeAfter::English => match after_subject {
            ["succeed" | "fail" | "pass", rest @ ..] => rest,
            _ => return false,
        },
        OutcomeAfter::French => match after_subject {
            ["a-t-il" | "a-t-elle", "réussi" | "échoué", rest @ ..] => rest,
            _ => return false,
        },
    };
    ((selected || prefixed_run) && tail.is_empty()) || run_context(tail)
}

fn words(text: &str) -> Vec<&str> {
    text.split_whitespace()
        .map(|word| word.trim_end_matches([',', '.', '?', '!']))
        .filter(|word| !word.is_empty())
        .collect()
}

enum OutcomeAfter {
    None,
    English,
    French,
}

fn outcome_subject<'a>(words: &'a [&str]) -> Option<(&'a [&'a str], OutcomeAfter)> {
    let subject = match words {
        [
            "what",
            "was",
            "the",
            "status" | "conclusion" | "result" | "outcome",
            "of" | "for",
            rest @ ..,
        ]
        | [
            "what",
            "status" | "conclusion" | "result" | "outcome",
            "is" | "was",
            "recorded",
            "for",
            rest @ ..,
        ]
        | [
            "quel",
            "est" | "était",
            "le",
            "résultat" | "statut",
            "de" | "du" | "pour",
            rest @ ..,
        ]
        | [
            "quelle",
            "est" | "était",
            "la",
            "conclusion",
            "de" | "pour",
            rest @ ..,
        ]
        | [
            "quel" | "quelle",
            "conclusion" | "résultat" | "statut",
            "est",
            "enregistré" | "enregistrée",
            "pour",
            rest @ ..,
        ] => rest,
        ["did", rest @ ..] => return Some((rest, OutcomeAfter::English)),
        ["le" | "la", rest @ ..] => return Some((rest, OutcomeAfter::French)),
        _ => return None,
    };
    Some((subject, OutcomeAfter::None))
}

fn after_job_or_step<'a>(words: &'a [&str]) -> Option<&'a [&'a str]> {
    let words = match words {
        ["the" | "la" | "le", rest @ ..] => rest,
        _ => words,
    };
    let (kind, after) = match words {
        [kind @ ("job" | "step" | "étape" | "l'étape"), rest @ ..] => (*kind, after_name(rest)?),
        _ => match after_name(words)? {
            [kind @ ("job" | "step" | "étape"), rest @ ..] => (*kind, rest),
            _ => return None,
        },
    };
    match (kind, after) {
        ("step" | "étape" | "l'étape", ["of" | "in" | "du", "job", rest @ ..]) => {
            after_name(rest)
        }
        _ => Some(after),
    }
}

fn after_name<'a>(words: &'a [&str]) -> Option<&'a [&'a str]> {
    let (first, rest) = words.split_first()?;
    if let Some(delimiter) = first
        .chars()
        .next()
        .filter(|ch| matches!(ch, '`' | '"' | '\''))
    {
        let open = &first[1..];
        if let Some(name) = open.strip_suffix(delimiter) {
            return (!name.is_empty()).then_some(rest);
        }
        let close = rest.iter().position(|word| word.ends_with(delimiter))?;
        if open.is_empty() && close == 0 && rest[close].len() == delimiter.len_utf8() {
            return None;
        }
        return Some(&rest[close + 1..]);
    }
    (first.chars().any(char::is_alphanumeric)
        && first
            .chars()
            .all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_')))
    .then_some(rest)
}

fn run_context(words: &[&str]) -> bool {
    let [
        "in" | "of" | "for" | "during" | "de" | "du" | "dans" | "pour" | "lors",
        rest @ ..,
    ] = words
    else {
        return false;
    };
    run_reference(rest)
}

fn run_reference(words: &[&str]) -> bool {
    let rest = match words {
        ["de" | "the", rest @ ..] => rest,
        _ => words,
    };
    let rest = match rest {
        ["recorded", rest @ ..] => rest,
        _ => rest,
    };
    if let [url] = rest {
        return canonical_attempt_link(url);
    }
    let rest = match rest {
        ["github", "actions", rest @ ..] | ["actions", rest @ ..] => rest,
        _ => rest,
    };
    let rest = match rest {
        ["workflow", rest @ ..] => rest,
        _ => rest,
    };
    let rest = match rest {
        [repo, rest @ ..] if run_repository(repo) => rest,
        _ => rest,
    };
    let ["run" | "exécution" | "l'exécution", run, rest @ ..] = rest else {
        return false;
    };
    if positive_id::<i64>(run).is_none() {
        return false;
    }
    let rest = match rest {
        ["attempt" | "tentative", attempt, rest @ ..] if positive_id::<u32>(attempt).is_some() => {
            rest
        }
        _ => rest,
    };
    rest.is_empty() || matches!(rest, ["in" | "of" | "de" | "sur", repo] if run_repository(repo))
}

fn run_repository(word: &str) -> bool {
    repository_name(word).is_some()
        && ![".md", ".json", ".log", ".txt", ".yml", ".yaml"]
            .iter()
            .any(|extension| word.ends_with(extension))
}

fn positive_id<T: std::str::FromStr + PartialOrd + Default>(word: &str) -> Option<T> {
    let digits = word.strip_prefix('#').unwrap_or(word);
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value: T = digits.parse().ok()?;
    (value > T::default()).then_some(value)
}
