//! Resolve structural hints and current-code passage ownership in one read snapshot.
use rusqlite::{Connection, OptionalExtension, params};

use super::{AnswerError, CodeScope, Excerpt, SqliteStore};
use crate::adapters::outbound::sqlite::retrieval::{CANDIDATE_COLUMNS, hit};
use crate::application::github::graph::Hit;
use crate::domain::corpus::snapshot::Location;
use crate::domain::github::graph::PROFILE;

struct Owner {
    repository: i64,
    branch: String,
    root: String,
    revision: String,
    publication: i64,
    definition: Hit,
}

fn storage(_: rusqlite::Error) -> AnswerError {
    AnswerError::Storage
}

pub(super) fn resolve(
    store: &SqliteStore,
    symbol: &str,
    root: Option<&str>,
    scope: Option<&CodeScope>,
) -> Result<Vec<Excerpt>, AnswerError> {
    let mut db = store.read().map_err(|_| AnswerError::Storage)?;
    let tx = db.transaction().map_err(storage)?;
    let mut q = tx.prepare(
        "SELECT e.repository_id,e.branch,k.value->>'path',p.revision,s.id,o.record
         FROM github_current_eligible e JOIN github_repositories r ON r.id=e.repository_id
         JOIN github_graph_publications p ON (p.repository_id,p.branch)=(e.repository_id,e.branch)
         JOIN source_snapshots s ON s.collection=e.collection AND s.phase='published' AND s.revision=p.revision
         CROSS JOIN json_each(p.roots) k
         JOIN github_graph_occurrences o ON (o.repository_id,o.branch,o.root,o.qualified_name)
             =(e.repository_id,e.branch,k.value->>'path',?1)
         WHERE r.current_code AND r.code_graph AND (?2 IS NULL OR k.value->>'path'=?2)
         AND (?3 IS NULL OR (e.repository_id,e.branch)=(?3,?4))
         AND p.profile=?5 AND o.record->>'role'='definition' LIMIT 2"
    ).map_err(storage)?;
    let mut rows = q
        .query(params![
            symbol,
            root,
            scope.map(|s| s.repository_id),
            scope.map(|s| s.branch.as_str()),
            PROFILE,
        ])
        .map_err(storage)?;
    let Some(row) = rows.next().map_err(storage)? else {
        return Ok(vec![]);
    };
    let owner = Owner {
        repository: row.get(0).map_err(storage)?,
        branch: row.get(1).map_err(storage)?,
        root: row.get(2).map_err(storage)?,
        revision: row.get(3).map_err(storage)?,
        publication: row.get(4).map_err(storage)?,
        definition: parse(&row.get::<_, String>(5).map_err(storage)?)?,
    };
    if rows.next().map_err(storage)?.is_some() {
        return Ok(vec![]);
    }
    let definition = &owner.definition;
    if definition.resolution != "definition" || definition.qualified_name.as_deref() != Some(symbol)
    {
        return Ok(vec![]);
    }
    let Some(head) = passage(&tx, &owner, &definition.site, &definition.name)? else {
        return Ok(vec![]);
    };
    let mut passages = vec![head];
    // A single call and import plus their definition leave slots for textual retrieval.
    for role in ["call", "import"] {
        let record: Option<String> = tx.query_row(
            "SELECT record FROM github_graph_occurrences WHERE repository_id=?1 AND branch=?2 AND root=?3 AND qualified_name=?4
             AND record->>'role'=?5 AND record->>'resolution'='resolved' ORDER BY ordinal LIMIT 1",
            params![owner.repository,owner.branch,owner.root,symbol,role], |row|row.get(0)).optional().map_err(storage)?;
        if let Some(record) = record {
            let edge = parse(&record)?;
            if edge
                .target
                .as_ref()
                .is_some_and(|target| target.same_range(&definition.site))
                && let Some(site) = passage(&tx, &owner, &edge.site, &edge.name)?
                && !passages
                    .iter()
                    .any(|p| p.evidence.same_evidence(&site.evidence))
            {
                passages.push(site);
            }
        }
    }
    Ok(passages)
}

fn parse(record: &str) -> Result<Hit, AnswerError> {
    serde_json::from_str(record).map_err(|_| AnswerError::Storage)
}

fn passage(
    db: &Connection,
    owner: &Owner,
    site: &Location,
    name: &str,
) -> Result<Option<Excerpt>, AnswerError> {
    let sql = format!("SELECT {CANDIDATE_COLUMNS},d.content FROM documents d JOIN collections c ON c.name=d.collection
        WHERE d.publication_id=?1 AND d.source=?2 AND d.revision=?3 AND d.kind='current' AND d.tracked_ref=?4
        AND d.passage_start<=?5 ORDER BY d.passage_start DESC LIMIT 1");
    let excerpt = db
        .query_row(
            &sql,
            params![
                owner.publication,
                format!("github:{}:{}", owner.repository, site.path),
                owner.revision,
                format!("refs/heads/{}", owner.branch),
                site.start_byte
            ],
            |row| {
                Ok(Excerpt {
                    evidence: hit(row)?,
                    text: row.get(7)?,
                    repository_id: Some(owner.repository),
                })
            },
        )
        .optional()
        .map_err(storage)?;
    Ok(excerpt.filter(|e| {
        e.evidence.location.as_ref().is_some_and(|loc| {
            loc.blob == site.blob
                && loc.path == site.path
                && loc.file_url == site.file_url
                && loc.start_byte <= site.start_byte
                && site.end_byte <= loc.end_byte
                && loc.start_line <= site.start_line
                && site.end_line <= loc.end_line
                && e.text.get(
                    (site.start_byte - loc.start_byte) as usize
                        ..(site.end_byte - loc.start_byte) as usize,
                ) == Some(name)
        })
    }))
}
