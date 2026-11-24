use std::{collections::HashSet, ops::Range, path::Path};

use super::{AnswerError, Answers, CodeScope, Excerpt, SearchHit, SearchQuery};
use crate::domain::corpus::snapshot::Location;

mod syntax;
use syntax::{declaration_symbol, has_declaration, imports, module_path, usage_window};

pub(super) struct LinkedUsage {
    pub(super) excerpt: Excerpt,
    pub(super) visible: Range<usize>,
    pub(super) usage: Range<usize>,
    pub(super) declaration: Range<usize>,
    pub(super) symbol: String,
    pub(super) scope: CodeScope,
}

/// Find an executable use of an unambiguous integer constant in the same snapshot.
pub(super) fn find(
    answers: &Answers,
    declaration: &Excerpt,
) -> Result<Option<LinkedUsage>, AnswerError> {
    let Some(owner) = declaration.repository_id else {
        return Ok(None);
    };
    let head = &declaration.evidence;
    if head.kind != "current"
        || !is_rust(head)
        || head
            .location
            .as_ref()
            .is_none_or(|location| location.start_byte != 0)
    {
        return Ok(None);
    }
    let Some(branch) = head
        .tracked_ref
        .as_deref()
        .and_then(|reference| reference.strip_prefix("refs/heads/"))
    else {
        return Ok(None);
    };
    let Some((symbol, declaration_range)) = declaration_symbol(&declaration.text) else {
        return Ok(None);
    };
    let Ok(query) = SearchQuery::parse_scoped(symbol, Some(owner), Some(branch.to_owned())) else {
        return Ok(None);
    };
    let hits: Vec<_> = answers
        .repository
        .search(&query)
        .map_err(|_| AnswerError::Storage)?
        .into_iter()
        .filter(|candidate| same_snapshot(head, candidate))
        .collect();
    let mut excerpts = answers.evidence.excerpts(&hits, query.scope())?;
    excerpts.retain(|excerpt| {
        excerpt.repository_id == Some(owner)
            && same_snapshot(head, &excerpt.evidence)
            && hits.iter().any(|hit| hit.same_evidence(&excerpt.evidence))
    });
    // The search is bounded; a second declaration in any returned passage makes
    // the meaning of an unqualified use ambiguous.
    if excerpts.iter().any(|excerpt| {
        !excerpt.evidence.same_evidence(head) && has_declaration(&excerpt.text, symbol)
    }) {
        return Ok(None);
    }
    let Some(anchor) = head.location.as_ref() else {
        return Ok(None);
    };
    let Some((index, visible, usage)) = select_usage(&excerpts, anchor, symbol) else {
        return Ok(None);
    };
    Ok(excerpts.into_iter().nth(index).map(|excerpt| LinkedUsage {
        excerpt,
        visible,
        usage,
        declaration: declaration_range,
        symbol: symbol.to_owned(),
        scope: CodeScope {
            repository_id: owner,
            branch: branch.to_owned(),
        },
    }))
}

fn select_usage(
    excerpts: &[Excerpt],
    anchor: &Location,
    symbol: &str,
) -> Option<(usize, Range<usize>, Range<usize>)> {
    let (root, module) = module_path(&anchor.path)?;
    let import = format!(
        "crate::{}{}",
        if module.is_empty() {
            String::new()
        } else {
            format!("{module}::")
        },
        symbol
    );
    let imported = excerpts
        .iter()
        .filter_map(|excerpt| {
            let location = excerpt.evidence.location.as_ref()?;
            (location.start_byte == 0 && imports(&excerpt.text, &import)).then_some((
                excerpt.evidence.source.as_str(),
                location.path.as_str(),
                location.blob.as_str(),
            ))
        })
        .collect::<HashSet<_>>();
    let mut best: Option<(usize, Range<usize>, Range<usize>, usize)> = None;
    for (index, excerpt) in excerpts.iter().enumerate() {
        let Some(location) = excerpt.evidence.location.as_ref() else {
            continue;
        };
        let Some((candidate_root, _)) = module_path(&location.path) else {
            continue;
        };
        if candidate_root != root {
            continue;
        }
        let same_file = location.path == anchor.path && location.blob == anchor.blob;
        let file = (
            excerpt.evidence.source.as_str(),
            location.path.as_str(),
            location.blob.as_str(),
        );
        if !same_file && !imported.contains(&file) {
            continue;
        }
        let Some((visible, usage, score)) = usage_window(&excerpt.text, symbol) else {
            continue;
        };
        if best
            .as_ref()
            .is_none_or(|(_, _, _, previous)| score > *previous)
        {
            best = Some((index, visible, usage, score));
        }
    }
    best.map(|(index, visible, usage, _)| (index, visible, usage))
}

fn is_rust(hit: &SearchHit) -> bool {
    hit.location.as_ref().is_some_and(|location| {
        Path::new(&location.path)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"))
    })
}

fn same_snapshot(head: &SearchHit, candidate: &SearchHit) -> bool {
    candidate.kind == "current"
        && candidate.revision == head.revision
        && candidate.tracked_ref == head.tracked_ref
        && is_rust(candidate)
}
