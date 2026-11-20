use std::error::Error;

use ctxfab_server::adapters::outbound::sqlite::SqliteStore;
use ctxfab_server::application::{
    AnswerLanguage, Failure, SearchQuery,
    ports::{CorpusStore, Repository},
};
use ctxfab_server::domain::corpus::{
    Batch, Checkpoint, CollectionAccess, Commit, Document, DocumentKind,
};
use rusqlite::Connection;

#[path = "fixtures/corpus.rs"]
mod fixtures;

#[test]
fn a_reader_keeps_its_snapshot_while_indexing_commits() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("corpus.sqlite3");
    let writer = SqliteStore::open(&path)?;
    let mut reader = Connection::open(&path)?;
    let snapshot = reader.transaction()?;
    let count = |database: &Connection| {
        database.query_row(
            "SELECT count(*) FROM documents_fts WHERE documents_fts MATCH 'snapshot'",
            [],
            |row| row.get::<_, i64>(0),
        )
    };
    assert_eq!(count(&snapshot)?, 0);
    let documents = [Document {
        source: "fixture:repository-42:src/main.rs",
        revision: "commit-a",
        kind: DocumentKind::Current {
            tracked_ref: "refs/heads/main",
        },
        title: "current",
        content: "snapshot",
    }];
    let batch = Batch {
        collection: "fixture",
        generation: 0,
        previous_cursor: None,
        next_cursor: "page-1",
        documents: &documents,
    };
    assert_eq!(writer.commit_batch(&batch)?, Commit::Indexed);
    assert_eq!(count(&snapshot)?, 0);
    snapshot.commit()?;
    assert_eq!(count(&reader)?, 1);
    assert_eq!(writer.search(&SearchQuery::parse("snapshot")?)?.len(), 1);
    Ok(())
}

#[test]
fn migration_rebuilds_fts_and_keeps_settings_checkpoints_and_provenance()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let fresh = directory.path().join("fresh.sqlite3");
    let upgraded = directory.path().join("previous.sqlite3");
    fixtures::seed_v1(&upgraded)?;
    let store = SqliteStore::open(&upgraded)?;
    let legacy = store.search(&SearchQuery::parse("migrationtoken")?)?;
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].source, "fixture:legacy");
    assert_eq!(legacy[0].revision, "commit-old");
    assert_eq!(legacy[0].tracked_ref, None);
    assert_eq!(store.settings()?.revision, 7);
    assert_eq!(store.settings()?.answer_language, AnswerLanguage::Fr);
    assert_eq!(store.diagnostics()?[0].code, "legacy_diagnostic");
    assert_eq!(
        store.checkpoint("legacy")?,
        Some(Checkpoint {
            generation: 2,
            cursor: Some("legacy-page".into()),
            access: CollectionAccess::Available
        })
    );
    assert_eq!(
        store.checkpoint("deleted")?,
        Some(Checkpoint {
            generation: 4,
            cursor: None,
            access: CollectionAccess::Available
        })
    );

    let documents = fixtures::documents();
    let query = SearchQuery::parse("fabricator")?;
    let mut results = Vec::new();
    for store in [SqliteStore::open(&fresh)?, store] {
        assert_eq!(store.checkpoint("fixture")?, None);
        let batch = fixture_page(&documents);
        assert_eq!(store.commit_batch(&batch)?, Commit::Indexed);
        let mut evidence: Vec<_> = store
            .search(&query)?
            .into_iter()
            .map(|hit| (hit.source, hit.revision, hit.kind, hit.tracked_ref))
            .collect();
        evidence.sort_unstable();
        assert_eq!(evidence.len(), 3);
        assert!(evidence.contains(&(
            fixtures::SOURCE.into(),
            "commit-b".into(),
            "current".into(),
            Some(fixtures::MAIN.into())
        )));
        assert!(evidence.contains(&(
            fixtures::SOURCE.into(),
            "commit-a".into(),
            "history".into(),
            None
        )));
        assert!(evidence.contains(&(
            "fixture:repository-42:.github/workflows/ci.yml".into(),
            "run-190-attempt-2".into(),
            "workflow".into(),
            None
        )));
        results.push(evidence);
    }
    assert_eq!(results[0], results[1]);
    let reopened = SqliteStore::open(&upgraded)?;
    assert_eq!(reopened.search(&query)?.len(), 3);
    assert_eq!(reopened.status()?.indexed_documents, 4);
    Ok(())
}

