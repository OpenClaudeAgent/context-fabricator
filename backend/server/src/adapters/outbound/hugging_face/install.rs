use std::fmt::Write as _;
use std::io;
use std::path::Path;

use rustix::fs::OFlags;
use sha2::{Digest, Sha256};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use super::HuggingFace;
use crate::adapters::outbound::{
    artifact_storage::DesktopArtifactReservation, verified_weights::WeightIdentity,
};
use crate::application::artifact_storage::AdmissionError;
use crate::application::model_management::LocalArtifactState;
use crate::domain::model::{Artifact, DownloadError, InstallationStage, staged_weight_peak};

impl HuggingFace {
    pub(super) fn installed_identity(
        &self,
        model: &Artifact,
    ) -> Result<Option<(u64, String)>, DownloadError> {
        let name = Path::new(model.file.as_ref());
        if !matches!(
            (name.components().next(), name.components().nth(1)),
            (Some(std::path::Component::Normal(_)), None)
        ) {
            return Err(DownloadError::Unavailable);
        }
        match std::fs::symlink_metadata(self.directory.join(name)) {
            Ok(metadata) => {
                let identity = WeightIdentity::of(&metadata).ok_or(DownloadError::Unavailable)?;
                Ok(Some((
                    metadata.len(),
                    serde_json::to_string(&identity).map_err(|_| DownloadError::Unavailable)?,
                )))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(DownloadError::Unavailable),
        }
    }
    pub(super) fn remove_installed(&self, model: &Artifact) -> Result<(), DownloadError> {
        self.remove_managed_name(model.file.as_ref())
    }

    pub(super) fn remove_partial(&self, model: &Artifact) -> Result<(), DownloadError> {
        self.remove_managed_name(&format!(".{}.part", model.file))?;
        let _ = self.remove_resume(model);
        Ok(())
    }

    fn remove_resume(&self, model: &Artifact) -> Result<(), DownloadError> {
        self.remove_managed_name(&format!(".{}.resume", model.file))?;
        let directory = rustix::fs::open(
            &self.directory,
            OFlags::DIRECTORY | OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| DownloadError::Unavailable)?;
        std::fs::File::from(directory).sync_all()?;
        Ok(())
    }

    fn remove_managed_name(&self, name: &str) -> Result<(), DownloadError> {
        use rustix::fs::{AtFlags, Mode, open, statat, unlinkat};
        use rustix::io::Errno;
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
        {
            return Err(DownloadError::Unavailable);
        }
        let directory = match open(
            &self.directory,
            OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        ) {
            Ok(directory) => directory,
            Err(Errno::NOENT) => return Ok(()),
            Err(_) => return Err(DownloadError::Unavailable),
        };
        match statat(&directory, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(info) if rustix::fs::FileType::from_raw_mode(info.st_mode).is_file() => {
                unlinkat(&directory, name, AtFlags::empty())
                    .map_err(|_| DownloadError::Unavailable)?;
                std::fs::File::from(directory)
                    .sync_all()
                    .map_err(|_| DownloadError::Unavailable)
            }
            Err(Errno::NOENT) => Ok(()),
            _ => Err(DownloadError::Unavailable),
        }
    }

    pub(super) async fn install_verified(
        &self,
        model: &Artifact,
        mut pause: tokio::sync::watch::Receiver<bool>,
        stage: tokio::sync::watch::Sender<InstallationStage>,
    ) -> Result<(), DownloadError> {
        check_pause(&pause)?;
        ensure_directory(&self.directory).await?;
        let destination = self.directory.join(model.file.as_ref());
        if self.managed.as_ref().is_some_and(|managed| {
            managed.receipts.accepts_verified_weight(
                &destination,
                &model.profile,
                Some(&model.sha256),
            )
        }) {
            return Ok(());
        }
        if size(&destination).await? == Some(model.bytes) {
            let mut existing = private_file().read(true).open(&destination).await?;
            stage.send_replace(InstallationStage::Verifying);
            if let Some(identity) = verified(&mut existing, model, &pause).await? {
                check_pause(&pause)?;
                self.record_weight(&destination, model, &identity)?;
                let _ = self.remove_resume(model);
                return Ok(());
            }
        }

        stage.send_replace(InstallationStage::Preparing);
        let staged = self.partial_path(model);
        let staged_bytes = size(&staged).await?.unwrap_or(0).min(model.bytes);
        let _reservation = self
            .reserve_weight(staged_weight_peak(model.bytes, staged_bytes))
            .await?;
        check_pause(&pause)?;
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
            self.remove_resume(model)?;
            file.set_len(0).await?;
        }
        self.transfer(model, &mut file, &mut pause, &stage).await?;
        check_pause(&pause)?;
        stage.send_replace(InstallationStage::Verifying);
        let Some(verified_identity) = verified(&mut file, model, &pause).await? else {
            drop(file);
            fs::remove_file(&staged).await?;
            let _ = self.remove_resume(model);
            return Err(DownloadError::Integrity);
        };
        file.sync_all().await?;
        check_pause(&pause)?;
        if WeightIdentity::of(&file.metadata().await?).as_ref() != Some(&verified_identity) {
            return Err(DownloadError::Integrity);
        }
        fs::rename(staged, &destination).await?;
        let published =
            WeightIdentity::of(&file.metadata().await?).ok_or(DownloadError::Integrity)?;
        if !verified_identity.same_payload(&published) {
            return Err(DownloadError::Integrity);
        }
        self.record_weight(&destination, model, &published)?;
        let _ = self.remove_resume(model);
        Ok(())
    }

