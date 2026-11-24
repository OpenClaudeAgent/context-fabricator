use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ctxfab_storage_protection_qualification::{
    AccessMode, DataKey, Error, export_plaintext, open_existing,
};
use rusqlite::{Connection, ErrorCode, OpenFlags, params};
use tempfile::TempDir;

const SOURCE_MARKER: &str = "ctxfabcipheroriginalsource4711";
const QUESTION_MARKER: &str = "ctxfabcipherstoredquestion4711";
const ANSWER_MARKER: &str = "ctxfabcipherstoredanswer4711";
const WAL_MARKER: &str = "ctxfabcipherwalcontent4711";
const TEMP_MARKER: &str = "ctxfabciphertemporarycontent4711";
const PIN: &str = "a7ca08bd608bb81e237197ef6981689926784349";
const BLOB: &str = "74c9918ae1d06c5dbefdb1ac43f1b0cb81b55439";
const LOCATION: &str = r#"{"path":"src/fixture.rs","blob":"74c9918ae1d06c5dbefdb1ac43f1b0cb81b55439","fileUrl":"https://github.com/fixture/cipher/blob/a7ca08bd608bb81e237197ef6981689926784349/src/fixture.rs","startByte":0,"endByte":30,"startLine":1,"endLine":1}"#;
const MODEL_SHA: &str = "3b4880f3b60ad204ac85791fd6945bd53a0abe3aaeaf60447426797906c75340";

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn migrations() -> TestResult<Vec<PathBuf>> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../backend/server/src/adapters/outbound/sqlite/migrations");
    let mut paths: Vec<_> = fs::read_dir(directory)?
        .map(|entry| entry.map(|file| file.path()))
        .collect::<Result<_, _>>()?;
    paths.retain(|path| path.extension().is_some_and(|value| value == "sql"));
    paths.sort();
    assert!(paths.len() >= 35);
    Ok(paths)
}

fn seed_plaintext(path: &Path, migrations: &[PathBuf]) -> TestResult {
    let mut db = Connection::open(path)?;
    db.pragma_update(None, "foreign_keys", true)?;
    db.pragma_update(None, "temp_store", "MEMORY")?;
    let tx = db.transaction()?;
    for path in migrations {
        tx.execute_batch(&fs::read_to_string(path)?)?;
    }
    tx.pragma_update(None, "user_version", i64::try_from(migrations.len())?)?;
    tx.execute_batch(
        "INSERT INTO github_repositories
             (id,full_name,default_branch,private,branch,interval_minutes,
              current_code,history,actions,code_graph,contents_access,actions_access,verified)
         VALUES(41,'fixture/cipher','main',1,'main',60,1,1,1,1,1,1,1);
         INSERT INTO collections(name) VALUES('cipher-fixture');",
    )?;
    tx.execute(
        "INSERT INTO source_snapshots(id,collection,generation,revision,tracked_ref,phase)
         VALUES(1,'cipher-fixture',0,?1,'refs/heads/main','published')",
        [PIN],
    )?;
    tx.execute(
        "INSERT INTO source_blobs VALUES('cipher-fixture',?1,?2)",
        params![BLOB, SOURCE_MARKER],
    )?;
    tx.execute(
        "INSERT INTO source_snapshot_files(snapshot_id,collection,path,source,blob,file_url)
         VALUES(1,'cipher-fixture','src/fixture.rs','github:41:src/fixture.rs',?1,
                json_extract(?2,'$.fileUrl'))",
        params![BLOB, LOCATION],
    )?;
    tx.execute(
        "INSERT INTO documents(id,collection,source,revision,kind,tracked_ref,title,content,
                               passage_start,source_location,publication_id)
         VALUES(1,'cipher-fixture','github:41:src/fixture.rs',?1,'current','refs/heads/main',
                'Cipher source',?2,0,?3,1)",
        params![PIN, SOURCE_MARKER, LOCATION],
    )?;
    tx.execute(
        "INSERT INTO document_vectors VALUES(1,'fixture-embedding',?1)",
        [vector_bytes()],
    )?;
    seed_provenance(&tx)?;
    tx.commit()?;
    Ok(())
}

fn vector_bytes() -> Vec<u8> {
    [0x00, 0x00, 0x80, 0x3f].repeat(1024)
}

