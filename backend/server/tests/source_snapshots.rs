use std::error::Error;

use ctxfab_inference::{DIMENSIONS, Embedding};
use ctxfab_server::adapters::outbound::sqlite::SqliteStore;
use ctxfab_server::application::{
    SearchQuery,
    answers::EvidenceStore,
    ports::{CorpusStore, Repository, SnapshotStore},
    retrieval::VectorIndex,
};
use ctxfab_server::domain::corpus::{
    Batch, CollectionAccess, Commit, Document, DocumentKind,
    snapshot::{File, Plan, Snapshot, SnapshotError},
};

fn start(
    store: &SqliteStore,
    collection: &str,
    predecessor: Option<i64>,
    revision: &str,
) -> Result<Snapshot, SnapshotError> {
    store.begin_snapshot(&Plan {
        collection,
        generation: 0,
        predecessor,
        revision,
        tracked_ref: "refs/heads/main",
    })
}

fn file<'a>(path: &'a str, blob: &'a str, text: Option<&'a str>) -> File<'a> {
    File {
        source: path,
        path,
        blob,
        file_url: "https://example.org/pinned-file",
        text,
    }
}

#[test]
fn pinned_snapshot_restarts_then_replaces_paths_in_one_publication() -> Result<(), Box<dyn Error>> {
    let work = tempfile::tempdir()?;
    let database = work.path().join("corpus.sqlite3");
    let collection = "github/primary";
    let store = SqliteStore::open(&database)?;
    let original = start(&store, collection, None, "commit-a")?;
    let unchanged = "sharedfixture\n";
    store.append_snapshot(
        original.id,
        0,
        None,
        &[
            file("old.rs", "stable-blob", Some(unchanged)),
            file("obsolete.rs", "deleted-blob", Some("gonefixture\n")),
        ],
    )?;
    store.publish_snapshot(original.id)?;
    let other = start(&store, "github/other", None, "commit-a")?;
    store.append_snapshot(
        other.id,
        0,
        None,
        &[file("old.rs", "stable-blob", Some(unchanged))],
    )?;
    store.publish_snapshot(other.id)?;

    let candidate = start(&store, collection, Some(original.id), "commit-b")?;
    store.append_snapshot(
        candidate.id,
        0,
        Some("tree-continuation"),
        &[file("renamed.rs", "stable-blob", None)],
    )?;
    assert!(matches!(
        store.publish_snapshot(candidate.id),
        Err(SnapshotError::Conflict)
    ));
    assert_eq!(store.search(&SearchQuery::parse("gonefixture")?)?.len(), 1);
    drop(store);

    let store = SqliteStore::open(&database)?;
    assert_eq!(
        store
            .active_snapshot(collection)?
            .ok_or("published snapshot")?
            .id,
        original.id
    );
    let pending = store
        .pending_snapshot(collection)?
        .ok_or("missing durable stage")?;
    assert_eq!(pending.id, candidate.id);
    assert_eq!(pending.cursor.as_deref(), Some("tree-continuation"));
    assert_eq!(pending.sequence, 1);
    assert!(store.search(&SearchQuery::parse("renamed")?)?.is_empty());
    assert!(matches!(
        store.append_snapshot(pending.id, 0, None, &[]),
        Err(SnapshotError::Conflict)
    ));
    store.append_snapshot(pending.id, pending.sequence, None, &[])?;
    store.publish_snapshot(pending.id)?;
    store.publish_snapshot(pending.id)?;
    let hits = store.search(&SearchQuery::parse("sharedfixture")?)?;
    assert_eq!(hits.len(), 2);
    assert!(
        hits.iter()
            .any(|hit| hit.source == "old.rs" && hit.revision == "commit-a")
    );
    assert!(
        hits.iter()
            .any(|hit| hit.source == "renamed.rs" && hit.revision == "commit-b")
    );
    assert!(
        store
            .search(&SearchQuery::parse("gonefixture")?)?
            .is_empty()
    );
    assert!(matches!(
        store.publish_snapshot(original.id),
        Err(SnapshotError::Conflict)
    ));

    let clear = start(&store, collection, Some(candidate.id), "commit-c")?;
    store.append_snapshot(clear.id, 0, None, &[])?;
    store.publish_snapshot(clear.id)?;
    assert_eq!(
        store.search(&SearchQuery::parse("sharedfixture")?)?.len(),
        1
    );
    Ok(())
}

