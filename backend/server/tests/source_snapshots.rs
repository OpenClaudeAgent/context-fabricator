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
    let (dense, _) = store.rank("fixture", &vector, Vec::new(), None)?;
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
         INSERT INTO document_vectors SELECT id, 'fixture', zeroblob(4096) FROM documents;",
    )?;
    drop(database);
    let store = SqliteStore::open(&path)?;
    assert_eq!(store.progress("fixture", None)?.indexed, 2);
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
