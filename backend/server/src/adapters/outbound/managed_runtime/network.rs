//! Counted, bounded retrieval of a compiled-in GitHub release asset.
//!
//! GitHub documents the `github.com/.../releases/download/...` browser URL:
//! <https://docs.github.com/en/rest/releases/assets>.
//! GitHub release links redirect to `release-assets.githubusercontent.com`:
//! <https://github.com/orgs/community/discussions/167206>.
//! Neither a redirect URL nor a response body is trusted as a release pin.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

#[cfg(test)]
use std::net::SocketAddrV4;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use fluent_uri::UriRef;
use http_body_util::{BodyExt, Full};
use hyper::{Request, StatusCode, Uri, body::Incoming, header};
use rustix::fs::OFlags;
use tokio::{
    fs::File,
    io::{AsyncSeekExt, AsyncWriteExt},
    time::timeout,
};

use crate::{
    adapters::outbound::http::HttpTransport,
    domain::sync::{Reply, RequestKind},
    runtime::outbound::OutboundGate,
};

/// Register this quota independently of model and GitHub API traffic.
pub const BUCKET: &str = "runtime-release";
const MAX_ARCHIVE: u64 = 512 * 1024 * 1024;
const MAX_REDIRECTS: u8 = 4;
const MAX_RETRIES: u8 = 2;
const MAX_INLINE_COOLDOWN: Duration = Duration::from_secs(60);
const DEADLINE: Duration = Duration::from_mins(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum RuntimeDownloadError {
    #[error("Runtime release configuration is invalid")]
    InvalidConfiguration,
    #[error("Runtime release download failed")]
    Transfer,
    #[error("Runtime release staging failed")]
    Staging,
}

/// Owns the private temporary archive until the verifier has copied it.
pub struct StagedArchive {
    path: PathBuf,
    file: std::fs::File,
}
impl StagedArchive {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    #[must_use]
    pub fn file(&self) -> &std::fs::File {
        &self.file
    }
}
impl Drop for StagedArchive {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Clone, Copy)]
enum Scope {
    Release,
    #[cfg(test)]
    Loopback(SocketAddrV4),
}
impl Scope {
    fn permits(self, uri: &Uri, origin: bool) -> bool {
        let Some(authority) = uri.authority() else {
            return false;
        };
        if authority.as_str().contains('@') {
            return false;
        }
        let host = authority.host();
        match self {
            Self::Release => {
                uri.scheme_str() == Some("https")
                    && authority.port().is_none_or(|port| port.as_u16() == 443)
                    && (host.eq_ignore_ascii_case("github.com")
                        || (!origin
                            && host.eq_ignore_ascii_case("release-assets.githubusercontent.com")))
            }
            #[cfg(test)]
            Self::Loopback(address) => {
                address.ip().is_loopback()
                    && uri.scheme_str() == Some("http")
                    && host == address.ip().to_string()
                    && authority.port_u16() == Some(address.port())
            }
        }
    }
}

/// Validate a compiled GitHub release browser asset URL before startup.
///
/// # Errors
/// Rejects non-HTTPS, non-GitHub, mutable-style, malformed or non-ZIP URLs.
pub fn validate_release_url(value: &str) -> Result<(), RuntimeDownloadError> {
    let uri: Uri = value
        .parse()
        .map_err(|_| RuntimeDownloadError::InvalidConfiguration)?;
    let parts: Vec<_> = uri
        .path()
        .strip_prefix('/')
        .unwrap_or_default()
        .split('/')
        .collect();
    if !Scope::Release.permits(&uri, true)
        || uri.query().is_some()
        || parts.len() != 6
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || !part.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+')
                })
        })
        || parts[2] != "releases"
        || parts[3] != "download"
        || parts[4] == "latest"
        || Path::new(parts[5])
            .extension()
            .is_none_or(|ext| ext != "zip")
    {
        return Err(RuntimeDownloadError::InvalidConfiguration);
    }
    Ok(())
}