    fn record_weight(
        &self,
        destination: &Path,
        model: &Artifact,
        verified: &WeightIdentity,
    ) -> Result<(), DownloadError> {
        if let Some(managed) = &self.managed {
            if WeightIdentity::at(destination)?.as_ref() != Some(verified) {
                return Err(DownloadError::Integrity);
            }
            managed.receipts.record_verified_weight(model, verified)?;
        }
        Ok(())
    }

    async fn reserve_weight(
        &self,
        bytes: u64,
    ) -> Result<Option<DesktopArtifactReservation>, DownloadError> {
        let Some(managed) = &self.managed else {
            return Ok(None);
        };
        let budget = managed.budget.clone();
        let result = tokio::task::spawn_blocking(move || budget.try_reserve(bytes))
            .await
            .map_err(|_| DownloadError::Unavailable)?;
        result.map(Some).map_err(|error| match error {
            AdmissionError::Unavailable {
                required,
                available,
            } => {
                tracing::warn!(
                    code = "model_storage_limit",
                    required,
                    available,
                    "Model storage reservation refused"
                );
                DownloadError::StorageLimit
            }
            _ => DownloadError::Unavailable,
        })
    }

    pub(super) async fn installed_and_staged(
        &self,
        model: &Artifact,
    ) -> Result<LocalArtifactState, DownloadError> {
        let path = self.directory.join(model.file.as_ref());
        let installed = if let Some(managed) = &self.managed {
            managed
                .receipts
                .accepts_verified_weight(&path, &model.profile, Some(&model.sha256))
        } else {
            size(&path).await? == Some(model.bytes)
        };
        let staged_bytes = size(&self.partial_path(model))
            .await?
            .unwrap_or(0)
            .min(model.bytes);
        Ok(LocalArtifactState {
            installed,
            staged_bytes,
        })
    }

    fn partial_path(&self, model: &Artifact) -> std::path::PathBuf {
        self.directory.join(format!(".{}.part", model.file))
    }
}

async fn size(path: &Path) -> io::Result<Option<u64>> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => Ok(metadata.is_file().then_some(metadata.len())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
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

async fn verified(
    file: &mut File,
    model: &Artifact,
    pause: &tokio::sync::watch::Receiver<bool>,
) -> Result<Option<WeightIdentity>, DownloadError> {
    file.flush().await?;
    let metadata = file.metadata().await?;
    let identity = WeightIdentity::of(&metadata);
    if metadata.len() != model.bytes || identity.is_none() {
        return Ok(None);
    }
    file.rewind().await?;
    if model.remote_file.is_some() {
        let mut magic = [0_u8; 4];
        if model.bytes < magic.len() as u64
            || file.read_exact(&mut magic).await.is_err()
            || &magic != b"GGUF"
        {
            return Ok(None);
        }
        file.rewind().await?;
    }
    let mut digest = Sha256::new();
    let mut bound = (&mut *file).take(model.bytes);
    let mut buffer = vec![0; 128 * 1024];
    loop {
        check_pause(pause)?;
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
    if bound.limit() != 0 || encoded != model.sha256 {
        return Ok(None);
    }
    let after = WeightIdentity::of(&file.metadata().await?);
    Ok(identity.filter(|before| after.as_ref() == Some(before)))
}

fn check_pause(pause: &tokio::sync::watch::Receiver<bool>) -> Result<(), DownloadError> {
    if *pause.borrow() {
        Err(DownloadError::Paused)
    } else {
        Ok(())
    }
}