#[test]
fn a_new_generation_can_republish_the_same_commit_and_discard_a_failed_stage()
-> Result<(), Box<dyn Error>> {
    let store = SqliteStore::open(std::path::Path::new(":memory:"))?;
    let collection = "github/primary";
    let original = start(&store, collection, None, "commit-a")?;
    store.append_snapshot(
        original.id,
        0,
        None,
        &[
            file("kept.rs", "kept-blob", Some("keepfixture\n")),
            file("removed.rs", "removed-blob", Some("removefixture\n")),
        ],
    )?;
    store.publish_snapshot(original.id)?;
    let disagreement = store.begin_snapshot(&Plan {
        collection: "github/other",
        generation: 0,
        predecessor: None,
        revision: "commit-a",
        tracked_ref: "refs/heads/release",
    })?;
    store.append_snapshot(
        disagreement.id,
        0,
        None,
        &[file(
            "kept.rs",
            "inconsistent-blob",
            Some("inconsistentfixture\n"),
        )],
    )?;
    assert!(matches!(
        store.publish_snapshot(disagreement.id),
        Err(SnapshotError::Invalid)
    ));
    store.discard_snapshot(disagreement.id)?;
    assert!(store.pending_snapshot("github/other")?.is_none());
    assert!(
        store
            .search(&SearchQuery::parse("inconsistentfixture")?)?
            .is_empty()
    );
    assert_eq!(store.search(&SearchQuery::parse("keepfixture")?)?.len(), 1);

    assert!(store.set_collection_access(collection, 0, CollectionAccess::Lost)?);
    assert!(store.set_collection_access(collection, 1, CollectionAccess::Available)?);
    let next = store.begin_snapshot(&Plan {
        collection,
        generation: 2,
        predecessor: Some(original.id),
        revision: "commit-a",
        tracked_ref: "refs/heads/main",
    })?;
    assert_ne!(next.id, original.id);
    store.append_snapshot(next.id, 0, None, &[file("kept.rs", "kept-blob", None)])?;
    store.publish_snapshot(next.id)?;
    assert!(
        store
            .search(&SearchQuery::parse("removefixture")?)?
            .is_empty()
    );
    assert_eq!(
        store.search(&SearchQuery::parse("keepfixture")?)?[0].access,
        "available"
    );
    assert!(matches!(
        store.discard_snapshot(next.id),
        Err(SnapshotError::Conflict)
    ));
    Ok(())
}