fn seed_provenance(db: &Connection) -> TestResult {
    db.execute(
        "INSERT INTO github_history_lineage(repository_id,branch,snapshot)
         VALUES(41,'main',json_object('branch','main','tip',?1,'boundary','root','commits',json_array()))",
        [PIN],
    )?;
    db.execute(
        "INSERT INTO github_actions_snapshots(repository_id,branch,snapshot)
         VALUES(41,'main',json_object('definitionCommit',?1,'runs',json_array()))",
        [PIN],
    )?;
    db.execute(
        "INSERT INTO github_graph_publications VALUES(41,'main',?1,'rust','fixture','[]')",
        [PIN],
    )?;
    db.execute(
        "INSERT INTO verified_weights VALUES('fixture-qwen','fixture.gguf',?1,'fixture-identity')",
        [MODEL_SHA],
    )?;
    db.execute(
        "UPDATE answer_history_settings SET enabled=1,retention_days=7",
        [],
    )?;
    db.execute(
        "INSERT INTO answer_history_entries
             (id,question,scope_json,completed_at_ms,answer_version,answer_json,outcome)
         VALUES(1,?1,json_object('repositoryId',41,'branch','main'),1700000000000,1,
                json_object('outcome','answered','model','fixture-qwen','mode','bm25','text',?2,
                'citations',json_array(json_object('quote',?3,'evidence',json_object('revision',?4))),
                'evidenceRepositoryIds',json_array(41)), 'answered')",
        params![QUESTION_MARKER, ANSWER_MARKER, SOURCE_MARKER, PIN],
    )?;
    db.execute("INSERT INTO answer_history_owners VALUES(1,41)", [])?;
    Ok(())
}

