use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use gix_transport::packetline::decode::{Stream, streaming};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::{
    Method, Request, Response, StatusCode, Uri,
    body::{Bytes, Incoming},
    header::{self, HeaderValue},
};

use super::{
    MAX_PACK_BYTES,
    wire::{Endpoint, Response as WireResponse},
};
use crate::adapters::outbound::github::{
    GitHubRest,
    request::{authority_error, cooldown, forbidden, provider_trace, remaining_reset},
};
use crate::application::{Failure, github::current::SourceRequest};
use crate::domain::github::{Error, RepositoryName};
use crate::domain::sync::{Reply, RequestKind};

const ADVERTISEMENT: &str = "application/x-git-upload-pack-advertisement";
const RESULT: &str = "application/x-git-upload-pack-result";

impl GitHubRest {
    pub(super) async fn exchange_git(
        &self,
        name: &RepositoryName<'_>,
        source: Option<SourceRequest<'_>>,
        endpoint: Endpoint,
        protocol_v2: bool,
        body: Bytes,
    ) -> Result<WireResponse, Error> {
        if self.transport.cooling_down().map_err(|_| Error::Storage)? {
            return Err(Error::Throttled);
        }
        let (method, tail, route, media, limit) = match endpoint {
            Endpoint::Advertisement => (
                Method::GET,
                "/info/refs?service=git-upload-pack",
                "/{owner}/{repo}.git/info/refs",
                ADVERTISEMENT,
                1024 * 1024,
            ),
            Endpoint::UploadPack => (
                Method::POST,
                "/git-upload-pack",
                "/{owner}/{repo}.git/git-upload-pack",
                RESULT,
                MAX_PACK_BYTES,
            ),
        };
        let authority = if self.scheme == "https" {
            Uri::from_static("https://github.com")
                .authority()
                .cloned()
                .ok_or(Error::InvalidSelection)?
        } else {
            self.authority.clone()
        };
        let uri = Uri::builder()
            .scheme(self.scheme)
            .authority(authority)
            .path_and_query(format!("/{}/{}.git{tail}", name.owner, name.name))
            .build()
            .map_err(|_| Error::InvalidSelection)?;
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::ACCEPT, media)
            .header(header::USER_AGENT, "Context-Fabricator");
        if let Some(source) = source {
            let encoded = STANDARD.encode([b"x-access-token:", source.secret].concat());
            let mut authorization = HeaderValue::from_str(&format!("Basic {encoded}"))
                .map_err(|_| Error::InvalidToken)?;
            authorization.set_sensitive(true);
            request = request.header(header::AUTHORIZATION, authorization);
        }
        if protocol_v2 {
            request = request.header("Git-Protocol", "version=2");
        }
        if matches!(endpoint, Endpoint::UploadPack) {
            request = request.header(
                header::CONTENT_TYPE,
                "application/x-git-upload-pack-request",
            );
        }
        let request = request.body(Full::new(body)).map_err(|_| Error::Upstream)?;
        let reply = self
            .transport
            .send_guarded(
                RequestKind::Pagination,
                request,
                Duration::from_secs(90),
                || {
                    if let Some(source) = source {
                        self.scope_authority
                            .as_ref()
                            .ok_or(Failure::Storage)?
                            .check_scope(source.scope)
                            .map_err(authority_error)?;
                    }
                    Ok(())
                },
                provider_trace(source.and_then(|source| source.trace), route),
                async |response| self.read_git_reply(response, media, limit).await,
            )
            .await
            .map_err(|error| match error {
                Failure::Conflict => Error::Conflict,
                _ => Error::Storage,
            })?;
        match reply {
            Reply::Success(bytes) => bytes.map(|body| WireResponse {
                headers: format!("Content-Type: {media}\n").into(),
                body,
            }),
            Reply::Credentials => Err(Error::Reconnect),
            Reply::Throttled(_) => Err(Error::Throttled),
            Reply::Permanent => Err(Error::Access),
            Reply::ExpiredCursor => Err(Error::ExpiredPin),
            Reply::Temporary => Err(Error::Upstream),
        }
    }

    async fn read_git_reply(
        &self,
        response: &mut Response<Incoming>,
        media: &str,
        limit: usize,
    ) -> Reply<Result<Bytes, Error>> {
        match response.status() {
            StatusCode::UNAUTHORIZED => Reply::Credentials,
            StatusCode::FORBIDDEN => forbidden(response).await,
            StatusCode::NOT_FOUND => Reply::Permanent,
            StatusCode::TOO_MANY_REQUESTS => {
                Reply::Throttled(cooldown(response.headers()).unwrap_or(Duration::from_secs(60)))
            }
            StatusCode::SERVICE_UNAVAILABLE => {
                cooldown(response.headers()).map_or(Reply::Temporary, Reply::Throttled)
            }
            StatusCode::OK
                if response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .is_some_and(|value| value == media) =>
            {
                if let Some(delay) = remaining_reset(response.headers())
                    && self.transport.pause(delay).is_err()
                {
                    return Reply::Temporary;
                }
                match Limited::new(response.body_mut(), limit).collect().await {
                    Ok(body) => {
                        let bytes = body.to_bytes();
                        if media == RESULT && rejected_pin(&bytes) {
                            Reply::ExpiredCursor
                        } else {
                            Reply::Success(Ok(bytes))
                        }
                    }
                    Err(error) if error.is::<LengthLimitError>() => {
                        Reply::Success(Err(Error::CollectionLimit))
                    }
                    Err(_) => Reply::Temporary,
                }
            }
            _ => Reply::Temporary,
        }
    }
}

fn rejected_pin(bytes: &[u8]) -> bool {
    // GitHub can refuse a stale want inside a successful HTTP response.
    matches!(
        streaming(bytes),
        Ok(Stream::Complete { line, .. })
            if line.check_error().is_some_and(|error| error.0.starts_with(b"upload-pack: not our ref "))
    )
}