#[test]
fn embedding_profiles_survive_refresh_and_keep_independent_storage() -> Result<(), Box<dyn Error>> {
    let work = tempfile::tempdir()?;
    let path = work.path().join("profiles.sqlite3");
    let collection = "github/primary";
    let store = SqliteStore::open(&path)?;
    let database = rusqlite::Connection::open(&path)?;
    database.execute("INSERT INTO collections(name) VALUES (?1)", [collection])?;
    database.execute(
        "INSERT INTO github_code_scopes(collection, repository_id, branch) VALUES (?1, 41, 'main')",
        [collection],
    )?;
    let original = start(&store, collection, None, "one")?;
    store.append_snapshot(
        original.id,
        0,
        None,
        &[file("kept.rs", "blob", Some("retained content\n"))],
    )?;
    store.publish_snapshot(original.id)?;
    let mut values = vec![0.0; DIMENSIONS];
    values[0] = 1.0;
    let left = Embedding::new(values.clone())?;
    values[0] = -1.0;
    let right = Embedding::new(values)?;
    let bytes = || -> rusqlite::Result<i64> {
        database.query_row(
            "SELECT bytes FROM github_storage_ledger WHERE family = 'sources'",
            [],
            |row| row.get(0),
        )
    };
    let baseline = bytes()?;
    for (profile, vector) in [("left", &left), ("right", &right)] {
        assert!(store.save_vector(profile, &store.pending(profile, 1)?[0], vector)?);
    }
    assert_eq!(bytes()? - baseline, 2 * 4096 + 4 + 5);
    assert_eq!(store.progress("left", None)?.indexed, 1);
    assert_eq!(store.progress("right", None)?.indexed, 1);
    let disabled = store.set_enabled("left", false)?;
    assert!(store.state("right")?.enabled);

    let replacement = start(&store, collection, Some(original.id), "two")?;
    store.append_snapshot(replacement.id, 0, None, &[file("kept.rs", "blob", None)])?;
    store.publish_snapshot(replacement.id)?;
    store.publish_snapshot(replacement.id)?;
    assert_eq!(bytes()? - baseline, 2 * 4096 + 4 + 5);
    let after_refresh = store.state("left")?;
    assert!(!after_refresh.enabled);
    assert_eq!(after_refresh.epoch, disabled.epoch);
    for (profile, vector) in [("left", &left), ("right", &right)] {
        let stored: (String, Vec<u8>) = database.query_row(
            "SELECT d.revision, v.embedding FROM document_vectors v
             JOIN documents d ON d.id = v.document_id WHERE v.profile = ?1",
            [profile],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(stored.0, "two");
        assert_eq!(&stored.1[..4], &vector.values()[0].to_le_bytes());
    }

    let removed = store.delete_index("left")?;
    assert_eq!((removed.removed_vectors, removed.removed_bytes), (1, 4096));
    assert_eq!(bytes()? - baseline, 4096 + 5);
    assert_eq!(store.progress("right", None)?.indexed, 1);
    let clear = start(&store, collection, Some(replacement.id), "end")?;
    store.append_snapshot(clear.id, 0, None, &[])?;
    store.publish_snapshot(clear.id)?;
    assert_eq!(store.progress("right", None)?.indexed, 0);
    let independent: i64 = database.query_row(
        "SELECT bytes FROM github_storage_usage WHERE family = 'sources'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(bytes()?, independent);
    Ok(())
}

#[test]
fn access_and_deletion_invalidate_sealed_work() -> Result<(), Box<dyn Error>> {
    let store = SqliteStore::open(std::path::Path::new(":memory:"))?;
    let collection = "github/primary";
    let original = start(&store, collection, None, "commit-a")?;
    store.append_snapshot(
        original.id,
        0,
        None,
        &[file(
            "original.rs",
            "original-blob",
            Some("retainedfixture\n"),
        )],
    )?;
    store.publish_snapshot(original.id)?;
    let candidate = start(&store, collection, Some(original.id), "commit-b")?;
    store.append_snapshot(
        candidate.id,
        0,
        None,
        &[file(
            "unpublished.rs",
            "other-blob",
            Some("unpublishedfixture\n"),
        )],
    )?;
    assert!(store.set_collection_access(collection, 0, CollectionAccess::Lost)?);
    assert!(matches!(
        store.publish_snapshot(candidate.id),
        Err(SnapshotError::Conflict)
    ));
    let retained = store.search(&SearchQuery::parse("retainedfixture")?)?;
    assert_eq!(
        retained.first().ok_or("expected retained evidence")?.access,
        "retained"
    );
    store.delete_collection(collection)?;
    assert!(store.excerpts(&retained, None)?.is_empty());
    assert!(
        store
            .search(&SearchQuery::parse("unpublishedfixture")?)?
            .is_empty()
    );
    assert!(matches!(
        start(&store, collection, None, "commit-b"),
        Err(SnapshotError::Conflict)
    ));
    Ok(())
}

#[test]
fn search_reopens_the_exact_late_passage_of_a_pinned_source() -> Result<(), Box<dyn Error>> {
    let store = SqliteStore::open(std::path::Path::new(":memory:"))?;
    let snapshot = start(&store, "github/primary", None, "commit-a")?;
    let padding = "A sufficiently long unrelated leading line.\r\n".repeat(69);
    let code = format!("{padding}fn targetfixture() {{}}\r\n");
    store.append_snapshot(
        snapshot.id,
        0,
        None,
        &[file("src/lib.rs", "source-blob", Some(&code))],
    )?;
    store.publish_snapshot(snapshot.id)?;
    assert_eq!(store.status()?.indexed_documents, 1);
    let hits = store.search(&SearchQuery::parse("targetfixture")?)?;
    let hit = hits.first().ok_or("tail symbol must be searchable")?;
    assert_eq!(hit.revision, "commit-a");
    let location = hit.location.as_ref().ok_or("missing source coordinates")?;
    assert_eq!(location.path, "src/lib.rs");
    assert_eq!(location.blob, "source-blob");
    assert_eq!(location.end_line, 70);
    let excerpts = store.excerpts(&hits, None)?;
    let excerpt = excerpts.first().ok_or("missing evidence")?;
    assert_eq!(
        excerpt.text,
        code[usize::try_from(location.start_byte)?..usize::try_from(location.end_byte)?]
    );
    assert!(excerpt.text.contains("fn targetfixture() {}"));
    let matches = store.search(&SearchQuery::parse("unrelated")?)?;
    assert_eq!(matches.len(), 2);
    let beginning = matches
        .iter()
        .find(|hit| hit.passage_start() == Some(0))
        .ok_or("missing leading passage")?;
    let following = store.following(std::slice::from_ref(beginning), None)?;
    assert_eq!(following.len(), 1);
    assert!(following[0].evidence.same_evidence(hit));
    assert!(store.following(&hits, None)?.is_empty());
    let mut values = vec![0.; DIMENSIONS];
    values[0] = 1.;
    let vector = Embedding::new(values)?;
    for passage in store.pending("fixture", 8)? {
        assert!(store.save_vector("fixture", &passage, &vector)?);
    }
    let dense = store
        .rank("fixture", &vector, Vec::new(), None, None, false)?
        .dense;
    assert_eq!(dense.len(), 2);
    assert_ne!(dense[0].passage_start(), dense[1].passage_start());
    let successor = start(&store, "github/primary", Some(snapshot.id), "commit-b")?;
    store.append_snapshot(
        successor.id,
        0,
        None,
        &[file("src/lib.rs", "source-blob", None)],
    )?;
    store.publish_snapshot(successor.id)?;
    let progress = store.progress("fixture", None)?;
    assert_eq!((progress.indexed, progress.total), (2, 2));
    assert!(
        store.excerpts(&hits, None)?.is_empty(),
        "retired evidence must not be quoted"
    );
    Ok(())
}

#[test]
fn migration_preserves_vectors_and_the_canonical_legacy_owner() -> Result<(), Box<dyn Error>> {
    let work = tempfile::tempdir()?;
    let path = work.path().join("legacy.sqlite3");
    let database = historical_database(&path, 9)?;
    database.execute_batch(
        "PRAGMA user_version = 9;
         INSERT INTO collections (name) VALUES ('left'), ('right');
         INSERT INTO documents (id, collection, source, revision, kind, title, content)
         VALUES (17, 'left', 'shared', 'old', 'history', 'Legacy', 'legacytoken'),
                (29, 'right', 'shared', 'old', 'history', 'Legacy', 'legacytoken');
         INSERT INTO document_vectors
         SELECT id, 'fixture', CAST(X'CAFE' || zeroblob(4094) AS BLOB) FROM documents;",
    )?;
    drop(database);
    let store = SqliteStore::open(&path)?;
    assert_eq!(store.progress("fixture", None)?.indexed, 2);
    let database = rusqlite::Connection::open(&path)?;
    let exact_vectors: i64 = database.query_row(
        "SELECT count(*) FROM document_vectors
         WHERE embedding = CAST(X'CAFE' || zeroblob(4094) AS BLOB)",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(exact_vectors, 2);
    let query = SearchQuery::parse("legacytoken")?;
    assert_eq!(store.search(&query)?.len(), 1);
    assert!(store.set_collection_access("left", 0, CollectionAccess::Lost)?);
    assert_eq!(store.search(&query)?[0].access, "available");
    store.delete_collection("right")?;
    assert_eq!(store.search(&query)?[0].access, "retained");
    assert!(store.set_collection_access("left", 1, CollectionAccess::Available)?);
    let changed = store.commit_batch(&Batch {
        collection: "left",
        generation: 2,
        previous_cursor: None,
        next_cursor: "changed",
        documents: &[Document {
            source: "shared",
            revision: "old",
            kind: DocumentKind::History,
            title: "Legacy",
            content: "replacementtoken",
        }],
    })?;
    assert_eq!(changed, Commit::Indexed);
    assert_eq!(store.progress("fixture", None)?.indexed, 0);
    assert!(store.search(&query)?.is_empty());
    let pending = store.pending("fixture", 1)?;
    assert_eq!(pending[0].id, 17);
    Ok(())
}

#[test]
fn github_upgrade_keeps_an_existing_collection_with_the_same_name() -> Result<(), Box<dyn Error>> {
    use ctxfab_server::application::github::{Store, current::Store as CurrentStore};

    let work = tempfile::tempdir()?;
    let path = work.path().join("v11.sqlite3");
    let database = historical_database(&path, 11)?;
    database.execute_batch(
        "PRAGMA user_version = 11;
         UPDATE github_connection SET credential_ref = 'fixture-key', login = 'fixture';
         INSERT INTO github_repositories VALUES
            (41, 'fixture/alpha', 'main', 0, 'main', 60, 1, 0, 0, 0, 1, NULL, 1);
         INSERT INTO collections (name) VALUES ('github/current/41/main');
         INSERT INTO documents (collection, source, revision, kind, title, content)
         VALUES ('github/current/41/main', 'other-provider', 'one', 'history', 'Legacy', 'legacytoken');",
    )?;
    drop(database);
    let store = SqliteStore::open(&path)?;
    let scope = store.current_scope(41)?;
    assert_ne!(scope.collection, "github/current/41/main");
    let settings = store.snapshot()?;
    store.save(
        settings.revision,
        settings.repositories.first().ok_or("repository missing")?,
    )?;
    assert_eq!(store.current_scope(41)?.collection, scope.collection);
    store.remove(settings.revision + 1, 41)?;
    assert_eq!(
        store.search(&SearchQuery::parse("legacytoken")?)?[0].access,
        "available"
    );
    Ok(())
}

fn historical_database(
    path: &std::path::Path,
    version: usize,
) -> Result<rusqlite::Connection, Box<dyn Error>> {
    let database = rusqlite::Connection::open(path)?;
    database.pragma_update(None, "foreign_keys", true)?;
    let migrations_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/adapters/outbound/sqlite/migrations");
    let mut migrations = std::fs::read_dir(migrations_dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    migrations.sort_unstable();
    for migration in migrations.into_iter().take(version) {
        database.execute_batch(&std::fs::read_to_string(migration)?)?;
    }
    Ok(database)
}

#[test]
fn compound_identifier_ranking_reaches_pinned_definitions() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("corpus.sqlite3");
    let store = SqliteStore::open(&database)?;
    let snapshot = start(&store, "github/primary", None, "commit-a")?;
    let paths: Vec<_> = (0..24)
        .map(|i| format!("long/github-quota/{i}.md"))
        .collect();
    let mut files = vec![
        file(
            "backend/domain/github.rs",
            "definition",
            Some("pub const MAX_REQUESTS_PER_MINUTE: u16 = 60;\n"),
        ),
        file(
            "backend/other.rs",
            "wrong-boundary",
            Some("const MAX_REQUESTS_PER_MINUTE_EXTRA: u16 = 123;\n"),
        ),
        file(
            "backend/client.rs",
            "usage",
            Some("use crate::github::MAX_REQUESTS_PER_MINUTE;\n"),
        ),
    ];
    files.extend(paths.iter().map(|path| {
        file(
            path,
            "discussion",
            Some(
                "// Quel plafond de requêtes GitHub par minute fixe MAX_REQUESTS_PER_MINUTE ?\n\
                 // const MAX_REQUESTS_PER_MINUTE: u16 = 123;\n",
            ),
        )
    }));
    store.append_snapshot(snapshot.id, 0, None, &files)?;
    store.publish_snapshot(snapshot.id)?;
    drop(store);

    let store = SqliteStore::open(&database)?;
    let query = SearchQuery::parse(
        "Quel plafond de requêtes GitHub par minute fixe MAX_REQUESTS_PER_MINUTE ?",
    )?;
    let hits = store.search(&query)?;
    assert_eq!(hits.len(), 20);
    assert_eq!(hits[0].source, "backend/domain/github.rs");
    assert_eq!(
        hits[0]
            .location
            .as_ref()
            .ok_or("pinned definition")?
            .start_line,
        1
    );
    assert_eq!(
        store.search(&SearchQuery::parse("plafond de requêtes")?)?[0].source,
        paths[0]
    );
    Ok(())
}

#[test]
fn technical_questions_surface_exact_paths_and_declarations() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let store = SqliteStore::open(&directory.path().join("corpus.sqlite3"))?;
    let snapshot = start(&store, "github/primary", None, "commit-a")?;
    let files = [
        file(
            "src/checkout.rs",
            "definition",
            Some("pub(crate) async fn issueReceipt() {}\n"),
        ),
        file(
            "src/checkout.rs.bak",
            "similar-file",
            Some(
                "Archived notes for src/checkout.rs. issueReceipt is declared in src/checkout.rs.\n",
            ),
        ),
        file(
            "src/callers.rs",
            "different-symbol",
            Some(
                "fn issueReceiptAgain() {}\n// Where is issueReceipt declared? issueReceipt issueReceipt\n",
            ),
        ),
        file(
            "guide.md",
            "guide",
            Some(
                "How do receipts work in src/checkout.rs? Find issueReceipt declared in src/checkout.rs.\n",
            ),
        ),
    ];
    store.append_snapshot(snapshot.id, 0, None, &files)?;
    store.publish_snapshot(snapshot.id)?;
    for question in [
        "How do receipts work in `src/checkout.rs`?",
        "Où est déclarée issueReceipt ?",
        "What does checkout.rs do?",
        "Where is issueReceipt declared in src/checkout.rs?",
    ] {
        let hits = store.search(&SearchQuery::parse(question)?)?;
        assert_eq!(hits[0].source, "src/checkout.rs", "{question}");
        assert_eq!(hits[0].revision, "commit-a");
        assert_eq!(
            hits.iter()
                .filter(|hit| hit.source == "src/checkout.rs")
                .count(),
            1
        );
    }
    assert_eq!(
        store.search(&SearchQuery::parse("How do receipts work")?)?[0].source,
        "guide.md"
    );
    let collision = start(&store, "github/other", None, "commit-b")?;
    store.append_snapshot(
        collision.id,
        0,
        None,
        &[file(
            "legacy/checkout.rs",
            "other-file",
            Some("// The legacy checkout works differently.\n"),
        )],
    )?;
    store.publish_snapshot(collision.id)?;
    let ambiguous = store.search(&SearchQuery::parse("What does checkout.rs do?")?)?;
    let lexical = store.search(&SearchQuery::parse("What does checkout.RS do?")?)?;
    assert_eq!(ambiguous.len(), lexical.len());
    assert!(
        ambiguous
            .iter()
            .zip(&lexical)
            .all(|(left, right)| left.same_evidence(right))
    );
    let exact = store.search(&SearchQuery::parse("What does src/checkout.rs do?")?)?;
    assert_eq!(exact[0].source, "src/checkout.rs");
    assert_eq!(exact[0].revision, "commit-a");
    for path in ["checkout.rs", "missing/checkout.rs"] {
        let unresolved = store.search(&SearchQuery::parse(&format!(
            "Where is issueReceipt declared in {path}?"
        ))?)?;
        let lexical = store.search(&SearchQuery::parse(&format!(
            "Where is ISSUERECEIPT declared in {path}?"
        ))?)?;
        assert_ne!(lexical[0].source, "src/checkout.rs");
        assert_eq!(unresolved.len(), lexical.len());
        assert!(
            unresolved
                .iter()
                .zip(&lexical)
                .all(|(left, right)| left.same_evidence(right)),
            "{path}"
        );
    }
    Ok(())
}

#[test]
fn answers_focus_readme_sections_without_guessing_an_owner() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("corpus.sqlite3");
    let store = SqliteStore::open(&database)?;
    let connection = rusqlite::Connection::open(&database)?;
    connection.execute_batch(
        "INSERT INTO github_repositories
            (id, full_name, default_branch, private, branch, interval_minutes,
             current_code, history, actions, code_graph, verified)
         VALUES (42, 'example/Atlas', 'main', 0, 'main', 15, 1, 0, 0, 0, 1),
                (43, 'example/Delta', 'main', 0, 'main', 15, 1, 0, 0, 0, 1);
         INSERT INTO collections (name) VALUES ('github/atlas');
         INSERT INTO github_code_scopes (collection, repository_id, branch)
         VALUES ('github/atlas', 42, 'main');",
    )?;
    let snapshot = start(&store, "github/atlas", None, "commit-a")?;
    let readme = "# Atlas\n\n## License\n\nThe project uses the Quartz license.";
    store.append_snapshot(
        snapshot.id,
        0,
        None,
        &[
            file("README.md", "readme", Some(readme)),
            file(
                "docs/README.md",
                "docs",
                Some("## License\n\nThe project uses the Wood license."),
            ),
        ],
    )?;
    store.publish_snapshot(snapshot.id)?;

    let question = SearchQuery::parse("Which license does the README state?")?;
    let focus = store
        .readme_section(&question)?
        .ok_or("root README section")?;
    assert_eq!(focus.source, "README.md");
    assert_eq!(focus.revision, "commit-a");
    let missing = SearchQuery::parse("What license does example/Delta say in its README?")?;
    assert!(store.readme_section(&missing)?.is_none());
    assert!(!store.search(&missing)?.is_empty());
    for absent in [
        SearchQuery::parse("What setup does the README document?")?,
        SearchQuery::parse("What license does docs/README.md state?")?,
        SearchQuery::parse_scoped(question.original(), Some(42), Some("wrong".into()))?,
    ] {
        assert!(store.readme_section(&absent)?.is_none());
    }

    let other = start(&store, "github/other", None, "commit-b")?;
    store.append_snapshot(
        other.id,
        0,
        None,
        &[File {
            source: "other:README.md",
            ..file(
                "README.md",
                "other-readme",
                Some("## License\n\nOther license."),
            )
        }],
    )?;
    store.publish_snapshot(other.id)?;
    assert!(store.readme_section(&question)?.is_none());

    let named = SearchQuery::parse("Which license appears in the README d’Atlas ?")?;
    let named = store
        .readme_section(&named)?
        .ok_or("named README section")?;
    let scoped = SearchQuery::parse_scoped(question.original(), Some(42), Some("main".into()))?;
    let scoped = store
        .readme_section(&scoped)?
        .ok_or("scoped README section")?;
    assert!(named.same_evidence(&focus));
    assert!(scoped.same_evidence(&focus));
    Ok(())
}

#[test]
fn answers_focus_visible_markdown_section_ahead_of_license_boilerplate()
-> Result<(), Box<dyn Error>> {
    for (file_name, heading) in [
        ("README.md", "License\n-------"),
        ("README.markdown", "## License"),
    ] {
        let store = SqliteStore::open(std::path::Path::new(":memory:"))?;
        let snapshot = start(&store, "github/atlas", None, "pinned")?;
        let source = format!(
            "<!--\n## License\n{}-->\n\n# Atlas\n\n```md\n## License\n```\n\n{}\n{heading}\n\nThe project uses the Quartz license.\n",
            "Boilerplate license terms apply to this file.\n".repeat(54),
            "Introduction and implementation overview.\n".repeat(85),
        );
        store.append_snapshot(
            snapshot.id,
            0,
            None,
            &[file(file_name, "immutable-blob", Some(&source))],
        )?;
        store.publish_snapshot(snapshot.id)?;
        let query =
            SearchQuery::parse(&format!("Which license does {file_name} state for Atlas?"))?;
        let focused = store.readme_section(&query)?.ok_or("visible section")?;
        let excerpt = &store.excerpts(&[focused], None)?[0];
        assert!(
            excerpt
                .text
                .contains("The project uses the Quartz license.")
        );
        let location = excerpt
            .evidence
            .location
            .as_ref()
            .ok_or("pinned location")?;
        assert_eq!(location.blob, "immutable-blob");
        assert_eq!(
            excerpt.text,
            source[location.start_byte as usize..location.end_byte as usize],
        );
    }
    Ok(())
}

#[test]
fn answers_find_exact_published_java_method_only_with_unique_scope() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("java.sqlite3");
    let store = SqliteStore::open(&database)?;
    rusqlite::Connection::open(&database)?.execute_batch(
        "INSERT INTO github_repositories
            (id, full_name, default_branch, private, branch, interval_minutes,
             current_code, history, actions, code_graph, verified)
         VALUES (61, 'example/Atlas', 'main', 0, 'main', 15, 1, 0, 0, 0, 1),
                (62, 'example/Delta', 'main', 0, 'main', 15, 1, 0, 0, 0, 1);
         INSERT INTO collections (name) VALUES ('github/atlas'), ('github/delta');
         INSERT INTO github_code_scopes (collection, repository_id, branch)
         VALUES ('github/atlas', 61, 'main'), ('github/delta', 62, 'main');",
    )?;
    let atlas = start(&store, "github/atlas", None, "atlas-1")?;
    let delta = start(&store, "github/delta", None, "delta-1")?;
    let method = "public boolean equals(Object value) { return value == this; }";
    let source = format!("public class Feature {{\n    {method}\n}}");
    for (snapshot, path, blob) in [
        (&atlas, "a/Feature.java", "atlas-blob"),
        (&delta, "b/Feature.java", "delta-blob"),
    ] {
        store.append_snapshot(snapshot.id, 0, None, &[file(path, blob, Some(&source))])?;
    }
    store.publish_snapshot(atlas.id)?;
    let prose = "In Feature, when does equals return true?";
    let question = SearchQuery::parse(prose)?;
    let [witness] = store
        .java_method_passages(&question)?
        .try_into()
        .map_err(|_| "one verified passage expected")?;
    let witness: ctxfab_server::application::answers::Excerpt = witness;
    assert!(witness.text.contains(method));
    assert_eq!(witness.evidence.revision, "atlas-1");
    assert_eq!(witness.repository_id, Some(61));
    for missing in [
        SearchQuery::parse("In example/Delta, when does Feature equals return true?")?,
        SearchQuery::parse("In Feature.java, when does Feature equals return true?")?,
        SearchQuery::parse("What does the README say about Feature equals?")?,
        SearchQuery::parse_scoped(prose, Some(61), Some("other".into()))?,
    ] {
        assert!(store.java_method_passages(&missing)?.is_empty());
    }

    store.publish_snapshot(delta.id)?;
    assert!(store.java_method_passages(&question)?.is_empty());
    let pinned = SearchQuery::parse_scoped(prose, Some(61), Some("main".into()))?;
    let pinned = store.java_method_passages(&pinned)?;
    assert_eq!(pinned.len(), 1);
    assert!(pinned[0].evidence.same_evidence(&witness.evidence));
    Ok(())
}