#[test]
fn storage_failure_rolls_back_the_batch_and_allows_replay_after_reopen()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("replay.sqlite3");
    let store = SqliteStore::open(&path)?;
    let documents = fixtures::documents();
    let first = fixture_page(&documents[1..2]);
    assert_eq!(store.commit_batch(&first)?, Commit::Indexed);
    {
        let writer_fault = Connection::open(&path)?;
        writer_fault.execute_batch(
            "CREATE TRIGGER interrupt_fixture BEFORE INSERT ON documents
             WHEN new.kind = 'workflow'
             BEGIN SELECT raise(ABORT, 'fixture interrupted'); END;",
        )?;
    }
    let pending = Batch {
        previous_cursor: Some(first.next_cursor),
        next_cursor: "page-2",
        ..fixture_page(&documents)
    };
    assert_eq!(store.commit_batch(&pending), Err(Failure::Storage));
    drop(store);

    let store = SqliteStore::open(&path)?;
    assert_eq!(
        store
            .checkpoint("fixture")?
            .and_then(|checkpoint| checkpoint.cursor),
        Some("page-1".into())
    );
    assert!(
        store
            .search(&SearchQuery::parse("stablecurrent")?)?
            .is_empty()
    );
    assert_eq!(store.status()?.collected_documents, 1);
    assert_eq!(store.status()?.indexed_documents, 1);
    Connection::open(&path)?.execute_batch("DROP TRIGGER interrupt_fixture;")?;
    assert_eq!(store.commit_batch(&pending)?, Commit::Indexed);
    assert_eq!(store.commit_batch(&pending)?, Commit::AlreadyIndexed);
    assert_eq!(store.search(&SearchQuery::parse("fabricator")?)?.len(), 3);
    assert_eq!(store.status()?.collected_documents, 3);
    assert_eq!(store.status()?.indexed_documents, 3);
    Ok(())
}

#[test]
fn named_refs_advance_independently_and_require_complete_provenance() -> Result<(), Box<dyn Error>>
{
    let store = SqliteStore::open(std::path::Path::new(":memory:"))?;
    let mut documents = fixtures::documents().into_iter().collect::<Vec<_>>();
    documents.push(Document {
        kind: DocumentKind::Current {
            tracked_ref: "refs/heads/release",
        },
        title: "Release",
        content: "releaseonly",
        ..documents[0]
    });
    let first = fixture_page(&documents);
    assert_eq!(store.commit_batch(&first)?, Commit::Indexed);
    let updated = [Document {
        revision: "commit-c",
        content: "newcurrent",
        ..documents[0]
    }];
    let next = Batch {
        previous_cursor: Some(first.next_cursor),
        next_cursor: "page-2",
        documents: &updated,
        ..first
    };
    assert_eq!(store.commit_batch(&next)?, Commit::Indexed);
    assert!(
        store
            .search(&SearchQuery::parse("stablecurrent")?)?
            .is_empty()
    );
    assert_eq!(
        store.search(&SearchQuery::parse("newcurrent")?)?[0].revision,
        "commit-c"
    );
    assert_eq!(
        store.search(&SearchQuery::parse("releaseonly")?)?[0].revision,
        "commit-b"
    );
    assert_eq!(
        store.search(&SearchQuery::parse("archival")?)?[0].revision,
        "commit-a"
    );

    let invalid = [Document {
        kind: DocumentKind::Current { tracked_ref: "" },
        ..documents[0]
    }];
    let invalid_batch = Batch {
        previous_cursor: Some(next.next_cursor),
        next_cursor: "page-3",
        documents: &invalid,
        ..next
    };
    assert_eq!(
        store.commit_batch(&invalid_batch)?,
        Commit::InvalidProvenance
    );
    assert_eq!(
        store
            .checkpoint("fixture")?
            .and_then(|checkpoint| checkpoint.cursor),
        Some("page-2".into())
    );
    assert_eq!(store.status()?.collected_documents, 4);
    Ok(())
}

#[test]
fn a_duplicate_owner_leaves_bm25_order_unchanged() -> Result<(), Box<dyn Error>> {
    let store = SqliteStore::open(std::path::Path::new(":memory:"))?;
    let alpha = Document {
        source: "fixture:alpha",
        revision: "commit-a",
        kind: DocumentKind::History,
        title: "entry",
        content: "alpha",
    };
    let beta = Document {
        source: "fixture:beta",
        content: "beta",
        ..alpha
    };
    let documents = [alpha, beta];
    assert_eq!(
        store.commit_batch(&fixture_page(&documents))?,
        Commit::Indexed
    );
    let query = SearchQuery::parse("alpha beta")?;
    let initial = store.search(&query)?;
    assert_eq!(initial.len(), 2);
    assert_eq!(initial[0].source, "fixture:alpha");

    assert_eq!(
        store.commit_batch(&Batch {
            collection: "overlap",
            ..fixture_page(&documents[..1])
        })?,
        Commit::Indexed
    );
    let overlapping = store.search(&query)?;
    assert_eq!(overlapping.len(), 2);
    assert_eq!(overlapping[0].source, initial[0].source);
    store.delete_collection("fixture")?;
    let surviving = store.search(&query)?;
    assert_eq!(surviving.len(), 1);
    assert_eq!(surviving[0].source, "fixture:alpha");
    Ok(())
}

