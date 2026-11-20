use std::collections::HashSet;
use std::error::Error;
use std::ops::Range;
use std::path::Path;

use ctxfab_inference::MAX_PASSAGE_BYTES;
use ctxfab_server::domain::corpus::{passage::passages, snapshot::Location};
use rusqlite::{Connection, OpenFlags, params};
use serde::Deserialize;
use serde_json::json;

use super::Reference;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hints {
    path: String,
    boundaries: Vec<usize>,
    insertion: String,
    edited_boundaries: Vec<usize>,
}

pub(super) fn prepare(
    database: &Path,
    corpus: &Path,
    manifest: &Path,
    references: &[Reference],
    use_structure: bool,
) -> Result<(), Box<dyn Error>> {
    let hints: Vec<Hints> = serde_json::from_slice(&std::fs::read(manifest)?)?;
    assert_eq!(hints.len(), references.len());
    let mut connection = Connection::open(database)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    let tx = connection.transaction()?;
    for reference in references {
        let plan = hints
            .iter()
            .find(|hint| hint.path == reference.path)
            .ok_or("missing structural hints")?;
        let original = std::fs::read_to_string(corpus.join(&reference.file))?;
        file_comparison(&tx, reference, plan, &original, use_structure)?;
    }
    tx.commit()?;
    Ok(())
}

fn file_comparison(
    database: &Connection,
    reference: &Reference,
    plan: &Hints,
    original: &str,
    use_structure: bool,
) -> Result<(), Box<dyn Error>> {
    let budget = (MAX_PASSAGE_BYTES - (reference.path.len() + 1)).min(2_048);
    let source = format!("github:{}:{}", reference.repository_id, reference.path);
    let (publication, collection, bytes): (i64, String, String) = database.query_row(
        "SELECT s.id, s.collection, b.content FROM source_snapshots s
         JOIN source_snapshot_files f ON f.snapshot_id = s.id
         JOIN source_blobs b ON (b.collection, b.blob) = (s.collection, f.blob)
         WHERE s.phase = 'published' AND s.revision = ?1
           AND f.source = ?2 AND f.path = ?3 AND f.blob = ?4",
        params![reference.commit, source, reference.path, reference.blob],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(bytes.as_bytes(), original.as_bytes());
    let original_ranges = ranges(original, budget, &[])?;
    assert_projection(database, &source, publication, original, &original_ranges)?;
    let updated = format!("{}{original}", plan.insertion);
    let (before, after) = if use_structure {
        let proposed = ranges(original, budget, &plan.boundaries)?;
        assert_ne!(proposed, original_ranges, "structural hints had no effect");
        install_passages(
            database,
            reference,
            &source,
            &collection,
            publication,
            original,
            &proposed,
        )?;
        assert_projection(database, &source, publication, original, &proposed)?;
        (proposed, ranges(&updated, budget, &plan.edited_boundaries)?)
    } else {
        (original_ranges, ranges(&updated, budget, &[])?)
    };
    let reusable: HashSet<_> = nonblank(original, &before)
        .map(|range| &original[range.clone()])
        .collect();
    let changed = nonblank(&updated, &after)
        .filter(|range| !reusable.contains(&updated[(*range).clone()]))
        .count();
    eprintln!(
        "Code chunking input {}",
        json!({
            "path":reference.path, "sourceBytes":original.len(), "budgetBytes":budget,
            "passages":nonblank(original,&before).count(), "editedPassages":nonblank(&updated,&after).count(),
            "newEmbeddingInputs":changed, "insertedBytes":plan.insertion.len(),
        })
    );
    Ok(())
}

fn ranges(
    source: &str,
    budget: usize,
    hints: &[usize],
) -> Result<Vec<Range<usize>>, Box<dyn Error>> {
    assert!(hints.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(hints.iter().all(|&byte| byte == source.len()
        || (byte > 0 && source.as_bytes().get(byte - 1) == Some(&b'\n'))));
    let mut cuts = Vec::new();
    let mut start = 0;
    while start < source.len() {
        let end = if source.len() - start <= budget {
            source.len()
        } else {
            let upper = hints.partition_point(|end| *end <= start + budget);
            if let Some(end) = upper
                .checked_sub(1)
                .map(|index| hints[index])
                .filter(|end| *end > start)
            {
                end
            } else {
                let one = passages(&source[start..], budget)
                    .and_then(|mut chunks| chunks.next())
                    .ok_or("line fallback")?;
                start + one.bytes.end
            }
        };
        assert!((1..=budget).contains(&(end - start)));
        assert!(source.get(start..end).is_some(), "split a UTF-8 scalar");
        cuts.push(start..end);
        start = end;
    }
    let combined: String = cuts.iter().map(|span| &source[span.clone()]).collect();
    assert_eq!(combined.as_bytes(), source.as_bytes());
    Ok(cuts)
}

fn nonblank<'a>(
    source: &'a str,
    ranges: &'a [Range<usize>],
) -> impl Iterator<Item = &'a Range<usize>> {
    ranges
        .iter()
        .filter(|span| !source[(*span).clone()].trim().is_empty())
}

