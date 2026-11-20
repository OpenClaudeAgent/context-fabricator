use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::net::SocketAddrV4;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::entropy::random_uuid;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Endpoint<'a> {
    api_version: u8,
    process_id: u32,
    url: String,
    token: &'a str,
}

pub struct Session {
    directory: PathBuf,
    _lock: File,
    published: bool,
}

impl Session {
    /// Own the directory for the duration of one backend session.
    ///
    /// # Errors
    /// Returns an I/O error for an insecure directory or if another backend owns it.
    pub fn acquire(directory: &Path) -> io::Result<Self> {
        let mut builder = DirBuilder::new();
        builder.recursive(true).mode(0o700).create(directory)?;
        let metadata = fs::symlink_metadata(directory)?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private data directory required",
            ));
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(directory.join("session.lock"))?;
        lock.try_lock()?;
        let session = Self {
            directory: directory.to_owned(),
            _lock: lock,
            published: false,
        };
        match fs::remove_file(session.endpoint_path()) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        Ok(session)
    }

    /// Atomically publish discovery metadata to clients of the current user.
    ///
    /// # Errors
    /// Returns an error if the private discovery file cannot be written.
    pub fn publish(&mut self, address: SocketAddrV4, token: &str) -> io::Result<()> {
        let endpoint = Endpoint {
            api_version: 1,
            process_id: std::process::id(),
            url: format!("http://{address}"),
            token,
        };
        let temporary = self.directory.join(format!(".endpoint-{}", random_uuid()?));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        let write = (|| {
            serde_json::to_writer(&mut file, &endpoint)?;
            file.flush()?;
            fs::rename(&temporary, self.endpoint_path())
        })();
        if write.is_err() {
            let _ = fs::remove_file(temporary);
        }
        write?;
        self.published = true;
        Ok(())
    }

    fn endpoint_path(&self) -> PathBuf {
        self.directory.join("endpoint.json")
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.published {
            let _ = fs::remove_file(self.endpoint_path());
        }
    }
}
