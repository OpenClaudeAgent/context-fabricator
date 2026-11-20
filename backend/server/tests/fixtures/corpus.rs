use std::path::Path;

use ctxfab_server::domain::corpus::{Document, DocumentKind};
use rusqlite::Connection;

pub const SOURCE: &str = "fixture:repository-42:src/main.rs";
pub const MAIN: &str = "refs/heads/main";

pub fn documents() -> [Document<'static>; 3] {
    [
        Document {
            source: SOURCE,
            revision: "commit-b",
            kind: DocumentKind::Current { tracked_ref: MAIN },
            title: "Current source",
            content: "fabricator stablecurrent",
        },
        Document {
            source: SOURCE,
            revision: "commit-a",
            kind: DocumentKind::History,
            title: "Historical source",
            content: "fabricator archival",
        },
        Document {
            source: "fixture:repository-42:.github/workflows/ci.yml",
            revision: "run-190-attempt-2",
            kind: DocumentKind::Workflow,
            title: "Fixture workflow",
            content: "fabricator buildpipeline",
        },
    ]
}

pub fn seed_v1(path: &Path) -> rusqlite::Result<()> {
    let database = Connection::open(path)?;
    database.execute_batch(include_str!(
        "../../src/adapters/outbound/sqlite/migrations/0001_foundation.sql"
    ))?;
    database.execute_batch(
        "PRAGMA user_version = 1;
         UPDATE settings SET revision = 7, answer_language = 'fr';
         INSERT INTO diagnostics (timestamp, code, message)
             VALUES (unixepoch(), 'legacy_diagnostic', 'Legacy fixture.');
         INSERT INTO collections (name, generation, cursor)
             VALUES ('legacy', 2, 'legacy-page'), ('deleted', 4, NULL);
         INSERT INTO documents (id, collection, source, revision, kind, title, content)
             VALUES (11, 'legacy', 'fixture:legacy', 'commit-old', 'current',
                     'Legacy record', 'migrationtoken');
         INSERT INTO documents_fts (documents_fts, rowid, title, content)
             VALUES ('delete', 11, 'Legacy record', 'migrationtoken');",
    )?;
    Ok(())
}