#[test]
fn explicitly_authoritative_root_readme_honors_published_repository_and_file_scope()
-> Result<(), Box<dyn Error>> {
    let work = tempfile::tempdir()?;
    let db = work.path().join("authority.sqlite3");
    let store = SqliteStore::open(&db)?;
    rusqlite::Connection::open(&db)?.execute_batch(
        "INSERT INTO github_repositories
            (id, full_name, default_branch, private, branch, interval_minutes,
             current_code, history, actions, code_graph, verified)
         VALUES (81, 'example/ProjectOne', 'main', 0, 'main', 15, 1, 0, 0, 0, 1),
                (82, 'example/ProjectTwo', 'main', 0, 'main', 15, 1, 0, 0, 0, 1);
         INSERT INTO collections (name) VALUES ('github/one'), ('github/two');
         INSERT INTO github_code_scopes (collection,repository_id,branch)
         VALUES ('github/one',81,'main'), ('github/two',82,'main');",
    )?;
    let snapshot = start(&store, "github/one", None, "authority-one")?;
    store.append_snapshot(
        snapshot.id,
        0,
        None,
        &[
            file(
                "README.md",
                "root",
                Some("# ProjectOne\nIt groups tasks.\n"),
            ),
            file(
                "docs/README.md",
                "nested",
                Some("# ProjectOne\nIt publishes packages.\n"),
            ),
            file("setup.xml", "false", Some("ProjectOne groups messages.")),
        ],
    )?;
    store.publish_snapshot(snapshot.id)?;
    for original in [
        "According to the README, what does example/ProjectOne do?",
        "Que fait ProjectOne d’après son README ?",
        "Dans le README de example/ProjectOne, que fait ProjectOne ?",
        "In the example/ProjectOne README, what does ProjectOne do?",
        "What does the README say about ProjectOne?",
    ] {
        let query = SearchQuery::parse(original)?;
        let evidence = store
            .authoritative_readme(&query)?
            .ok_or("authoritative result")?;
        assert_eq!(evidence.len(), 1);
        assert!(evidence[0].text.contains("It groups tasks."));
        assert_eq!(evidence[0].evidence.source, "README.md");
        assert_eq!(evidence[0].repository_id, Some(81));
        assert_markdown_publication_scope(&store, &evidence[0], &query)?;
    }
    for original in [
        "Que fait ProjectTwo d’après son README ?",
        "According to example/NoSuchProject README, what is available?",
        "Dans le README de example/NoSuchProject, que peut-on faire ?",
        "In the example/NoSuchProject README, what is available?",
    ] {
        let query = SearchQuery::parse(original)?;
        assert!(
            store
                .authoritative_readme(&query)?
                .is_some_and(|evidence| evidence.is_empty())
        );
    }
    for query in [
        "What does --help do according to the README?",
        "Que fait --help selon le README ?",
        "According to the README, does it support TCP/UDP?",
    ] {
        let scoped = SearchQuery::parse_scoped(query, Some(81), Some("main".into()))?;
        assert_eq!(
            store
                .authoritative_readme(&scoped)?
                .map(|evidence| evidence.len()),
            Some(1)
        );
    }
    let wrong_ref = SearchQuery::parse_scoped(
        "According to the README, what does it do?",
        Some(81),
        Some("wrong".into()),
    )?;
    assert!(
        store
            .authoritative_readme(&wrong_ref)?
            .is_some_and(|evidence| evidence.is_empty())
    );
    for original in [
        "According to docs/README.md, what is available?",
        "According to README.md and setup.xml, what is available?",
        "Compare the code with the claim according to the README.",
        "According to the README, compare ProjectOne and ProjectTwo.",
        "According to the README, what does ProjectOne and ProjectTwo provide?",
    ] {
        let query = SearchQuery::parse(original)?;
        assert!(store.authoritative_readme(&query)?.is_none());
    }
    check_large_readme_authority(&store)?;
    Ok(())
}

