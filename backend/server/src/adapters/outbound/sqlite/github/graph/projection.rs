//! Prepare graph evidence outside the SQLite write transaction, then swap its membership.
use std::collections::BTreeMap;
use std::path::Path;

use crate::application::github::graph::{Hit, Root};
use crate::domain::{
    corpus::snapshot::{Location, SnapshotError},
    github::{
        Error,
        current::Scope,
        graph::{self, Coverage, Resolution, Role, Source, Span},
    },
};
use rusqlite::{Connection, params};

const MAX_SERIALIZED_BYTES: usize = 16 * 1024 * 1024;
pub(super) struct File {
    path: String,
    blob: String,
    url: String,
    text: String,
}

/// A manifest can exceed the graph budget while still serving other projections.
pub(super) fn files(
    db: &Connection,
    stage: i64,
    scope: &Scope,
) -> Result<Option<Vec<File>>, Error> {
    let valid: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM source_snapshots
         WHERE id=?1 AND collection=?2 AND generation=?3 AND phase IN ('sealed','published'))",
            params![stage, scope.collection, scope.generation],
            |r| r.get(0),
        )
        .map_err(super::storage)?;
    if !valid {
        return Err(Error::Conflict);
    }
    let mut stmt = db
        .prepare(
            "SELECT f.path, f.blob, f.file_url, b.content FROM source_snapshot_files f
         JOIN source_blobs b USING(collection,blob) WHERE f.snapshot_id=?1 ORDER BY f.path",
        )
        .map_err(super::storage)?;
    let mut rows = stmt.query([stage]).map_err(super::storage)?;
    let mut files = Vec::new();
    let mut bytes = 0;
    while let Some(row) = rows.next().map_err(super::storage)? {
        let text = row
            .get_ref(3)
            .map_err(super::storage)?
            .as_str()
            .map_err(|_| Error::Storage)?;
        bytes += text.len();
        if files.len() == graph::MAX_FILES || bytes > graph::MAX_BYTES {
            return Ok(None);
        }
        files.push(File {
            path: row.get(0).map_err(super::storage)?,
            blob: row.get(1).map_err(super::storage)?,
            url: row.get(2).map_err(super::storage)?,
            text: text.to_owned(),
        });
    }
    Ok(Some(files))
}

struct Row {
    root: String,
    name: String,
    qualified: Option<String>,
    record: String,
}
pub(super) struct Prepared {
    coverage: &'static str,
    roots: Vec<Root>,
    rows: Vec<Row>,
    bytes: usize,
}

impl Prepared {
    fn empty(coverage: &'static str) -> Self {
        Self {
            coverage,
            roots: vec![],
            rows: vec![],
            bytes: 0,
        }
    }

    fn add(&mut self, root: &str, hit: &Hit) -> Result<(), Error> {
        let record = serde_json::to_string(hit).map_err(|_| Error::Storage)?;
        self.bytes += root.len()
            + hit.name.len()
            + record.len()
            + hit.qualified_name.as_ref().map_or(0, String::len);
        if self.bytes > MAX_SERIALIZED_BYTES {
            return Err(Error::CollectionLimit);
        }
        self.rows.push(Row {
            root: root.to_owned(),
            name: hit.name.clone(),
            qualified: hit.qualified_name.clone(),
            record,
        });
        Ok(())
    }

    pub(super) fn publish(
        &self,
        tx: &Connection,
        scope: &Scope,
        stage: i64,
    ) -> Result<(), SnapshotError> {
        let roots = serde_json::to_string(&self.roots).map_err(|_| SnapshotError::Storage)?;
        tx.execute(
            "DELETE FROM github_graph_publications WHERE repository_id=?1 AND branch=?2",
            params![scope.repository_id, scope.branch],
        )
        .map_err(|_| SnapshotError::Storage)?;
        let writes = tx.execute(
            "INSERT INTO github_graph_publications(repository_id,branch,revision,profile,coverage,roots)
             SELECT ?1,?2,revision,?3,?4,?5 FROM source_snapshots
             WHERE id=?6 AND collection=?7 AND generation=?8 AND phase IN ('sealed','published')",
            params![scope.repository_id, scope.branch, graph::PROFILE, self.coverage, roots,
                stage, scope.collection, scope.generation],
        ).map_err(|_| SnapshotError::Storage)?;
        if writes != 1 {
            return Err(SnapshotError::Conflict);
        }
        let mut insert = tx.prepare(
            "INSERT INTO github_graph_occurrences(repository_id,branch,root,ordinal,name,qualified_name,record)
             VALUES(?1,?2,?3,?4,?5,?6,?7)"
        ).map_err(|_| SnapshotError::Storage)?;
        for (index, row) in self.rows.iter().enumerate() {
            insert
                .execute(params![
                    scope.repository_id,
                    scope.branch,
                    row.root,
                    u32::try_from(index).map_err(|_| SnapshotError::Invalid)?,
                    row.name,
                    row.qualified,
                    row.record
                ])
                .map_err(|_| SnapshotError::Storage)?;
        }
        Ok(())
    }
}

