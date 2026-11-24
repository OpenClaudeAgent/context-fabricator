use rusqlite::{Connection, named_params};

use super::retrieval::{CANDIDATE_COLUMNS, hit};
use super::{CODE_COLLECTION, SqliteStore, scope_parameters, storage_failure};
use crate::application::{
    Failure, SearchHit, SearchQuery, answers::Excerpt, retrieval::MAX_RESULTS,
};

mod java_method;
mod readme;
pub(super) use java_method::resolve as resolve_java_method;

const SYMBOL_CANDIDATES: usize = 128;

#[derive(Clone, Copy)]
enum Selection<'a> {
    Text,
    Declarations,
    File(i64, &'a str, usize),
}

pub(super) fn fts_terms(query: &SearchQuery) -> String {
    query
        .terms()
        .iter()
        .map(|term| format!("\"{term}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

pub(super) fn readme_section(
    store: &SqliteStore,
    query: &SearchQuery,
) -> Result<Option<SearchHit>, Failure> {
    let Some(name) = query.markdown_readme_reference() else {
        return Ok(None);
    };
    let mut database = store.read()?;
    let snapshot = database.transaction().map_err(storage_failure)?;
    let files = resolve_publications(
        &snapshot,
        query,
        name,
        "lower(f.path) IN ('readme.md', 'readme.markdown')
         AND (lower(:names) = 'readme' OR lower(f.path) = lower(:names))",
    )
    .map_err(storage_failure)?;
    let [(publication, source)] = files.as_slice() else {
        return Ok(None);
    };
    readme::visible_section(&snapshot, query, *publication, source).map_err(storage_failure)
}

fn repository_words(query: &SearchQuery) -> Vec<&str> {
    query
        .original()
        .split(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '_' | '-' | '/' | '.'))
        .map(|word| word.trim_matches(['.', '/']))
        .filter(|word| !word.is_empty())
        .collect()
}

pub(super) fn authoritative_readme(
    store: &SqliteStore,
    query: &SearchQuery,
) -> Result<Option<Vec<Excerpt>>, Failure> {
    let Some(name) = query.readme_authority_reference() else {
        return Ok(None);
    };
    let mut database = store.read()?;
    let snapshot = database.transaction().map_err(storage_failure)?;
    let words = repository_words(query);
    let named = known_repositories(&snapshot, &words).map_err(storage_failure)?;
    if named.len() > 1 {
        return Ok(None);
    }
    let explicit = query.readme_authority_repository_hint();
    let explicit_resolves = explicit
        .map(|owner| known_repositories(&snapshot, &[owner]))
        .transpose()
        .map_err(storage_failure)?
        .is_none_or(|repos| !repos.is_empty() && repos == named);
    if !explicit_resolves
        || query.scope().is_some_and(|scope| {
            named
                .first()
                .is_some_and(|&repo| repo != scope.repository_id)
        })
    {
        return Ok(Some(Vec::new()));
    }
    let files = resolve_publications(
        &snapshot,
        query,
        name,
        "scope.repository_id IS NOT NULL AND s.tracked_ref = 'refs/heads/' || scope.branch
         AND lower(f.path) IN ('readme.md','readme.markdown')
         AND (lower(:names)='readme' OR lower(f.path)=lower(:names))",
    )
    .map_err(storage_failure)?;
    match files.as_slice() {
        [(publication, source)] => {
            let ranked = select(
                &snapshot,
                query,
                &fts_terms(query),
                Selection::File(*publication, source, 5),
            )
            .map_err(storage_failure)?;
            let priorities: Vec<_> = ranked.into_iter().map(|(hit, _)| hit).collect();
            readme::authority_passages(&snapshot, *publication, source, &priorities)
                .map(Some)
                .map_err(storage_failure)
        }
        [] => Ok(Some(Vec::new())),
        _ => Ok(None),
    }
}

fn known_repositories(database: &Connection, words: &[&str]) -> rusqlite::Result<Vec<i64>> {
    let words = serde_json::json!(words).to_string();
    database.prepare(
        "SELECT id FROM github_repositories
         WHERE EXISTS (SELECT 1 FROM json_each(?1) word
           WHERE lower(word.value) IN (lower(full_name), lower(substr(full_name,instr(full_name,'/')+1))))
         LIMIT 2"
    )?.query_map([words], |row| row.get(0))?.collect()
}

fn resolve_publications(
    database: &Connection,
    query: &SearchQuery,
    names: &str,
    path_filter: &str,
) -> rusqlite::Result<Vec<(i64, String)>> {
    let (repository_id, branch) = scope_parameters(query.scope());
    let mut statement = database.prepare(&format!(
        "WITH named_repositories AS (
             SELECT id FROM github_repositories r
             WHERE :repository_id IS NULL
               AND EXISTS (SELECT 1 FROM json_each(:words) word
                           WHERE lower(word.value) IN (lower(r.full_name),
                                 lower(substr(r.full_name, instr(r.full_name, '/') + 1))))
         )
         SELECT f.snapshot_id, f.source
         FROM source_snapshot_files f JOIN source_snapshots s ON s.id = f.snapshot_id
         LEFT JOIN github_code_scopes scope ON scope.collection = f.collection
         WHERE s.phase = 'published'
           AND (:repository_id IS NULL OR f.collection = {CODE_COLLECTION})
           AND (NOT EXISTS (SELECT 1 FROM named_repositories)
                OR ((SELECT count(*) FROM named_repositories) = 1
                    AND scope.repository_id = (SELECT id FROM named_repositories)))
           AND ({path_filter})
           AND EXISTS (SELECT 1 FROM documents d WHERE d.source = f.source
                       AND d.publication_id = f.snapshot_id AND d.kind = 'current')
         LIMIT 2"
    ))?;
    let words = serde_json::json!(repository_words(query)).to_string();
    statement.query_map(named_params! {
        ":repository_id": repository_id, ":branch": branch, ":names": names, ":words": words,
    }, |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
}

pub(super) fn rank(store: &SqliteStore, query: &SearchQuery) -> Result<Vec<SearchHit>, Failure> {
    let expression = fts_terms(query);
    let identifiers = query.referenced_identifiers();
    let mut database = store.read()?;
    let snapshot = database.transaction().map_err(storage_failure)?;
    let mut ranked = Vec::new();

    let mut paths = query.referenced_files().peekable();
    let has_file_reference = paths.peek().is_some();
    let mut resolved = Vec::new();
    for path in paths {
        let Some((publication, source)) =
            resolve_file(&snapshot, query, path).map_err(storage_failure)?
        else {
            continue;
        };
        let found = select(
            &snapshot,
            query,
            &expression,
            Selection::File(publication, &source, 1),
        )
        .map_err(storage_failure)?;
        if let Some((hit, _)) = found.into_iter().next() {
            append(&mut ranked, hit);
        }
        resolved.push(source);
    }

    if !identifiers.is_empty() && (!has_file_reference || !resolved.is_empty()) {
        let phrases = identifiers
            .iter()
            .map(|identifier| format!("\"{identifier}\""))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut symbols = select(
            &snapshot,
            query,
            &format!("({expression}) AND content : ({phrases})"),
            Selection::Declarations,
        )
        .map_err(storage_failure)?;
        symbols.retain(|(hit, _)| {
            hit.location
                .as_ref()
                .is_some_and(|location| declaration_language(&location.path))
                && (!has_file_reference || resolved.contains(&hit.source))
        });
        // Keep one definition per explicit symbol, leaving space for ordinary retrieval.
        for identifier in identifiers {
            if let Some((hit, _)) = symbols
                .iter()
                .find(|(_, content)| has_definition(content, identifier))
            {
                append(&mut ranked, hit.clone());
            }
        }
    }

    for (hit, _) in
        select(&snapshot, query, &expression, Selection::Text).map_err(storage_failure)?
    {
        if ranked.len() == MAX_RESULTS {
            break;
        }
        append(&mut ranked, hit);
    }
    snapshot.commit().map_err(storage_failure)?;
    Ok(ranked)
}

fn resolve_file(
    database: &Connection,
    query: &SearchQuery,
    path: &str,
) -> rusqlite::Result<Option<(i64, String)>> {
    let (repository_id, branch) = scope_parameters(query.scope());
    let mut statement = database.prepare(&format!(
        "SELECT files.snapshot_id, files.source
         FROM source_snapshot_files files
         JOIN source_snapshots publication ON publication.id = files.snapshot_id
         WHERE publication.phase = 'published'
           AND (:repository_id IS NULL OR files.collection = {CODE_COLLECTION})
           AND (files.path = :path
                OR (:basename AND substr(files.path, -(length(:path) + 1)) = '/' || :path))
           AND EXISTS (SELECT 1 FROM documents
                       WHERE source = files.source AND publication_id = files.snapshot_id)
         LIMIT 2"
    ))?;
    let mut rows = statement.query(named_params! {
        ":repository_id": repository_id, ":branch": branch,
        ":path": path, ":basename": !path.contains('/'),
    })?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let file = (row.get(0)?, row.get(1)?);
    Ok(rows.next()?.is_none().then_some(file))
}

fn append(ranked: &mut Vec<SearchHit>, hit: SearchHit) {
    if !ranked.iter().any(|previous| previous.same_evidence(&hit)) {
        ranked.push(hit);
    }
}

fn select(
    database: &Connection,
    query: &SearchQuery,
    expression: &str,
    selection: Selection<'_>,
) -> rusqlite::Result<Vec<(SearchHit, String)>> {
    let (repository_id, branch) = scope_parameters(query.scope());
    let current = "AND d.publication_id IS NOT NULL AND d.kind = 'current'";
    let (content, filter, limit, publication, source) = match selection {
        Selection::Declarations => ("d.content", current, SYMBOL_CANDIDATES, None, None),
        Selection::File(publication, source, limit) => {
            ("''", current, limit, Some(publication), Some(source))
        }
        Selection::Text => ("''", "", MAX_RESULTS, None, None),
    };
    // The FTS row may come from another owner of the same immutable passage.
    // Select the scoped owner before applying either candidate limit.
    let mut statement = database.prepare(&format!(
        "SELECT {CANDIDATE_COLUMNS}, {content}
         FROM documents_fts f
         JOIN documents canonical ON canonical.id = f.rowid
         JOIN documents d ON d.id = CASE WHEN :repository_id IS NULL THEN canonical.id
             ELSE (SELECT owned.id FROM documents owned
                   WHERE owned.collection = {CODE_COLLECTION}
                     AND owned.publication_id IS NOT NULL
                     AND (owned.source, owned.kind, owned.tracked_ref, owned.revision, owned.passage_start)
                       = (canonical.source, canonical.kind, canonical.tracked_ref, canonical.revision, canonical.passage_start))
             END
         JOIN collections c ON c.name = d.collection
         WHERE documents_fts MATCH :expression {filter}
           AND (:publication IS NULL OR (d.publication_id = :publication AND d.source = :source))
         ORDER BY bm25(documents_fts), d.source, d.revision, d.kind, d.tracked_ref, d.passage_start
         LIMIT {limit}"
    ))?;
    statement
        .query_map(
            named_params! {
                ":expression": expression, ":repository_id": repository_id, ":branch": branch,
                ":publication": publication, ":source": source,
            },
            |row| Ok((hit(row)?, row.get(7)?)),
        )?
        .collect()
}

fn identifier_character(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

fn declaration_language(path: &str) -> bool {
    matches!(
        std::path::Path::new(path)
            .extension()
            .and_then(std::ffi::OsStr::to_str),
        Some("rs" | "swift" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "go" | "zig")
    )
}

fn has_definition(content: &str, identifier: &str) -> bool {
    content.lines().any(|line| {
        // Favor simple code declarations. Other syntaxes remain searchable by BM25.
        let line = line.trim_start();
        let line = line
            .strip_prefix("pub(")
            .and_then(|visibility| visibility.split_once(')'))
            .map_or(line, |(_, definition)| definition.trim_start());
        let prefix_end = line
            .find(['"', '\'', '/', '*', '=', '{', '}', '#'])
            .unwrap_or(line.len());
        let mut terms = line[..prefix_end]
            .split(|character: char| !identifier_character(character))
            .filter(|term| !term.is_empty());
        terms
            .find(|term| {
                !matches!(
                    *term,
                    "pub"
                        | "private"
                        | "internal"
                        | "public"
                        | "package"
                        | "fileprivate"
                        | "export"
                        | "declare"
                        | "async"
                        | "unsafe"
                        | "final"
                )
            })
            .is_some_and(|term| {
                matches!(
                    term,
                    "const"
                        | "static"
                        | "let"
                        | "var"
                        | "fn"
                        | "func"
                        | "function"
                        | "struct"
                        | "class"
                        | "enum"
                        | "trait"
                        | "interface"
                )
            })
            && terms.next().is_some_and(|term| term == identifier)
    })
}