fn assert_provenance(db: &Connection, version: usize) -> TestResult {
    let schema: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    assert_eq!(schema, i64::try_from(version)?);
    let fts = db.query_row(
        "SELECT d.source_location, v.embedding, d.revision FROM documents_fts f
         JOIN documents d ON d.id=f.rowid JOIN document_vectors v ON v.document_id=d.id
         WHERE documents_fts MATCH ?1 ORDER BY bm25(documents_fts)",
        [SOURCE_MARKER],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    assert_eq!(fts, (LOCATION.to_owned(), vector_bytes(), PIN.to_owned()));
    let streams: (String, String, String, String) = db.query_row(
        "SELECT json_extract(h.snapshot,'$.tip'), json_extract(a.snapshot,'$.definitionCommit'),
                g.revision, w.sha256
         FROM github_history_lineage h
         JOIN github_actions_snapshots a USING (repository_id,branch)
         JOIN github_graph_publications g USING (repository_id,branch)
         CROSS JOIN verified_weights w",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    assert_eq!(
        streams,
        (PIN.into(), PIN.into(), PIN.into(), MODEL_SHA.into())
    );
    let journal: (String, String, String, i64) = db.query_row(
        "SELECT e.question,json_extract(e.answer_json,'$.text'),
                json_extract(e.answer_json,'$.citations[0].evidence.revision'), o.repository_id
         FROM answer_history_entries e JOIN answer_history_owners o ON o.entry_id=e.id",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    assert_eq!(
        journal,
        (QUESTION_MARKER.into(), ANSWER_MARKER.into(), PIN.into(), 41)
    );
    Ok(())
}

fn contains(bytes: &[u8], marker: &str) -> bool {
    bytes
        .windows(marker.len())
        .any(|window| window == marker.as_bytes())
}

fn no_plaintext(path: &Path) -> TestResult {
    let bytes = fs::read(path)?;
    assert!(
        [
            SOURCE_MARKER,
            QUESTION_MARKER,
            ANSWER_MARKER,
            WAL_MARKER,
            TEMP_MARKER
        ]
        .into_iter()
        .all(|marker| !contains(&bytes, marker)),
        "a synthetic plaintext marker appeared in a protected artifact"
    );
    Ok(())
}

#[test]
fn exports_and_reopens_two_application_schemas() -> TestResult {
    let migrations = migrations()?;
    let directory = TempDir::new()?;
    let key = DataKey::generate()?;
    let incorrect = DataKey::generate()?;
    for version in [migrations.len() - 1, migrations.len()] {
        let plaintext = directory
            .path()
            .join(format!("plaintext #? {version}.sqlite3"));
        seed_plaintext(&plaintext, &migrations[..version])?;
        let original = fs::read(&plaintext)?;
        assert!(contains(&original, SOURCE_MARKER));
        assert!(contains(&original, QUESTION_MARKER));
        assert_provenance(&Connection::open(&plaintext)?, version)?;
        let started = Instant::now();
        let staged = export_plaintext(&plaintext, directory.path(), &key)?;
        let wall = started.elapsed();
        assert_eq!(fs::read(&plaintext)?, original);
        no_plaintext(staged.path())?;
        let encrypted = fs::read(staged.path())?;
        assert!(matches!(
            export_plaintext(staged.path(), directory.path(), &key),
            Err(Error::PlaintextExpected)
        ));
        assert!(!encrypted.starts_with(b"SQLite format 3\0"));
        let reader = open_existing(staged.path(), &key, AccessMode::ReadOnly)?;
        let cipher: String = reader.pragma_query_value(None, "cipher_version", |row| row.get(0))?;
        let provider: String =
            reader.pragma_query_value(None, "cipher_provider", |row| row.get(0))?;
        if cfg!(target_os = "macos") {
            assert_eq!(provider, "commoncrypto");
        }
        assert_provenance(&reader, version)?;
        let key_error = open_existing(staged.path(), &incorrect, AccessMode::ReadOnly);
        assert!(
            matches!(key_error, Err(Error::Sqlite(ref error)) if error.sqlite_error_code() == Some(ErrorCode::NotADatabase))
        );
        let unkeyed = Connection::open_with_flags(staged.path(), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let version_without_key: rusqlite::Result<i64> =
            unkeyed.pragma_query_value(None, "user_version", |row| row.get(0));
        assert!(
            matches!(version_without_key, Err(ref error) if error.sqlite_error_code() == Some(ErrorCode::NotADatabase))
        );
        let missing = directory.path().join("missing.sqlite3");
        assert!(open_existing(&missing, &key, AccessMode::ReadOnly).is_err());
        assert!(!missing.exists());
        println!(
            "schema {version}: SQLCipher {cipher} / {provider}, SQLite {}, source {} bytes, protected {} bytes, export {:.1} ms",
            rusqlite::version(),
            original.len(),
            encrypted.len(),
            wall.as_secs_f64() * 1000.0,
        );
    }
    Ok(())
}

#[test]
fn protects_live_wal_and_keeps_temporary_sql_in_memory() -> TestResult {
    let migrations = migrations()?;
    let directory = TempDir::new()?;
    let plaintext = directory.path().join("plaintext.sqlite3");
    seed_plaintext(&plaintext, &migrations)?;
    let key = DataKey::generate()?;
    let staged = export_plaintext(&plaintext, directory.path(), &key)?;
    let writer = open_existing(staged.path(), &key, AccessMode::ReadWrite)?;
    writer.pragma_update(None, "journal_mode", "WAL")?;
    writer.pragma_update(None, "wal_autocheckpoint", 0)?;
    writer.execute_batch("CREATE TEMP TABLE working_text(value TEXT)")?;
    writer.execute("INSERT INTO working_text VALUES (?1)", [TEMP_MARKER])?;
    let temp_path: String = writer.query_row(
        "SELECT file FROM pragma_database_list WHERE name='temp'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(temp_path, "");
    writer.execute(
        "INSERT INTO documents(collection,source,revision,kind,tracked_ref,title,content)
         VALUES('cipher-fixture','github:41:src/new.rs',?1,'current','refs/heads/main','WAL source',?2)",
        params![PIN, WAL_MARKER],
    )?;
    let reader = open_existing(staged.path(), &key, AccessMode::ReadOnly)?;
    for connection in [&reader, &writer] {
        let temp: i64 = connection.pragma_query_value(None, "temp_store", |row| row.get(0))?;
        assert_eq!(temp, 2);
    }
    let hits: i64 = reader.query_row(
        "SELECT count(*) FROM documents_fts WHERE documents_fts MATCH ?1",
        [WAL_MARKER],
        |row| row.get(0),
    )?;
    assert_eq!(hits, 1);
    let denied = reader.execute("DELETE FROM answer_history_entries", []);
    assert!(
        matches!(denied, Err(ref error) if error.sqlite_error_code() == Some(ErrorCode::ReadOnly))
    );
    assert_provenance(&reader, migrations.len())?;
    let wal = staged.path().with_extension("sqlite3-wal");
    let shm = staged.path().with_extension("sqlite3-shm");
    let wal_bytes = fs::metadata(&wal)?.len();
    assert!(
        wal_bytes > 32,
        "an actual committed WAL frame must be present"
    );
    for path in [staged.path(), &wal, &shm] {
        no_plaintext(path)?;
    }
    drop(reader);
    drop(writer);
    assert_provenance(
        &open_existing(staged.path(), &key, AccessMode::ReadOnly)?,
        migrations.len(),
    )?;
    no_plaintext(staged.path())?;
    println!(
        "verified {wal_bytes} live encrypted WAL bytes, shared memory and in-memory temp storage"
    );
    Ok(())
}

#[test]
fn protects_rollback_journal_before_rollback() -> TestResult {
    let directory = TempDir::new()?;
    let plaintext = directory.path().join("plaintext.sqlite3");
    let migrations = migrations()?;
    seed_plaintext(&plaintext, &migrations)?;
    let key = DataKey::generate()?;
    let staged = export_plaintext(&plaintext, directory.path(), &key)?;
    let writer = open_existing(staged.path(), &key, AccessMode::ReadWrite)?;
    writer.pragma_update(None, "journal_mode", "PERSIST")?;
    writer.execute_batch("BEGIN IMMEDIATE")?;
    writer.execute(
        "UPDATE answer_history_entries SET question=?1",
        [TEMP_MARKER],
    )?;
    let journal = staged.path().with_extension("sqlite3-journal");
    let journal_bytes = fs::metadata(&journal)?.len();
    assert!(journal_bytes > 512, "a rollback page must be present");
    no_plaintext(&journal)?;
    writer.execute_batch("ROLLBACK")?;
    assert_provenance(&writer, migrations.len())?;
    println!("verified {journal_bytes} encrypted rollback journal bytes");
    Ok(())
}
