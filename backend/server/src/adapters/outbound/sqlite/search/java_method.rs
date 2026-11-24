use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
    path::Path,
};

use rusqlite::OptionalExtension;
use tree_sitter::Node;

use super::{
    CANDIDATE_COLUMNS, Connection, Failure, SearchQuery, SqliteStore, hit, storage_failure,
};
use crate::{
    application::answers::{Excerpt, JavaEvidence},
    domain::java::parse_source,
};

const MAX_SOURCE_BYTES: u32 = 256 * 1024;
const MAX_METHOD_BYTES: usize = 600;
const MAX_NAMES: usize = 16;
const MAX_PASSAGES: usize = 2;

struct Mention<'a> {
    owners: Vec<&'a str>,
    methods: Vec<&'a str>,
    qualified: bool,
}

fn word(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '$')
}

fn mention(query: &SearchQuery) -> Option<Mention<'_>> {
    if query
        .terms()
        .iter()
        .any(|term| term.eq_ignore_ascii_case("readme"))
    {
        return None;
    }
    let mut explicit = None;
    for token in query
        .original()
        .split(|c: char| !word(c) && !matches!(c, '.' | '#'))
    {
        let token = token.trim_matches('.');
        let Some((owner, method)) = token.split_once(['#', '.']) else {
            continue;
        };
        if owner.as_bytes().first().is_some_and(u8::is_ascii_uppercase)
            && owner.chars().all(word)
            && method
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_lowercase)
            && method.chars().all(word)
        {
            if explicit.is_some_and(|other| other != (owner, method)) {
                return None;
            }
            explicit = Some((owner, method));
        }
    }
    // Explicit file selectors retain the established file-identity search.
    if query.referenced_files().next().is_some() {
        return None;
    }
    if let Some((owner, method)) = explicit {
        return Some(Mention {
            owners: vec![owner],
            methods: vec![method],
            qualified: true,
        });
    }
    let mut owners = Vec::new();
    let mut methods = Vec::new();
    for token in query.original().split(|character| !word(character)) {
        if token.len() < 2 {
            continue;
        }
        let list = if token.as_bytes().first().is_some_and(u8::is_ascii_uppercase) {
            &mut owners
        } else if token.as_bytes().first().is_some_and(u8::is_ascii_lowercase) {
            &mut methods
        } else {
            continue;
        };
        if !list.contains(&token) {
            list.push(token);
        }
        if owners.len() > MAX_NAMES {
            return None;
        }
    }
    // Typical questions can contain many short method-like words. Bound the
    // type lookup separately while matching exact tokens only against the AST.
    (!owners.is_empty() && !methods.is_empty()).then_some(Mention {
        owners,
        methods,
        qualified: false,
    })
}

pub(in crate::adapters::outbound::sqlite) fn resolve(
    store: &SqliteStore,
    query: &SearchQuery,
    allow_class_local: bool,
) -> Result<JavaEvidence, Failure> {
    let Some(mention) = mention(query) else {
        return Ok(JavaEvidence::default());
    };
    let names = serde_json::json!(mention.owners).to_string();
    let mut db = store.read()?;
    let tx = db.transaction().map_err(storage_failure)?;
    let owners = super::resolve_publications(
        &tx,
        query,
        &names,
        "scope.repository_id IS NOT NULL AND s.tracked_ref = 'refs/heads/' || scope.branch
         AND EXISTS (
             SELECT 1 FROM json_each(:names) n
             WHERE f.path = n.value || '.java'
                OR substr(f.path, -(length(n.value) + 6)) = '/' || n.value || '.java'
         )",
    )
    .map_err(storage_failure)?;
    let [(publication, source)] = owners.as_slice() else {
        return Ok(JavaEvidence::default());
    };
    read_method(
        &tx,
        *publication,
        source,
        query,
        &mention,
        allow_class_local,
    )
    .map_err(storage_failure)
}

struct Source {
    path: String,
    blob: String,
    text: String,
    owner: i64,
}