fn check_large_readme_authority(store: &SqliteStore) -> Result<(), Box<dyn Error>> {
    let other = start(store, "github/two", None, "authority-two")?;
    let too_large = "x".repeat(256 * 1024 + 1);
    assert!(matches!(
        store.append_snapshot(
            other.id,
            0,
            None,
            &[file("README.md", "too-large", Some(&too_large))]
        ),
        Err(SnapshotError::Invalid)
    ));
    let large = format!(
        "{}
The zestwidget deployment reads the schedule.",
        "Ordinary background information.
"
        .repeat(7_500)
    );
    store.append_snapshot(
        other.id,
        0,
        None,
        &[file("README.md", "second", Some(&large))],
    )?;
    store.publish_snapshot(other.id)?;
    let intent =
        SearchQuery::parse("According to example/ProjectTwo README, what does zestwidget read?")?;
    let passages = store.authoritative_readme(&intent)?.ok_or("large README")?;
    assert_eq!(passages.len(), 5);
    assert!(passages[0].text.contains("zestwidget deployment"));
    assert!(
        store
            .authoritative_readme(&SearchQuery::parse(
                "According to the README, what does it do?"
            )?)?
            .is_none()
    );
    Ok(())
}

fn assert_markdown_publication_scope(
    store: &SqliteStore,
    evidence: &ctxfab_server::application::answers::Excerpt,
    query: &SearchQuery,
) -> Result<(), Box<dyn Error>> {
    assert_eq!(
        store.current_document(evidence, query.scope())?.as_deref(),
        Some("# ProjectOne\nIt groups tasks.\n")
    );
    let wrong = SearchQuery::parse_scoped(query.original(), Some(82), Some("main".into()))?;
    assert!(store.current_document(evidence, wrong.scope())?.is_none());
    Ok(())
}