#[test]
fn lexical_terms_follow_the_preferred_owner_and_its_updates() -> Result<(), Box<dyn Error>> {
    let store = SqliteStore::open(std::path::Path::new(":memory:"))?;
    let first = Document {
        source: "fixture:shared",
        revision: "commit-a",
        kind: DocumentKind::History,
        title: "First copy",
        content: "originaltoken",
    };
    let second = Document {
        title: "Second copy",
        content: "alternatetoken",
        ..first
    };
    assert_eq!(
        store.commit_batch(&fixture_page(std::slice::from_ref(&first)))?,
        Commit::Indexed
    );
    assert_eq!(
        store.commit_batch(&Batch {
            collection: "overlap",
            ..fixture_page(&[second])
        })?,
        Commit::Indexed
    );

    let original = SearchQuery::parse("originaltoken")?;
    let alternate = SearchQuery::parse("alternatetoken")?;
    assert_eq!(store.search(&original)?[0].title, "First copy");
    assert!(store.search(&alternate)?.is_empty());

    assert!(store.set_collection_access("fixture", 0, CollectionAccess::Lost)?);
    assert!(store.search(&original)?.is_empty());
    let hits = store.search(&alternate)?;
    assert_eq!(hits[0].title, "Second copy");
    assert_eq!(hits[0].access, "available");

    assert!(store.set_collection_access("fixture", 1, CollectionAccess::Available)?);
    let updated = Document {
        content: "correctedtoken",
        ..first
    };
    assert_eq!(
        store.commit_batch(&Batch {
            generation: 2,
            previous_cursor: Some("page-1"),
            next_cursor: "page-2",
            ..fixture_page(&[updated])
        })?,
        Commit::Indexed
    );
    assert!(store.search(&original)?.is_empty());
    assert!(store.search(&alternate)?.is_empty());
    assert_eq!(
        store.search(&SearchQuery::parse("correctedtoken")?)?.len(),
        1
    );
    Ok(())
}

#[test]
fn retained_evidence_survives_access_loss_and_deletion_is_scoped() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("retained.sqlite3");
    let store = SqliteStore::open(&path)?;
    let documents = fixtures::documents();
    let first = fixture_page(&documents);
    let overlap = Batch {
        collection: "overlap",
        ..first
    };
    assert_eq!(store.commit_batch(&first)?, Commit::Indexed);
    assert_eq!(store.commit_batch(&overlap)?, Commit::Indexed);
    assert!(store.set_collection_access("fixture", 0, CollectionAccess::Lost)?);
    assert!(!store.set_collection_access("fixture", 0, CollectionAccess::Available)?);
    assert_eq!(store.commit_batch(&first)?, Commit::StaleCheckpoint);
    let while_paused = Batch {
        generation: 1,
        ..first
    };
    assert_eq!(store.commit_batch(&while_paused)?, Commit::AccessLost);
    drop(store);

    let store = SqliteStore::open(&path)?;
    let query = SearchQuery::parse("fabricator")?;
    let hits = store.search(&query)?;
    assert_eq!(hits.len(), 3);
    assert!(hits.iter().all(|hit| hit.access == "available"));
    store.delete_collection("overlap")?;
    assert_eq!(store.commit_batch(&overlap)?, Commit::StaleCheckpoint);
    let retained = store.search(&query)?;
    assert_eq!(retained.len(), 3);
    assert!(retained.iter().all(|hit| hit.access == "retained"));

    assert!(store.set_collection_access("fixture", 1, CollectionAccess::Available)?);
    let checkpoint = store.checkpoint("fixture")?;
    assert_eq!(
        checkpoint,
        Some(Checkpoint {
            generation: 2,
            cursor: Some("page-1".into()),
            access: CollectionAccess::Available
        })
    );
    assert!(
        store
            .search(&query)?
            .iter()
            .all(|hit| hit.access == "available")
    );
    store.delete_collection("fixture")?;
    assert!(store.search(&query)?.is_empty());
    assert_eq!(store.status()?.indexed_documents, 0);
    assert_eq!(store.commit_batch(&while_paused)?, Commit::StaleCheckpoint);
    Ok(())
}

fn fixture_page<'a>(documents: &'a [Document<'a>]) -> Batch<'a> {
    Batch {
        collection: "fixture",
        generation: 0,
        previous_cursor: None,
        next_cursor: "page-1",
        documents,
    }
}