fn read_method(
    db: &Connection,
    publication: i64,
    source: &str,
    query: &SearchQuery,
    mention: &Mention<'_>,
    allow_class_local: bool,
) -> rusqlite::Result<JavaEvidence> {
    let file = db
        .query_row(
            "SELECT f.path, f.blob, b.content, scope.repository_id
         FROM source_snapshot_files f JOIN source_blobs b USING (collection, blob)
         JOIN github_code_scopes scope ON scope.collection = f.collection
         WHERE f.snapshot_id = ?1 AND f.source = ?2 AND length(CAST(b.content AS BLOB)) <= ?3",
            (publication, source, MAX_SOURCE_BYTES),
            |row| {
                Ok(Source {
                    path: row.get(0)?,
                    blob: row.get(1)?,
                    text: row.get(2)?,
                    owner: row.get(3)?,
                })
            },
        )
        .optional()?;
    let Some(file) = file else {
        return Ok(JavaEvidence::default());
    };
    let Some(owner) = Path::new(&file.path)
        .file_stem()
        .and_then(|name| name.to_str())
    else {
        return Ok(JavaEvidence::default());
    };
    let Some(tree) = parse_source(&file.text) else {
        return Ok(JavaEvidence::default());
    };
    let Some(body) = declared_body(tree.root_node(), &file.text, owner) else {
        return Ok(JavaEvidence::default());
    };
    let class_local = allow_class_local && !mention.qualified && distinctive_class(query, owner);
    let Some((method, class_local)) = selected_method(
        body,
        &file.text,
        &mention.methods,
        class_local.then_some((query, owner)),
    ) else {
        return Ok(JavaEvidence::default());
    };
    let (Ok(start), Ok(end)) = (u32::try_from(method.start), u32::try_from(method.end)) else {
        return Ok(JavaEvidence::default());
    };
    let passages = read_passages(db, publication, source, start, end, &file)?;
    Ok(JavaEvidence {
        passages,
        class_local,
    })
}

fn read_passages(
    db: &Connection,
    publication: i64,
    source: &str,
    start: u32,
    end: u32,
    file: &Source,
) -> rusqlite::Result<Vec<Excerpt>> {
    let mut stmt = db.prepare(&format!(
        "SELECT {CANDIDATE_COLUMNS}, d.content
         FROM documents d JOIN collections c ON c.name = d.collection
         WHERE d.publication_id = ?1 AND d.source = ?2 AND d.kind = 'current'
           AND d.passage_start < ?4 AND d.passage_start >= (
               SELECT max(anchor.passage_start) FROM documents anchor
               WHERE anchor.publication_id = ?1 AND anchor.source = ?2 AND anchor.kind = 'current'
                 AND anchor.passage_start <= ?3
           )
         ORDER BY d.passage_start LIMIT {}",
        MAX_PASSAGES + 1
    ))?;
    let mut rows = stmt.query((publication, source, start, end))?;
    let mut excerpts = Vec::new();
    let mut next = start;
    while let Some(row) = rows.next()? {
        let evidence = hit(row)?;
        let text: String = row.get(7)?;
        let Some(loc) = &evidence.location else {
            return Ok(Vec::new());
        };
        if excerpts.len() == MAX_PASSAGES
            || loc.path != file.path
            || loc.blob != file.blob
            || loc.start_byte.max(start) != next
            || loc.end_byte <= next
            || file
                .text
                .get(loc.start_byte as usize..loc.end_byte as usize)
                != Some(text.as_str())
        {
            return Ok(Vec::new());
        }
        next = loc.end_byte;
        excerpts.push(Excerpt {
            evidence,
            text,
            repository_id: Some(file.owner),
        });
    }
    Ok(if next >= end { excerpts } else { Vec::new() })
}

fn declared_body<'tree>(root: Node<'tree>, text: &str, owner: &str) -> Option<Node<'tree>> {
    let mut cursor = root.walk();
    let mut owners = root.named_children(&mut cursor).filter(|node| {
        matches!(
            node.kind(),
            "class_declaration" | "interface_declaration" | "record_declaration"
        ) && is_public(*node)
    });
    let node = owners.next()?;
    if owners.next().is_some() || node_name(node, text)? != owner {
        return None;
    }
    node.child_by_field_name("body")
}

fn selected_method(
    body: Node<'_>,
    text: &str,
    methods: &[&str],
    class_local: Option<(&SearchQuery, &str)>,
) -> Option<(Range<usize>, bool)> {
    let mut cursor = body.walk();
    let mut exact = body.named_children(&mut cursor).filter(|node| {
        node.kind() == "method_declaration"
            && node_name(*node, text).is_some_and(|name| methods.contains(&name))
    });
    if let Some(method) = exact.next() {
        return (exact.next().is_none() && bounded_public(method))
            .then(|| (method.byte_range(), false));
    }
    let (query, owner) = class_local?;
    ranked_class_method(body, text, query, owner).map(|span| (span, true))
}

fn bounded_public(method: Node<'_>) -> bool {
    is_public(method)
        && method.child_by_field_name("body").is_some()
        && method.byte_range().len() <= MAX_METHOD_BYTES
}

fn distinctive_class(query: &SearchQuery, owner: &str) -> bool {
    let quoted: Vec<_> = query.original().split('`').skip(1).collect();
    let literals: Vec<_> = quoted
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| pair[0])
        .collect();
    if quoted.len() % 2 != 0 || literals.iter().any(|&literal| literal != owner) {
        return false;
    }
    let compound = owner.bytes().skip(1).any(|b| b.is_ascii_uppercase())
        && owner.bytes().any(|b| b.is_ascii_lowercase());
    (literals.contains(&owner) || compound)
        && query
            .referenced_identifiers()
            .iter()
            .all(|value| *value == owner)
        && !query
            .original()
            .split(|c| !word(c))
            .filter(|part| !part.is_empty())
            .skip(1)
            .any(|part| part != owner && part.len() > 1 && part.as_bytes()[0].is_ascii_uppercase())
}