fn install_passages(
    database: &Connection,
    reference: &Reference,
    source: &str,
    collection: &str,
    publication: i64,
    original: &str,
    ranges: &[Range<usize>],
) -> Result<(), Box<dyn Error>> {
    database.execute(
        "DELETE FROM documents WHERE source = ?1 AND publication_id = ?2",
        params![source, publication],
    )?;
    let mut insert = database.prepare(
        "INSERT INTO documents
         (collection, source, revision, kind, tracked_ref, title, content, passage_start, source_location, publication_id)
         VALUES (?, ?, ?, 'current', 'refs/heads/fixture', ?, ?, ?, ?, ?)",
    )?;
    for range in nonblank(original, ranges) {
        let content = &original[range.clone()];
        let prior_lines = original[..range.start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        let line_count = content.bytes().filter(|byte| *byte == b'\n').count();
        let locator = Location {
            path: reference.path.clone(),
            blob: reference.blob.clone(),
            file_url: reference.url(),
            start_byte: u32::try_from(range.start)?,
            end_byte: u32::try_from(range.end)?,
            start_line: u32::try_from(prior_lines + 1)?,
            end_line: u32::try_from(
                prior_lines + line_count + usize::from(!content.ends_with('\n')),
            )?,
        };
        insert.execute(params![
            collection,
            source,
            reference.commit,
            reference.path,
            content,
            locator.start_byte,
            serde_json::to_string(&locator)?,
            publication
        ])?;
    }
    Ok(())
}

fn assert_projection(
    database: &Connection,
    source: &str,
    publication: i64,
    original: &str,
    ranges: &[Range<usize>],
) -> Result<(), Box<dyn Error>> {
    let mut statement = database.prepare(
        "SELECT passage_start, content FROM documents WHERE source = ?1 AND publication_id = ?2 ORDER BY passage_start",
    )?;
    let mut rows = statement.query(params![source, publication])?;
    for range in nonblank(original, ranges) {
        let row = rows.next()?.ok_or("missing projected passage")?;
        assert_eq!(row.get::<_, u32>(0)?, u32::try_from(range.start)?);
        assert_eq!(row.get::<_, String>(1)?, &original[range.clone()]);
    }
    assert!(rows.next()?.is_none(), "unexpected projected passage");
    Ok(())
}

pub(super) fn report_size(database: &Path) -> Result<(), Box<dyn Error>> {
    let read = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let size: i64 = read.query_row(
        "SELECT (page_count - freelist_count) * page_size
         FROM pragma_page_count(), pragma_freelist_count(), pragma_page_size()",
        [],
        |row| row.get(0),
    )?;
    let vector_bytes: i64 = read.query_row(
        "SELECT coalesce(sum(length(embedding)),0) FROM document_vectors",
        [],
        |row| row.get(0),
    )?;
    eprintln!(
        "Code chunking storage {}",
        json!({"dbUsedPageBytes": size, "vectorPayloadBytes": vector_bytes})
    );
    Ok(())
}
