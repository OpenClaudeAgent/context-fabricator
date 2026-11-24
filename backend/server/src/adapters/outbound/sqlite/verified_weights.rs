use std::io;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension as _, params};

use super::SqliteStore;
use crate::adapters::outbound::verified_weights::WeightIdentity;
use crate::domain::model::{Artifact, DownloadError};

impl SqliteStore {
    /// # Errors
    /// Returns an error if the verified weight identity cannot be persisted.
    pub fn record_verified_weight(
        &self,
        artifact: &Artifact,
        identity: &WeightIdentity,
    ) -> Result<(), DownloadError> {
        let identity = serde_json::to_string(identity).map_err(|_| DownloadError::Unavailable)?;
        self.write()
            .map_err(|_| DownloadError::Unavailable)?
            .execute(
                "INSERT INTO verified_weights (profile, file_name, sha256, identity) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (profile) DO UPDATE SET file_name = excluded.file_name,
                     sha256 = excluded.sha256, identity = excluded.identity",
                params![
                    artifact.profile.as_ref(),
                    artifact.file.as_ref(),
                    artifact.sha256.as_ref(),
                    identity
                ],
            )
            .map_err(|_| DownloadError::Unavailable)?;
        Ok(())
    }

    #[must_use]
    pub fn accepts_verified_weight(
        &self,
        path: &Path,
        profile: &str,
        sha256: Option<&str>,
    ) -> bool {
        self.read()
            .is_ok_and(|database| accepts(&database, path, profile, sha256).unwrap_or(false))
    }
}

/// Verifies the installed weight using the already migrated app database without write access.
/// # Errors
/// Returns an error if the file identity or app database cannot be read.
pub fn guardian_weight(path: &Path, profile: &str) -> io::Result<bool> {
    let invalid = || io::Error::other("managed weight verification failed");
    let directory = path.parent().ok_or_else(invalid)?;
    let support = directory.parent().ok_or_else(invalid)?;
    if directory.file_name().and_then(|name| name.to_str()) != Some("inference") {
        return Err(invalid());
    }
    let database = Connection::open_with_flags(
        support.join("context.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| invalid())?;
    database
        .busy_timeout(Duration::from_secs(1))
        .map_err(|_| invalid())?;
    accepts(&database, path, profile, None).map_err(|_| invalid())
}

fn accepts(
    database: &Connection,
    path: &Path,
    profile: &str,
    sha256: Option<&str>,
) -> Result<bool, rusqlite::Error> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(false);
    };
    if !path
        .parent()
        .and_then(|directory| directory.symlink_metadata().ok())
        .is_some_and(|metadata| metadata.is_dir())
    {
        return Ok(false);
    }
    let Ok(Some(actual)) = WeightIdentity::at(path) else {
        return Ok(false);
    };
    let record = database
        .query_row(
            "SELECT identity, sha256 FROM verified_weights WHERE profile = ?1 AND file_name = ?2",
            params![profile, name],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((identity, digest)) = record else {
        return Ok(false);
    };
    Ok(sha256.is_none_or(|expected| digest == expected)
        && serde_json::from_str::<WeightIdentity>(&identity).is_ok_and(|saved| saved == actual))
}