#[test]
fn named_java_class_ranks_relevant_complete_method_without_rewriting_ordinary_hits()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("class-local.sqlite3");
    let store = SqliteStore::open(&database)?;
    rusqlite::Connection::open(&database)?.execute_batch(
        "INSERT INTO github_repositories (id,full_name,default_branch,private,branch,interval_minutes,
                current_code,history,actions,code_graph,verified)
         VALUES (84,'example/Named','main',0,'main',15,1,0,0,0,1);
         INSERT INTO collections (name) VALUES ('github/named');
         INSERT INTO github_code_scopes (collection,repository_id,branch) VALUES ('github/named',84,'main');",
    )?;
    let snapshot = start(&store, "github/named", None, "named-pin")?;
    let class = format!(
        "public class WidgetState {{\n{}\n{}\n}}",
        "// Unrelated package header used in tests.\n".repeat(42),
        "private String value;\n
        public boolean decide(String candidate) {
            return value != null && value.equals(candidate);
        }
        public int size() { return value.length(); }"
    );
    store.append_snapshot(
        snapshot.id,
        0,
        None,
        &[
            file("src/WidgetState.java", "primary", Some(&class)),
            file(
                "src/OtherClass.java",
                "other",
                Some("public class OtherClass { public int size() { return 0; } }"),
            ),
            file(
                "docs/WidgetState.md",
                "notes",
                Some("When WidgetState compares a null value, an example may surprise."),
            ),
        ],
    )?;
    store.publish_snapshot(snapshot.id)?;
    let question = SearchQuery::parse("When WidgetState compares a null value, what happens?")?;
    assert!(store.java_method_passages(&question)?.is_empty());
    let result = store.java_evidence(&question)?;
    assert!(result.class_local);
    assert!(!result.passages.is_empty());
    assert!(
        result
            .passages
            .iter()
            .all(|p| p.repository_id == Some(84) && p.evidence.revision == "named-pin")
    );
    assert!(
        result
            .passages
            .iter()
            .map(|p| p.text.as_str())
            .collect::<String>()
            .contains("return value != null")
    );
    let hits: Vec<_> = result.passages.iter().map(|p| p.evidence.clone()).collect();
    assert_eq!(store.excerpts(&hits, question.scope())?.len(), hits.len());
    for control in [
        SearchQuery::parse("What does WidgetState and OtherClass do with a null value?")?,
        SearchQuery::parse("What does WidgetState#missing do with a null value?")?,
        SearchQuery::parse("What does WidgetState do with a null value according to the README?")?,
        SearchQuery::parse("What does src/WidgetState.java do with a null value?")?,
        SearchQuery::parse_scoped(question.original(), Some(84), Some("missing".into()))?,
    ] {
        assert!(store.java_evidence(&control)?.passages.is_empty());
    }
    Ok(())
}