pub struct RuntimeDownload {
    transport: HttpTransport,
    scope: Scope,
    transfer_slot: Option<Arc<tokio::sync::Semaphore>>,
}
impl RuntimeDownload {
    /// # Errors
    /// Returns an error if TLS initialization fails.
    pub fn new(gate: Arc<OutboundGate>) -> Result<Self, RuntimeDownloadError> {
        Self::with_scope(gate, Scope::Release)
    }
    fn with_scope(gate: Arc<OutboundGate>, scope: Scope) -> Result<Self, RuntimeDownloadError> {
        Ok(Self {
            transport: HttpTransport::new(gate).map_err(|_| RuntimeDownloadError::Transfer)?,
            scope,
            transfer_slot: None,
        })
    }

    #[must_use]
    pub fn with_transfer_slot(mut self, slot: Arc<tokio::sync::Semaphore>) -> Self {
        self.transfer_slot = Some(slot);
        self
    }

    /// Download a URL and size provided by trusted compiled release configuration.
    ///
    /// # Errors
    /// Returns an error for invalid configuration, rejected redirects, failed transfers,
    /// a size mismatch, or an unavailable private staging directory.
    pub async fn download(
        &self,
        release_url: &str,
        bytes: u64,
        directory: &Path,
    ) -> Result<StagedArchive, RuntimeDownloadError> {
        let origin: Uri = release_url
            .parse()
            .map_err(|_| RuntimeDownloadError::InvalidConfiguration)?;
        if bytes == 0
            || bytes > MAX_ARCHIVE
            || !self.scope.permits(&origin, true)
            || origin.path_and_query().is_none_or(|p| p.path().is_empty())
        {
            return Err(RuntimeDownloadError::InvalidConfiguration);
        }
        if matches!(self.scope, Scope::Release) {
            validate_release_url(release_url)?;
        }
        let (guard, mut file) = new_stage(directory)?;
        let mut target = origin.clone();
        let mut redirects = 0;
        let mut retries = 0;
        let mut kind = RequestKind::Manual;
        loop {
            let request = Request::get(target.clone())
                .header(header::ACCEPT_ENCODING, "identity")
                .header(header::CONNECTION, "close")
                .header(header::USER_AGENT, "context-fabricator-runtime")
                .body(Full::default())
                .map_err(|_| RuntimeDownloadError::Transfer)?;
            let reply = self
                .transport
                .send_checked(
                    kind,
                    request,
                    DEADLINE,
                    || Ok(()),
                    crate::adapters::outbound::http::ArtifactAdmission {
                        shared: self.transfer_slot.as_deref(),
                        pause: None,
                        ..Default::default()
                    },
                    async |response| handle(response, &target, self.scope, bytes, &mut file).await,
                )
                .await
                .map_err(|_| RuntimeDownloadError::Transfer)?;
            match reply {
                Reply::Success(Hop::Done) => {
                    file.sync_all()
                        .await
                        .map_err(|_| RuntimeDownloadError::Staging)?;
                    file.rewind()
                        .await
                        .map_err(|_| RuntimeDownloadError::Staging)?;
                    let file = file.into_std().await;
                    return Ok(StagedArchive {
                        path: guard.into_path(),
                        file,
                    });
                }
                Reply::Success(Hop::Redirect(next)) if redirects < MAX_REDIRECTS => {
                    redirects += 1;
                    target = next;
                    kind = RequestKind::Redirect;
                }
                Reply::Temporary | Reply::Throttled(..)
                    if retries < MAX_RETRIES && retry_here(&reply) =>
                {
                    retries += 1;
                    redirects = 0;
                    target = origin.clone();
                    kind = RequestKind::Retry;
                }
                _ => return Err(RuntimeDownloadError::Transfer),
            }
        }
    }
}

