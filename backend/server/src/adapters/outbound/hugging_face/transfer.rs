use rustix::fs::OFlags;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::os::unix::fs::MetadataExt;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use fluent_uri::UriRef;
use http_body_util::{BodyExt, Full};
use hyper::{Request, StatusCode, Uri, body::Incoming, header};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::time::timeout;

use super::{HuggingFace, representation};
use crate::domain::model::{Artifact, DownloadError, InstallationStage};
use crate::domain::sync::{Reply, RequestKind};

const TRANSFER_DEADLINE: Duration = Duration::from_mins(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: u8 = 4;
const MAX_RETRIES: u8 = 2;
const MAX_INLINE_COOLDOWN: Duration = Duration::from_secs(60);

enum Hop {
    Redirect(Uri),
    Finished,
    Paused,
    Reset,
}

impl HuggingFace {
    pub(super) async fn transfer(
        &self,
        model: &Artifact,
        file: &mut File,
        pause: &mut tokio::sync::watch::Receiver<bool>,
        stage: &tokio::sync::watch::Sender<InstallationStage>,
    ) -> Result<(), DownloadError> {
        let mut redirects = 0;
        let mut retries = 0;
        let origin = self.artifact_url(model)?;
        let mut target = origin.clone();
        let mut kind = RequestKind::ModelDownload;
        let marker = self.directory.join(format!(".{}.resume", model.file));
        let mut resume = load_marker(&marker, model, file).await;
        let mut reset_retry = false;
        loop {
            if *pause.borrow() {
                return Err(DownloadError::Paused);
            }
            file.flush().await?;
            let offset = file.metadata().await?.len();
            if offset == model.bytes {
                return Ok(());
            }
            let validator = resume
                .as_ref()
                .filter(|metadata| representation::matches(&metadata.representation, &target));
            let requested_offset = if validator.is_some() { offset } else { 0 };
            let reply = self
                .hop(
                    &target,
                    model,
                    requested_offset,
                    validator,
                    &marker,
                    kind,
                    file,
                    pause,
                    stage,
                )
                .await?;
            resume = load_marker(&marker, model, file).await;
            match reply {
                Reply::Success(Hop::Finished) => return Ok(()),
                Reply::Success(Hop::Paused) => return Err(DownloadError::Paused),
                Reply::Success(Hop::Reset) if !reset_retry => {
                    reset_retry = true;
                    invalidate(&marker).await?;
                    file.set_len(0).await?;
                    resume = None;
                    target = origin.clone();
                    redirects = 0;
                    kind = RequestKind::Retry;
                }
                Reply::Success(Hop::Redirect(next)) if redirects < MAX_REDIRECTS => {
                    redirects += 1;
                    target = next;
                    kind = RequestKind::Redirect;
                }
                Reply::Temporary | Reply::Throttled(..) if *pause.borrow() => {
                    file.flush().await?;
                    return Err(DownloadError::Paused);
                }
                Reply::Temporary | Reply::Throttled(..)
                    if retries < MAX_RETRIES && retry_here(&reply) =>
                {
                    retries += 1;
                    target = origin.clone();
                    redirects = 0;
                    kind = RequestKind::Retry;
                }
                _ => return Err(DownloadError::Transfer),
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn hop(
        &self,
        target: &Uri,
        model: &Artifact,
        offset: u64,
        validator: Option<&Resume>,
        marker: &Path,
        kind: RequestKind,
        file: &mut File,
        pause: &mut tokio::sync::watch::Receiver<bool>,
        stage: &tokio::sync::watch::Sender<InstallationStage>,
    ) -> Result<Reply<Hop>, DownloadError> {
        let mut request = Request::get(target.clone())
            .header(header::ACCEPT_ENCODING, "identity")
            .header(header::CONNECTION, "close");
        if offset > 0 {
            request = request
                .header(header::RANGE, format!("bytes={offset}-"))
                .header(
                    header::IF_RANGE,
                    &validator.ok_or(DownloadError::Transfer)?.etag,
                );
        }
        let request = request
            .body(Full::default())
            .map_err(|_| DownloadError::Transfer)?;
        let authority = pause.clone();
        let reset = std::sync::atomic::AtomicBool::new(false);
        let reply = self
            .transport
            .send_checked(
                kind,
                request,
                TRANSFER_DEADLINE,
                || {
                    if *authority.borrow() {
                        Err(crate::application::Failure::Conflict)
                    } else {
                        stage.send_replace(InstallationStage::Transferring);
                        Ok(())
                    }
                },
                crate::adapters::outbound::http::ArtifactAdmission {
                    shared: self.transfer_slot.as_deref(),
                    pause: Some(pause.clone()),
                    ..Default::default()
                },
                async |response| {
                    let reply = self
                        .handle_response(
                            target, model, offset, validator, marker, kind, file, response, pause,
                        )
                        .await;
                    if matches!(reply, Reply::Success(Hop::Reset)) {
                        reset.store(true, std::sync::atomic::Ordering::Relaxed);
                        Reply::Temporary
                    } else {
                        reply
                    }
                },
            )
            .await
            .map_err(|error| {
                if matches!(error, crate::application::Failure::Conflict) && *pause.borrow() {
                    DownloadError::Paused
                } else {
                    DownloadError::Unavailable
                }
            })?;
        if reset.load(std::sync::atomic::Ordering::Relaxed) {
            Ok(Reply::Success(Hop::Reset))
        } else {
            Ok(reply)
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_response(
        &self,
        target: &Uri,
        model: &Artifact,
        offset: u64,
        validator: Option<&Resume>,
        marker: &Path,
        kind: RequestKind,
        file: &mut File,
        response: &mut Response,
        pause: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Reply<Hop> {
        let status = response.status();
        match status {
            StatusCode::FORBIDDEN if kind == RequestKind::Redirect => Reply::Temporary,
            StatusCode::REQUEST_TIMEOUT => Reply::Temporary,
            status if status.is_server_error() => Reply::Temporary,
            StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT => content_header(response, header::LOCATION)
                .and_then(|location| resolve(target, location))
                .filter(|next| self.scope.permits(next))
                .map_or(Reply::Permanent, |next| Reply::Success(Hop::Redirect(next))),
            StatusCode::RANGE_NOT_SATISFIABLE if offset > 0 => Reply::Success(Hop::Reset),
            StatusCode::OK | StatusCode::PARTIAL_CONTENT => {
                stream(
                    response, model, target, offset, validator, marker, file, pause,
                )
                .await
            }
            _ => Reply::Permanent,
        }
    }
}

fn resolve(base: &Uri, location: &str) -> Option<Uri> {
    let base = fluent_uri::Uri::parse(base.to_string()).ok()?;
    let target = UriRef::parse(location).ok()?.resolve_against(&base).ok()?;
    if target.has_fragment() {
        return None;
    }
    target.as_str().parse().ok()
}

type Response = hyper::Response<Incoming>;

fn content_header(response: &Response, name: header::HeaderName) -> Option<&str> {
    let mut values = response.headers().get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    Some(value)
}

fn retry_here(reply: &Reply<Hop>) -> bool {
    !matches!(reply, Reply::Throttled(delay) if *delay > MAX_INLINE_COOLDOWN)
}

#[allow(clippy::too_many_arguments)]
async fn stream(
    response: &mut Response,
    model: &Artifact,
    target: &Uri,
    offset: u64,
    validator: Option<&Resume>,
    marker: &Path,
    file: &mut File,
    pause: &mut tokio::sync::watch::Receiver<bool>,
) -> Reply<Hop> {
    if response.headers().contains_key(header::CONTENT_ENCODING)
        && content_header(response, header::CONTENT_ENCODING) != Some("identity")
    {
        return if response.status() == StatusCode::PARTIAL_CONTENT && offset > 0 {
            Reply::Success(Hop::Reset)
        } else {
            Reply::Permanent
        };
    }
    let total = model.bytes;
    let start = if response.status() == StatusCode::PARTIAL_CONTENT {
        let expected = format!("bytes {offset}-{}/{total}", total - 1);
        if offset == 0
            || content_header(response, header::CONTENT_RANGE) != Some(&expected)
            || validator.is_none_or(|value| {
                content_header(response, header::ETAG) != Some(value.etag.as_str())
            })
        {
            return if offset > 0 {
                Reply::Success(Hop::Reset)
            } else {
                Reply::Permanent
            };
        }
        offset
    } else {
        if response.headers().contains_key(header::CONTENT_RANGE) {
            return Reply::Permanent;
        }
        0
    };
    if response.headers().contains_key(header::CONTENT_LENGTH)
        && content_header(response, header::CONTENT_LENGTH)
            .is_none_or(|length| length.parse::<u64>() != Ok(total - start))
    {
        return if start > 0 {
            Reply::Success(Hop::Reset)
        } else {
            Reply::Permanent
        };
    }
    if start == 0 {
        if invalidate(marker).await.is_err()
            || file.set_len(0).await.is_err()
            || file.sync_all().await.is_err()
        {
            return Reply::Permanent;
        }
        if let Some(etag) =
            content_header(response, header::ETAG).filter(|value| strong_etag(value))
            && save_marker(marker, model, target, file, etag)
                .await
                .is_err()
        {
            return Reply::Permanent;
        }
    }
    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return Reply::Permanent;
    }
    if start > 0 {
        tracing::info!(
            code = "model_download_resumed",
            offset_bytes = start,
            "Model download resumed at the verified offset"
        );
    }

    let mut remaining = total - start;
    loop {
        if *pause.borrow() {
            return if file.flush().await.is_ok() {
                Reply::Success(Hop::Paused)
            } else {
                Reply::Permanent
            };
        }
        let frame = tokio::select! {
            result = timeout(IDLE_TIMEOUT, response.body_mut().frame()) => result,
            _ = pause.changed() => {
                return if file.flush().await.is_ok() { Reply::Success(Hop::Paused) } else { Reply::Permanent };
            }
        };
        match frame {
            Ok(Some(Ok(frame))) => {
                let Ok(chunk) = frame.into_data() else {
                    continue;
                };
                let len = chunk.len() as u64;
                if len > remaining || file.write_all(&chunk).await.is_err() {
                    return Reply::Permanent;
                }
                remaining -= len;
            }
            Ok(None) if remaining == 0 => return Reply::Success(Hop::Finished),
            Ok(None | Some(Err(_))) | Err(_) => return Reply::Temporary,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Resume {
    sha256: String,
    bytes: u64,
    device: u64,
    inode: u64,
    representation: String,
    etag: String,
}

fn strong_etag(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && bytes.len() <= 512
        && bytes[0] == b'"'
        && bytes[bytes.len() - 1] == b'"'
        && bytes[1..bytes.len() - 1]
            .iter()
            .all(|b| *b == 0x21 || (0x23..=0x7e).contains(b) || *b >= 0x80)
}
fn marker_options() -> OpenOptions {
    let mut opts = OpenOptions::new();
    opts.mode(0o600)
        .custom_flags((OFlags::NOFOLLOW | OFlags::NONBLOCK).bits().cast_signed());
    opts
}
async fn load_marker(path: &Path, model: &Artifact, file: &File) -> Option<Resume> {
    let marker = marker_options().read(true).open(path).await.ok()?;
    let info = marker.metadata().await.ok()?;
    if !info.is_file() || info.len() > 4096 {
        return None;
    }
    let mut bytes = Vec::new();
    marker.take(4097).read_to_end(&mut bytes).await.ok()?;
    if bytes.len() > 4096 {
        return None;
    }
    let value: Resume = serde_json::from_slice(&bytes).ok()?;
    let info = file.metadata().await.ok()?;
    (value.sha256 == model.sha256
        && value.bytes == model.bytes
        && value.device == info.dev()
        && value.inode == info.ino()
        && strong_etag(&value.etag))
    .then_some(value)
}
async fn invalidate(path: &Path) -> Result<(), DownloadError> {
    match fs::remove_file(path).await {
        Ok(()) => {
            File::open(path.parent().ok_or(DownloadError::Unavailable)?)
                .await?
                .sync_all()
                .await?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}
async fn save_marker(
    path: &Path,
    model: &Artifact,
    target: &Uri,
    file: &File,
    etag: &str,
) -> Result<(), DownloadError> {
    let info = file.metadata().await?;
    let value = Resume {
        sha256: model.sha256.to_string(),
        bytes: model.bytes,
        device: info.dev(),
        inode: info.ino(),
        representation: representation::fingerprint(target),
        etag: etag.to_owned(),
    };
    let bytes = serde_json::to_vec(&value).map_err(|_| DownloadError::Unavailable)?;
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|_| DownloadError::Unavailable)?;
    let mut suffix = String::with_capacity(32);
    for byte in random {
        let _ = write!(suffix, "{byte:02x}");
    }
    let temporary = PathBuf::from(format!("{}.{}", path.display(), suffix));
    let result = async {
        let mut out = marker_options()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .await?;
        out.write_all(&bytes).await?;
        out.sync_all().await?;
        fs::rename(&temporary, path).await?;
        File::open(path.parent().ok_or(DownloadError::Unavailable)?)
            .await?
            .sync_all()
            .await?;
        Ok::<_, DownloadError>(())
    }
    .await;
    if result.is_err() {
        let _ = fs::remove_file(temporary).await;
    }
    result
}