pub(super) fn prepare(files: Option<&[File]>) -> Result<Prepared, Error> {
    let Some(files) = files else {
        return Ok(Prepared::empty("limit"));
    };
    let candidates: Vec<_> = files
        .iter()
        .map(|f| f.path.as_str())
        .filter(|p| Path::new(p).ends_with("src/lib.rs") || Path::new(p).ends_with("src/main.rs"))
        .collect();
    if candidates.is_empty() {
        return Ok(Prepared::empty("noConventionalRustRoots"));
    }
    let sources: Vec<_> = files
        .iter()
        .map(|f| Source {
            path: &f.path,
            blob: f.blob.clone(),
            text: &f.text,
        })
        .collect();
    let projections = match graph::project_roots(&sources, &candidates) {
        Ok(data) => data,
        Err(graph::Error::Limit) => return Ok(Prepared::empty("limit")),
        Err(_) => return Err(Error::Storage),
    };
    let file_by_path: BTreeMap<_, _> = files.iter().map(|f| (f.path.as_str(), f)).collect();
    let mut prepared = Prepared::empty("published");
    for projection in &projections {
        if let Err(error) = append(&mut prepared, &file_by_path, projection) {
            return if error == Error::CollectionLimit {
                Ok(Prepared::empty("limit"))
            } else {
                Err(error)
            };
        }
    }
    Ok(prepared)
}

fn append(
    result: &mut Prepared,
    files: &BTreeMap<&str, &File>,
    projection: &graph::Projection,
) -> Result<(), Error> {
    let root = &projection.crate_root;
    result.roots.push(Root {
        path: root.clone(),
        parsed_files: projection
            .files
            .iter()
            .filter(|f| f.coverage == Coverage::Parsed)
            .count(),
        reachable_files: projection.files.iter().filter(|f| f.reachable).count(),
        unsupported_files: projection
            .files
            .iter()
            .filter(|f| f.coverage == Coverage::UnsupportedLanguage)
            .count(),
        parse_errors: projection
            .files
            .iter()
            .filter(|f| f.coverage == Coverage::SyntaxError)
            .count(),
        limited_files: projection
            .files
            .iter()
            .filter(|f| f.coverage == Coverage::Limit)
            .count(),
    });
    let mut names = BTreeMap::new();
    for d in &projection.definitions {
        names.insert(
            (d.site.path.as_str(), d.site.start_byte),
            d.qualified_name.as_str(),
        );
        result.add(
            root,
            &Hit {
                name: d.name.clone(),
                qualified_name: Some(d.qualified_name.clone()),
                role: "definition".into(),
                resolution: "definition".into(),
                site: location(files, &d.site)?,
                target: None,
            },
        )?;
    }
    for r in &projection.references {
        let qualified_name = r
            .target
            .as_ref()
            .and_then(|s| names.get(&(s.path.as_str(), s.start_byte)))
            .map(|name| (*name).to_owned());
        let role = match r.role {
            Role::Call => "call",
            Role::Import => "import",
        };
        let resolution = match r.resolution {
            Resolution::Resolved => "resolved",
            Resolution::Ambiguous => "ambiguous",
            Resolution::Unresolved => "unresolved",
        };
        result.add(
            root,
            &Hit {
                name: r.name.clone(),
                qualified_name,
                role: role.into(),
                resolution: resolution.into(),
                site: location(files, &r.site)?,
                target: r.target.as_ref().map(|s| location(files, s)).transpose()?,
            },
        )?;
    }
    Ok(())
}

fn location(files: &BTreeMap<&str, &File>, span: &Span) -> Result<Location, Error> {
    let file = files.get(span.path.as_str()).ok_or(Error::Storage)?;
    if file.blob != span.blob
        || file
            .text
            .get(span.start_byte..span.end_byte)
            .is_none_or(str::is_empty)
    {
        return Err(Error::Storage);
    }
    let coord = |n| u32::try_from(n).map_err(|_| Error::Storage);
    Ok(Location {
        path: span.path.clone(),
        blob: span.blob.clone(),
        file_url: file.url.clone(),
        start_byte: coord(span.start_byte)?,
        end_byte: coord(span.end_byte)?,
        start_line: coord(span.start_line)?,
        end_line: coord(span.end_line)?,
    })
}
