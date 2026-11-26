use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http_body_util::{BodyExt, Full, Limited};
use hyper::{HeaderMap, Request, Response, StatusCode, Uri, body::Incoming, header};
use serde::de::DeserializeOwned;

use crate::application::Failure;
use crate::application::github::{MetadataRequest, current};
use crate::application::telemetry::{Stage, TraceContext};
use crate::domain::github::Error;
use crate::domain::sync::{Reply, RequestKind};

use super::super::http::{DispatchTrace, retry_after};
use super::GitHubRest;

const VERSION: &str = "2026-03-10";
const MAX_REDIRECTS: usize = 3;
const BODY_LIMIT: usize = 1024 * 1024;

type Parser<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

struct GetRequest<'a> {
    secret: &'a [u8],
    kind: RequestKind,
    path: &'a str,
    guarded: bool,
    trace: Option<DispatchTrace<'a>>,
}

enum Hop<T> {
    Data(Result<T, Error>),
    Redirect(Uri),
}

impl GitHubRest {
    pub(super) async fn get<T: Send>(
        &self,
        secret: &[u8],
        kind: RequestKind,
        path: &str,
        read: fn(&mut Response<Incoming>) -> Parser<'_, T>,
    ) -> Result<T, Error> {
        let request = GetRequest {
            secret,
            kind,
            path,
            guarded: false,
            trace: None,
        };
        self.get_hops(request, read, || Ok(())).await
    }

    pub(super) async fn get_current<T: Send>(
        &self,
        source: current::SourceRequest<'_>,
        kind: RequestKind,
        path: &str,
        route: &'static str,
        read: fn(&mut Response<Incoming>) -> Parser<'_, T>,
    ) -> Result<T, Error> {
        let request = GetRequest {
            secret: source.secret,
            kind,
            path,
            guarded: true,
            trace: provider_trace(source.trace, route),
        };
        self.get_hops(request, read, || {
            self.scope_authority
                .as_ref()
                .ok_or(Failure::Storage)?
                .check_scope(source.scope)
                .map_err(authority_error)
        })
        .await
    }

    pub(super) async fn get_metadata<T: Send>(
        &self,
        source: MetadataRequest<'_>,
        kind: RequestKind,
        path: &str,
        route: &'static str,
        read: fn(&mut Response<Incoming>) -> Parser<'_, T>,
    ) -> Result<T, Error> {
        let request = GetRequest {
            secret: source.secret,
            kind,
            path,
            guarded: true,
            trace: provider_trace(source.trace, route),
        };
        self.get_hops(request, read, || (source.guard)().map_err(authority_error))
            .await
    }

    async fn get_hops<T: Send>(
        &self,
        request: GetRequest<'_>,
        read: fn(&mut Response<Incoming>) -> Parser<'_, T>,
        check: impl Fn() -> Result<(), Failure> + Send + Sync,
    ) -> Result<T, Error> {
        let GetRequest {
            secret,
            mut kind,
            path,
            guarded,
            trace,
        } = request;
        let mut url = Uri::builder()
            .scheme(self.scheme)
            .authority(self.authority.clone())
            .path_and_query(path)
            .build()
            .map_err(|_| Error::InvalidSelection)?;
        let authorization = if secret.is_empty() {
            None
        } else {
            let mut value = header::HeaderValue::from_bytes(&[b"Bearer ", secret].concat())
                .map_err(|_| Error::InvalidToken)?;
            value.set_sensitive(true);
            Some(value)
        };
        for _ in 0..=MAX_REDIRECTS {
            if self.transport.cooling_down().map_err(|_| Error::Storage)? {
                return Err(Error::Throttled);
            }
            let mut request = Request::get(url)
                .header(header::ACCEPT, "application/vnd.github+json")
                .header(header::USER_AGENT, "Context-Fabricator")
                .header("X-GitHub-Api-Version", VERSION);
            if let Some(value) = &authorization {
                request = request.header(header::AUTHORIZATION, value);
            }
            let request = request
                .body(Full::default())
                .map_err(|_| Error::InvalidSelection)?;
            let mut needs_sso = false;
            let reply = self
                .transport
                .send_guarded(
                    kind,
                    request,
                    Duration::from_secs(15),
                    &check,
                    trace.map(|trace| {
                        if kind == RequestKind::Redirect {
                            DispatchTrace {
                                route: "/{redirect-target}",
                                ..trace
                            }
                        } else {
                            trace
                        }
                    }),
                    async |response| {
                        self.classify_get_reply(
                            response,
                            guarded && path.contains("/git/"),
                            read,
                            &mut needs_sso,
                        )
                        .await
                    },
                )
                .await
                .map_err(|error| match error {
                    Failure::Conflict => Error::Conflict,
                    _ => Error::Storage,
                })?;
            match reply {
                Reply::Success(Hop::Data(result)) => return result,
                Reply::Success(Hop::Redirect(next)) => {
                    url = next;
                    kind = RequestKind::Redirect;
                }
                Reply::Credentials => return Err(Error::Reconnect),
                Reply::Throttled(_) => return Err(Error::Throttled),
                Reply::Permanent if needs_sso => return Err(Error::SsoRequired),
                Reply::Permanent => return Err(Error::Access),
                Reply::ExpiredCursor => return Err(Error::ExpiredPin),
                Reply::Temporary => return Err(Error::Upstream),
            }
        }
        Err(Error::Upstream)
    }

