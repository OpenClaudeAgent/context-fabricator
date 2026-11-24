use std::ops::Range;

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use rusqlite::OptionalExtension;

use super::{CANDIDATE_COLUMNS, Connection, SearchHit, SearchQuery, hit};

const MAX_MARKDOWN_BYTES: u32 = 256 * 1024;

pub(super) fn visible_section(
    db: &Connection,
    query: &SearchQuery,
    publication: i64,
    source: &str,
) -> rusqlite::Result<Option<SearchHit>> {
    let file: Option<(String, String, String)> = db
        .query_row(
            "SELECT f.path, f.blob, b.content
             FROM source_snapshot_files f JOIN source_blobs b USING (collection, blob)
             WHERE f.snapshot_id = ?1 AND f.source = ?2
               AND lower(f.path) IN ('readme.md', 'readme.markdown')
               AND length(CAST(b.content AS BLOB)) <= ?3",
            (publication, source, MAX_MARKDOWN_BYTES),
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((path, blob, text)) = file else {
        return Ok(None);
    };
    let Some(section) = matching_heading(query, &text) else {
        return Ok(None);
    };
    let offset = u32::try_from(section.start).unwrap_or(u32::MAX);
    let passage: Option<(SearchHit, String)> = db
        .query_row(
            &format!(
                "SELECT {CANDIDATE_COLUMNS}, d.content
                 FROM documents d JOIN collections c ON c.name = d.collection
                 WHERE d.publication_id = ?1 AND d.source = ?2 AND d.kind = 'current'
                   AND d.passage_start <= ?3
                 ORDER BY d.passage_start DESC LIMIT 1"
            ),
            (publication, source, offset),
            |row| Ok((hit(row)?, row.get(7)?)),
        )
        .optional()?;
    // The parser chooses a source offset. The existing passage supplies the bytes and
    // identity sent to Answers, and must still cover the entire rendered heading.
    Ok(passage.and_then(|(hit, content)| {
        hit.location.as_ref().and_then(|location| {
            let span = location.start_byte as usize..location.end_byte as usize;
            (location.blob == blob
                && location.path == path
                && span.start <= section.start
                && section.end <= span.end
                && text.get(span) == Some(content.as_str()))
            .then_some(())
        })?;
        Some(hit)
    }))
}

fn matching_heading(query: &SearchQuery, text: &str) -> Option<Range<usize>> {
    let terms: Vec<_> = query
        .terms()
        .iter()
        .map(|term| term.to_lowercase())
        .collect();
    let mut seen_heading = false;
    let mut active = false;
    let mut rendered = String::new();
    let mut best = None;
    let mut best_score = 0;
    for (event, span) in Parser::new(text).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                active = seen_heading || level != HeadingLevel::H1;
                seen_heading = true;
                rendered.clear();
            }
            Event::Text(text) | Event::Code(text) if active => rendered.push_str(&text),
            Event::SoftBreak | Event::HardBreak if active => rendered.push(' '),
            Event::End(TagEnd::Heading(_)) if active => {
                active = false;
                let words: Vec<_> = rendered
                    .split(|c: char| !c.is_alphanumeric())
                    .filter(|word| !word.is_empty())
                    .collect();
                // Require the complete heading. A common word such as "the" alone
                // does not make an unrelated section relevant.
                if words.iter().any(|word| word.chars().count() >= 4)
                    && words
                        .iter()
                        .all(|word| terms.contains(&word.to_lowercase()))
                {
                    match words.len().cmp(&best_score) {
                        std::cmp::Ordering::Greater => {
                            best_score = words.len();
                            best = Some(span);
                        }
                        std::cmp::Ordering::Equal => best = None,
                        std::cmp::Ordering::Less => {}
                    }
                }
            }
            _ => {}
        }
    }
    best
}

pub(super) fn authority_passages(
    db: &Connection,
    publication: i64,
    source: &str,
    priorities: &[SearchHit],
) -> rusqlite::Result<Vec<crate::application::answers::Excerpt>> {
    let starts: Vec<_> = priorities
        .iter()
        .filter_map(SearchHit::passage_start)
        .collect();
    let starts = serde_json::json!(starts).to_string();
    let mut statement = db.prepare(&format!(
        "SELECT {CANDIDATE_COLUMNS},d.content,f.path,f.blob,scope.repository_id,d.passage_start,
                substr(CAST(b.content AS BLOB),d.passage_start+1,length(CAST(d.content AS BLOB)))
         FROM documents d JOIN collections c ON c.name=d.collection
         JOIN source_snapshot_files f ON f.snapshot_id=d.publication_id AND f.source=d.source
         JOIN source_blobs b ON b.collection=f.collection AND b.blob=f.blob
         JOIN github_code_scopes scope ON scope.collection=f.collection
         WHERE d.publication_id=?1 AND d.source=?2 AND d.kind='current'
           AND (d.passage_start IN (SELECT value FROM json_each(?3))
             OR d.passage_start IN (
               SELECT first.passage_start FROM documents first
               WHERE first.publication_id=?1 AND first.source=?2 AND first.kind='current'
               ORDER BY first.passage_start LIMIT 5
             ))
         ORDER BY d.passage_start"
    ))?;
    let mut windows = statement.query((publication, source, starts))?;
    let mut all = Vec::new();
    while let Some(row) = windows.next()? {
        let evidence = hit(row)?;
        let text: String = row.get(7)?;
        let path: String = row.get(8)?;
        let blob: String = row.get(9)?;
        let owner: i64 = row.get(10)?;
        let start: u32 = row.get(11)?;
        let actual: Vec<u8> = row.get(12)?;
        if evidence.location.as_ref().is_none_or(|location| {
            location.blob != blob
                || location.path != path
                || location.start_byte != start
                || location.end_byte.checked_sub(start).map(|n| n as usize) != Some(text.len())
                || actual != text.as_bytes()
        }) {
            return Ok(Vec::new());
        }
        all.push(crate::application::answers::Excerpt {
            evidence,
            text,
            repository_id: Some(owner),
        });
    }
    all.sort_by_key(|item| {
        priorities
            .iter()
            .position(|e| e.same_evidence(&item.evidence))
            .unwrap_or(usize::MAX)
    });
    all.truncate(5);
    Ok(all)
}
