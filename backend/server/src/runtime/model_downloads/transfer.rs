use std::time::Duration;

use fluent_uri::UriRef;
use http_body_util::{BodyExt, Full};
use hyper::{Request, StatusCode, Uri, body::Incoming, header};
use tokio::fs::File;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::time::timeout;

use super::{Artifact, DownloadError, ModelDownloads};
use crate::domain::sync::{Reply, RequestKind};

const TRANSFER_DEADLINE: Duration = Duration::from_mins(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: u8 = 4;
const MAX_RETRIES: u8 = 2;
const MAX_INLINE_COOLDOWN: Duration = Duration::from_secs(60);

enum Hop {
    Redirect(Uri),
    Finished,
}

impl ModelDownloads {
    pub(super) async fn transfer(
        &self,
        model: &Artifact,
        file: &mut File,
    ) -> Result<(), DownloadError> {
        let mut redirects = 0;
        let mut retries = 0;
        let mut target = model.url.clone();
        let mut kind = RequestKind::ModelDownload;
        loop {
            file.flush().await?;
            let offset = file.metadata().await?.len();
            if offset == model.bytes {
                return Ok(());
            }
            let reply = self.hop(&target, model.bytes, offset, kind, file).await?;
            match reply {
                Reply::Success(Hop::Finished) => return Ok(()),
                Reply::Success(Hop::Redirect(next)) if redirects < MAX_REDIRECTS => {
                    redirects += 1;
                    target = next;
                    kind = RequestKind::Redirect;
                }
                Reply::Temporary | Reply::Throttled(..)
                    if retries < MAX_RETRIES && retry_here(&reply) =>
                {
                    retries += 1;
                    target = model.url.clone();
                    kind = RequestKind::Retry;
                }
                _ => return Err(DownloadError::Transfer),
            }
        }
    }

    async fn hop(
        &self,
        target: &Uri,
        total: u64,
        offset: u64,
        kind: RequestKind,
        file: &mut File,
    ) -> Result<Reply<Hop>, DownloadError> {
        let mut request = Request::get(target.clone())
            .header(header::ACCEPT_ENCODING, "identity")
            .header(header::CONNECTION, "close");
        if offset > 0 {
            request = request.header(header::RANGE, format!("bytes={offset}-"));
        }
        let request = request
            .body(Full::default())
            .map_err(|_| DownloadError::Transfer)?;
        self.transport
            .send(kind, request, TRANSFER_DEADLINE, async |response| {
                self.handle_response(target, total, offset, kind, file, response)
                    .await
            })
            .await
            .map_err(|_| DownloadError::Unavailable)
    }

    async fn handle_response(
        &self,
        target: &Uri,
        total: u64,
        offset: u64,
        kind: RequestKind,
        file: &mut File,
        response: &mut Response,
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
            StatusCode::OK | StatusCode::PARTIAL_CONTENT => {
                stream(response, total, offset, file).await
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
    response.headers().get(name)?.to_str().ok()
}

fn retry_here(reply: &Reply<Hop>) -> bool {
    !matches!(reply, Reply::Throttled(delay) if *delay > MAX_INLINE_COOLDOWN)
}

async fn stream(response: &mut Response, total: u64, offset: u64, file: &mut File) -> Reply<Hop> {
    if response.headers().contains_key(header::CONTENT_ENCODING)
        && content_header(response, header::CONTENT_ENCODING) != Some("identity")
    {
        return Reply::Permanent;
    }
    let start = if response.status() == StatusCode::PARTIAL_CONTENT {
        let expected = format!("bytes {offset}-{}/{total}", total - 1);
        if offset == 0 || content_header(response, header::CONTENT_RANGE) != Some(&expected) {
            return Reply::Permanent;
        }
        offset
    } else {
        if response.headers().contains_key(header::CONTENT_RANGE) {
            return Reply::Permanent;
        }
        0
    };
    if content_header(response, header::CONTENT_LENGTH)
        .is_some_and(|length| length.parse::<u64>() != Ok(total - start))
    {
        return Reply::Permanent;
    }
    if start == 0 && file.set_len(0).await.is_err() {
        return Reply::Permanent;
    }
    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return Reply::Permanent;
    }

    let mut remaining = total - start;
    loop {
        match timeout(IDLE_TIMEOUT, response.body_mut().frame()).await {
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