    async fn classify_get_reply<T: Send>(
        &self,
        response: &mut Response<Incoming>,
        expired_if_missing: bool,
        read: fn(&mut Response<Incoming>) -> Parser<'_, T>,
        needs_sso: &mut bool,
    ) -> Reply<Hop<T>> {
        match response.status() {
            StatusCode::UNAUTHORIZED => Reply::Credentials,
            StatusCode::TOO_MANY_REQUESTS => {
                Reply::Throttled(cooldown(response.headers()).unwrap_or(Duration::from_secs(60)))
            }
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND if sso_required(response.headers()) => {
                *needs_sso = true;
                Reply::Permanent
            }
            StatusCode::FORBIDDEN => forbidden(response).await,
            StatusCode::NOT_FOUND if expired_if_missing => Reply::ExpiredCursor,
            StatusCode::NOT_FOUND | StatusCode::CONFLICT => Reply::Permanent,
            StatusCode::SERVICE_UNAVAILABLE => {
                cooldown(response.headers()).map_or(Reply::Temporary, Reply::Throttled)
            }
            status if status.is_success() => {
                if let Some(delay) = remaining_reset(response.headers())
                    && self.transport.pause(delay).is_err()
                {
                    return Reply::Temporary;
                }
                Reply::Success(Hop::Data(read(response).await))
            }
            status if status.is_redirection() => self
                .redirect(response.headers())
                .map_or(Reply::Temporary, |next| Reply::Success(Hop::Redirect(next))),
            _ => Reply::Temporary,
        }
    }

    fn redirect(&self, headers: &HeaderMap) -> Option<Uri> {
        let location = headers.get(header::LOCATION)?.to_str().ok()?;
        if location.starts_with('/') && !location.starts_with("//") {
            return Uri::builder()
                .scheme(self.scheme)
                .authority(self.authority.clone())
                .path_and_query(location)
                .build()
                .ok();
        }
        let url: Uri = location.parse().ok()?;
        (url.scheme_str() == Some(self.scheme) && url.authority() == Some(&self.authority))
            .then_some(url)
    }
}

pub(super) fn provider_trace<'a>(
    trace: Option<TraceContext<'a>>,
    route: &'static str,
) -> Option<DispatchTrace<'a>> {
    trace.map(|context| DispatchTrace {
        context,
        stage: Stage::GitHubProviderRequest,
        route,
    })
}

pub(super) fn authority_error(error: Error) -> Failure {
    match error {
        Error::Conflict => Failure::Conflict,
        _ => Failure::Storage,
    }
}

async fn decode<T: DeserializeOwned>(
    response: &mut Response<Incoming>,
    limit: usize,
) -> Result<T, Error> {
    let body = Limited::new(response.body_mut(), limit)
        .collect()
        .await
        .map_err(|_| Error::Upstream)?;
    serde_json::from_slice(&body.to_bytes()).map_err(|_| Error::Upstream)
}

pub(super) fn json<T: DeserializeOwned + Send + 'static>(
    response: &mut Response<Incoming>,
) -> Parser<'_, T> {
    json_with_limit::<T, BODY_LIMIT>(response)
}

pub(super) fn json_with_limit<T: DeserializeOwned + Send + 'static, const LIMIT: usize>(
    response: &mut Response<Incoming>,
) -> Parser<'_, T> {
    Box::pin(decode(response, LIMIT))
}

pub(super) fn skip_body(_: &mut Response<Incoming>) -> Parser<'_, ()> {
    Box::pin(std::future::ready(Ok(())))
}

pub(super) fn read_page<T: DeserializeOwned + Send>(
    response: &mut Response<Incoming>,
) -> Parser<'_, (Vec<T>, bool)> {
    Box::pin(async move {
        let next = response.headers().get_all("link").iter().any(|value| {
            value.to_str().is_ok_and(|links| {
                links.split(',').any(|link| {
                    link.split(';')
                        .skip(1)
                        .any(|param| param.trim() == "rel=\"next\"")
                })
            })
        });
        Ok((decode(response, BODY_LIMIT).await?, next))
    })
}

pub(super) async fn forbidden<T>(response: &mut Response<Incoming>) -> Reply<T> {
    #[derive(serde::Deserialize)]
    struct Problem {
        message: String,
    }
    if let Some(delay) = cooldown(response.headers()) {
        return Reply::Throttled(delay);
    }
    if decode::<Problem>(response, BODY_LIMIT)
        .await
        .is_ok_and(|problem| {
            problem
                .message
                .to_ascii_lowercase()
                .contains("secondary rate limit")
        })
    {
        return Reply::Throttled(Duration::from_secs(60));
    }
    Reply::Permanent
}

fn sso_required(headers: &HeaderMap) -> bool {
    headers
        .get("x-github-sso")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|directive| directive.trim().eq_ignore_ascii_case("required"))
}

pub(super) fn cooldown(headers: &HeaderMap) -> Option<Duration> {
    retry_after(headers).max(remaining_reset(headers))
}

pub(super) fn remaining_reset(headers: &HeaderMap) -> Option<Duration> {
    if headers.get("x-ratelimit-remaining")?.to_str().ok()? != "0" {
        return None;
    }
    let reset = headers
        .get("x-ratelimit-reset")?
        .to_str()
        .ok()?
        .parse::<u64>()
        .ok()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Some(Duration::from_secs(reset.saturating_sub(now).max(1)))
}