fn ranked_class_method(
    body: Node<'_>,
    text: &str,
    query: &SearchQuery,
    owner: &str,
) -> Option<Range<usize>> {
    let terms: BTreeSet<_> = query
        .terms()
        .iter()
        .map(|term| term.to_ascii_lowercase())
        .filter(|term| term.len() > 1 && !term.eq_ignore_ascii_case(owner))
        .collect();
    let mut cursor = body.walk();
    let mut best = None;
    let mut tied = false;
    for method in body
        .named_children(&mut cursor)
        .filter(|n| n.kind() == "method_declaration" && bounded_public(*n))
    {
        let extent = method.byte_range();
        let counts = matching_terms(text.get(extent.clone())?, &terms);
        if counts.len() < 2 {
            continue;
        }
        // Distinct matching terms matter first, then repeated, bounded use inside the method.
        let score = (
            counts.len(),
            counts.values().map(|n| (*n).min(3)).sum::<usize>(),
        );
        if best.as_ref().is_none_or(|(prior, _)| score > *prior) {
            best = Some((score, extent));
            tied = false;
        } else if best.as_ref().is_some_and(|(prior, _)| score == *prior) {
            tied = true;
        }
    }
    (!tied).then(|| best.map(|(_, extent)| extent)).flatten()
}

fn matching_terms(source: &str, terms: &BTreeSet<String>) -> BTreeMap<String, usize> {
    let mut matches = BTreeMap::new();
    let mut add = |part: &str| {
        let token = part.to_ascii_lowercase();
        if terms.contains(&token) {
            *matches.entry(token).or_default() += 1;
        }
    };
    for word in source
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
    {
        add(word);
        let mut start = 0;
        for (i, pair) in word.as_bytes().windows(2).enumerate() {
            if pair[0].is_ascii_lowercase() && pair[1].is_ascii_uppercase() {
                add(&word[start..=i]);
                start = i + 1;
            }
        }
        if start > 0 {
            add(&word[start..]);
        }
    }
    matches
}

#[cfg(test)]
fn declared_method(text: &str, owner: &str, methods: &[&str]) -> Option<Range<usize>> {
    let tree = parse_source(text)?;
    let body = declared_body(tree.root_node(), text, owner)?;
    selected_method(body, text, methods, None).map(|(span, _)| span)
}

fn node_name<'a>(node: Node<'_>, text: &'a str) -> Option<&'a str> {
    text.get(node.child_by_field_name("name")?.byte_range())
}

fn is_public(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    let Some(modifiers) = node
        .named_children(&mut cursor)
        .find(|n| n.kind() == "modifiers")
    else {
        return false;
    };
    let mut cursor = modifiers.walk();
    modifiers
        .children(&mut cursor)
        .any(|child| child.kind() == "public")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_named_class_and_member_keep_exact_case() -> Result<(), Box<dyn std::error::Error>> {
        let query = SearchQuery::parse("When does Option equals compare unrecognized values?")?;
        let selection = mention(&query).ok_or("plain type/member")?;
        assert!(selection.owners.contains(&"Option"));
        assert!(selection.methods.contains(&"equals"));
        for query in [
            "What does the README say about Option equals?",
            "How does Option equals work in src/other/Option.java?",
            "Compare Option.equals and Option.hashCode",
        ] {
            assert!(mention(&SearchQuery::parse(query)?).is_none());
        }
        let unit =
            "public class Option { public boolean equals(Object value) { return value == this; } }";
        let range = declared_method(unit, "Option", &selection.methods).ok_or("exact method")?;
        assert_eq!(
            &unit[range],
            "public boolean equals(Object value) { return value == this; }"
        );
        let overloaded = "public class Option {
           public boolean equals(Object value) { return value == this; }
           private boolean equals(String value) { return false; }
        }";
        assert!(declared_method(overloaded, "Option", &selection.methods).is_none());
        Ok(())
    }
    #[test]
    fn class_local_selection_declines_tied_or_ambiguous_declarations()
    -> Result<(), Box<dyn std::error::Error>> {
        let source = "public class WidgetState {
            public boolean once(Object value) { return value == null; }
            public boolean again(Object value) { return value == null; }
        }";
        let query = SearchQuery::parse("When WidgetState receives a null value, what happens?")?;
        let tree = parse_source(source).ok_or("fixture Java")?;
        let body = declared_body(tree.root_node(), source, "WidgetState").ok_or("class body")?;
        assert!(selected_method(body, source, &[], Some((&query, "WidgetState"))).is_none());
        assert!(
            selected_method(
                body,
                source,
                &["once", "again"],
                Some((&query, "WidgetState"))
            )
            .is_none()
        );
        Ok(())
    }
}