enum Hop {
    Done,
    Redirect(Uri),
}
fn retry_here(reply: &Reply<Hop>) -> bool {
    !matches!(reply, Reply::Throttled(delay) if *delay > MAX_INLINE_COOLDOWN)
}

async fn handle(
    response: &mut hyper::Response<Incoming>,
    target: &Uri,
    scope: Scope,
    total: u64,
    file: &mut File,
) -> Reply<Hop> {
    match response.status() {
        StatusCode::REQUEST_TIMEOUT => return Reply::Temporary,
        status if status.is_server_error() => return Reply::Temporary,
        StatusCode::FORBIDDEN if !scope.permits(target, true) => return Reply::Temporary,
        StatusCode::MOVED_PERMANENTLY
        | StatusCode::FOUND
        | StatusCode::SEE_OTHER
        | StatusCode::TEMPORARY_REDIRECT
        | StatusCode::PERMANENT_REDIRECT => {
            let next = single_header(response, header::LOCATION)
                .and_then(|location| resolve(target, location))
                .filter(|uri| scope.permits(uri, false));
            return next.map_or(Reply::Permanent, |uri| Reply::Success(Hop::Redirect(uri)));
        }
        StatusCode::OK => {}
        _ => return Reply::Permanent,
    }
    if response.headers().contains_key(header::CONTENT_ENCODING)
        && single_header(response, header::CONTENT_ENCODING) != Some("identity")
    {
        return Reply::Permanent;
    }
    if response.headers().contains_key(header::CONTENT_RANGE)
        || single_header(response, header::CONTENT_LENGTH)
            .and_then(|value| value.parse::<u64>().ok())
            != Some(total)
    {
        return Reply::Permanent;
    }
    if file.set_len(0).await.is_err() || file.rewind().await.is_err() {
        return Reply::Permanent;
    }
    let mut remaining = total;
    loop {
        match timeout(IDLE_TIMEOUT, response.body_mut().frame()).await {
            Ok(Some(Ok(frame))) => {
                let Ok(chunk) = frame.into_data() else {
                    continue;
                };
                let length = chunk.len() as u64;
                if length > remaining || file.write_all(&chunk).await.is_err() {
                    return Reply::Permanent;
                }
                remaining -= length;
            }
            Ok(None) if remaining == 0 => return Reply::Success(Hop::Done),
            Ok(None | Some(Err(_))) | Err(_) => return Reply::Temporary,
        }
    }
}
fn single_header(response: &hyper::Response<Incoming>, name: header::HeaderName) -> Option<&str> {
    let mut values = response.headers().get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}
fn resolve(base: &Uri, location: &str) -> Option<Uri> {
    let base = fluent_uri::Uri::parse(base.to_string()).ok()?;
    let uri = UriRef::parse(location).ok()?.resolve_against(&base).ok()?;
    if uri.has_fragment() {
        return None;
    }
    uri.as_str().parse().ok()
}

struct StageGuard(PathBuf);
impl StageGuard {
    fn into_path(mut self) -> PathBuf {
        std::mem::take(&mut self.0)
    }
}
impl Drop for StageGuard {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}
fn new_stage(directory: &Path) -> Result<(StageGuard, File), RuntimeDownloadError> {
    if !std::fs::symlink_metadata(directory)
        .map_err(|_| RuntimeDownloadError::Staging)?
        .is_dir()
    {
        return Err(RuntimeDownloadError::Staging);
    }
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| RuntimeDownloadError::Staging)?;
    let mut suffix = String::with_capacity(32);
    for byte in random {
        let _ = write!(suffix, "{byte:02x}");
    }
    let path = directory.join(format!(".runtime-download-{suffix}.zip"));
    let mut options = std::fs::OpenOptions::new();
    options
        .write(true)
        .read(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags((OFlags::NOFOLLOW | OFlags::NONBLOCK).bits().cast_signed());
    let file = options
        .open(&path)
        .map_err(|_| RuntimeDownloadError::Staging)?;
    Ok((StageGuard(path), File::from_std(file)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
