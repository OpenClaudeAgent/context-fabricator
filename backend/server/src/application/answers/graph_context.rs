//! Exact Rust names opt into structural candidate discovery on the local publications.
use super::{AnswerError, EvidenceStore, Excerpt, MAX_SOURCES, SearchQuery};

fn selector(question: &str) -> Option<(&str, Option<&str>)> {
    let mut names = question
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
        .filter(|word| word.starts_with("crate::"));
    let symbol = names.next()?;
    if symbol.len() > 256
        || names.any(|other| other != symbol)
        || !symbol
            .split("::")
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_alphanumeric() || c == '_'))
    {
        return None;
    }
    let mut roots = question.split('`').skip(1).step_by(2).filter(|part| {
        std::path::Path::new(part)
            .extension()
            .is_some_and(|ext| ext == "rs")
    });
    let root = roots.next();
    if roots.any(|other| Some(other) != root) {
        return None;
    }
    Some((symbol, root))
}

pub(super) fn contribute(
    store: &dyn EvidenceStore,
    query: &SearchQuery,
    passages: &mut Vec<Excerpt>,
) -> Result<bool, AnswerError> {
    let Some((symbol, root)) = selector(query.original()) else {
        return Ok(false);
    };
    let mut graph = store.graph_passages(symbol, root, query.scope())?;
    if graph.is_empty() {
        return Ok(false);
    }
    for passage in passages.drain(..) {
        let head = &graph[0];
        if passage.repository_id == head.repository_id
            && passage.evidence.revision == head.evidence.revision
            && passage.evidence.tracked_ref == head.evidence.tracked_ref
            && passage.evidence.kind == "current"
            && !graph
                .iter()
                .any(|previous| previous.evidence.same_evidence(&passage.evidence))
        {
            graph.push(passage);
        }
    }
    graph.truncate(MAX_SOURCES);
    *passages = graph;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_symbol_and_optional_root_work_in_plain_questions() {
        assert_eq!(
            selector("Where is crate::orders::submit called?"),
            Some(("crate::orders::submit", None))
        );
        assert_eq!(
            selector("Qui appelle `crate::orders::submit` dans `shop/src/lib.rs` ?"),
            Some(("crate::orders::submit", Some("shop/src/lib.rs")))
        );
        assert_eq!(selector("compare crate::a and crate::b"), None);
        assert_eq!(selector("Where is submit called?"), None);
    }
}
