use std::fmt::Write as _;
use std::io;
use std::path::Path;

use rustix::fs::OFlags;
use sha2::{Digest, Sha256};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use super::{Artifact, DownloadError, ModelDownloads, size};

impl ModelDownloads {
    pub(super) async fn install(&self, model: &Artifact) -> Result<(), DownloadError> {
        ensure_directory(&self.directory).await?;
        let destination = self.directory.join(model.file);
        if size(&destination).await? == Some(model.bytes) {
            let mut existing = private_file().read(true).open(&destination).await?;
            if verified(&mut existing, model).await? {
                return Ok(());
            }
        }

        let staged = self.partial_path(model);
        let mut file = private_file()
            .create(true)
            .read(true)
            .write(true)
            .open(&staged)
            .await?;
        let metadata = file.metadata().await?;
        if !metadata.is_file() {
            return Err(DownloadError::Unavailable);
        }
        if metadata.len() > model.bytes {
            file.set_len(0).await?;
        }
        self.transfer(model, &mut file).await?;
        if !verified(&mut file, model).await? {
            drop(file);
            fs::remove_file(&staged).await?;
            return Err(DownloadError::Integrity);
        }
        file.sync_all().await?;
        drop(file);
        fs::rename(staged, destination).await?;
        Ok(())
    }
}

async fn ensure_directory(directory: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    match builder.mode(0o700).create(directory).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            if fs::symlink_metadata(directory).await?.is_dir() {
                Ok(())
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

fn private_file() -> OpenOptions {
    let mut options = OpenOptions::new();
    options
        .mode(0o600)
        .custom_flags((OFlags::NOFOLLOW | OFlags::NONBLOCK).bits().cast_signed());
    options
}

async fn verified(file: &mut File, model: &Artifact) -> io::Result<bool> {
    file.flush().await?;
    if file.metadata().await?.len() != model.bytes {
        return Ok(false);
    }
    file.rewind().await?;
    let mut digest = Sha256::new();
    let mut bound = file.take(model.bytes);
    let mut buffer = vec![0; 128 * 1024];
    loop {
        let read = bound.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let mut encoded = String::with_capacity(64);
    for byte in digest.finalize() {
        let _ = write!(encoded, "{byte:02x}");
    }
    Ok(bound.limit() == 0 && encoded == model.sha256)
}
