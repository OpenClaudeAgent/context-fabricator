//! `SQLCipher` adapter for an isolated storage qualification.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use rusqlite::{Connection, OpenFlags, params};
use tempfile::{Builder, NamedTempFile};
use zeroize::Zeroizing;

const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";
const RAW_KEY_LEN: usize = 67;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Could not obtain a random test key")]
    Entropy,
    #[error("Expected an existing plaintext SQLite store")]
    PlaintextExpected,
    #[error("Could not encode the SQLite path")]
    PathEncoding,
    #[error("An application store must have a positive schema version")]
    MissingSchemaVersion,
    #[error("The cipher or SQLite integrity check failed")]
    Integrity,
    #[error("The staged schema version changed")]
    SchemaVersionMismatch,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

pub struct DataKey(Zeroizing<[u8; 32]>);

#[derive(Clone, Copy)]
pub enum AccessMode {
    ReadOnly,
    ReadWrite,
}

impl DataKey {
    /// # Errors
    /// Returns an error if the operating system cannot supply entropy.
    pub fn generate() -> Result<Self, Error> {
        let mut key = Zeroizing::new([0; 32]);
        getrandom::fill(&mut *key).map_err(|_| Error::Entropy)?;
        Ok(Self(key))
    }

    fn raw_key(&self) -> Zeroizing<[u8; RAW_KEY_LEN]> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = Zeroizing::new([0; RAW_KEY_LEN]);
        encoded[..2].copy_from_slice(b"x'");
        encoded[RAW_KEY_LEN - 1] = b'\'';
        let (digits, _) = encoded[2..RAW_KEY_LEN - 1].as_chunks_mut::<2>();
        for (pair, byte) in digits.iter_mut().zip(self.0.iter()) {
            pair[0] = HEX[usize::from(byte >> 4)];
            pair[1] = HEX[usize::from(byte & 0xf)];
        }
        encoded
    }
}

/// Open an already populated protected store with no file creation flag.
///
/// # Errors
/// Returns an error if opening, keying or reading the schema fails.
pub fn open_existing(path: &Path, key: &DataKey, mode: AccessMode) -> Result<Connection, Error> {
    let flags = match mode {
        AccessMode::ReadOnly => OpenFlags::SQLITE_OPEN_READ_ONLY,
        AccessMode::ReadWrite => OpenFlags::SQLITE_OPEN_READ_WRITE,
    };
    let db = open_keyed(path, key, flags)?;
    schema_version(&db, None)?;
    Ok(db)
}

fn open_keyed(path: &Path, key: &DataKey, flags: OpenFlags) -> Result<Connection, Error> {
    let db = Connection::open_with_flags(path, flags)?;
    let encoded = key.raw_key();
    db.key(None::<&str>, &encoded[..], false)?;
    // An unavailable codec returns no row rather than authorizing a plaintext open.
    let _: String = db.pragma_query_value(None, "cipher_version", |row| row.get(0))?;
    db.pragma_update(None, "temp_store", "MEMORY")?;
    db.pragma_update(None, "foreign_keys", true)?;
    Ok(db)
}

fn schema_version(db: &Connection, schema: Option<&str>) -> Result<u32, Error> {
    let version = db.pragma_query_value(schema, "user_version", |row| row.get(0))?;
    if version == 0 {
        return Err(Error::MissingSchemaVersion);
    }
    Ok(version)
}

/// Export a quiescent plaintext copy into a newly owned, verified scratch file.
///
/// The caller owns the source snapshot and the scratch directory. The returned
/// file is temporary and can be inspected or closed by the qualification.
/// # Errors
/// Returns an error if the source, export, or integrity verification fails.
pub fn export_plaintext(
    source: &Path,
    scratch: &Path,
    key: &DataKey,
) -> Result<NamedTempFile, Error> {
    let source = source.canonicalize()?;
    let mut header = [0; SQLITE_HEADER.len()];
    File::open(&source)?.read_exact(&mut header)?;
    if &header != SQLITE_HEADER {
        return Err(Error::PlaintextExpected);
    }
    let path = source.to_str().ok_or(Error::PathEncoding)?;
    let source_uri = format!(
        "file:{}?mode=ro",
        utf8_percent_encode(path, NON_ALPHANUMERIC)
    );
    let stage = Builder::new()
        .prefix("context-protected-")
        .suffix(".sqlite3")
        .tempfile_in(scratch)?;
    let db = open_keyed(
        stage.path(),
        key,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
    )?;
    db.execute("ATTACH DATABASE ?1 AS source KEY ''", params![source_uri])?;
    let version = schema_version(&db, Some("source"))?;
    db.query_row("SELECT sqlcipher_export('main', 'source')", [], |_| Ok(()))?;
    db.pragma_update(None, "user_version", version)?;
    db.execute_batch("DETACH DATABASE source")?;
    drop(db);

    let reader = open_existing(stage.path(), key, AccessMode::ReadOnly)?;
    if schema_version(&reader, None)? != version {
        return Err(Error::SchemaVersionMismatch);
    }
    verify_integrity(&reader)?;
    drop(reader);
    stage.as_file().sync_all()?;
    Ok(stage)
}

fn verify_integrity(db: &Connection) -> Result<(), Error> {
    {
        let mut auth = db.prepare("PRAGMA cipher_integrity_check")?;
        if auth.query([])?.next()?.is_some() {
            return Err(Error::Integrity);
        }
    }
    let check: String = db.pragma_query_value(None, "integrity_check", |row| row.get(0))?;
    let mut foreign_keys = db.prepare("PRAGMA foreign_key_check")?;
    if check == "ok" && foreign_keys.query([])?.next()?.is_none() {
        Ok(())
    } else {
        Err(Error::Integrity)
    }
}
