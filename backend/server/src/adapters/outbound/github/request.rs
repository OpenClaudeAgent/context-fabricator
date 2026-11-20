use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use http_body_util::{BodyExt, Full, Limited};
use hyper::{HeaderMap, Request, Response, StatusCode, Uri, body::Incoming, header};
use serde::de::DeserializeOwned;

use crate::application::Failure;
use crate::domain::github::Error;
use crate::domain::github::current::Scope;
use crate::domain::sync::{Reply, RequestKind};

use super::super::http::retry_after;
use super::{GitHubRepository, GitHubRest};

const VERSION: &str = "2026-03-10";
const MAX_REDIRECTS: usize = 3;
const BODY_LIMIT: usize = 1024 * 1024;

type Parser<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + Send + 'a>>;

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
        self.get_guarded(secret, kind, path, read, None).await
    }

    pub(super) async fn get_guarded<T: Send>(
        &self,
        secret: &[u8],
        mut kind: RequestKind,
        path: &str,
        read: fn(&mut Response<Incoming>) -> Parser<'_, T>,
        scope: Option<&Scope>,
    ) -> Result<T, Error> {
        let mut url = Uri::builder()
            .scheme(self.scheme)
            .authority(self.authority.clone())
            .path_and_query(path)
            .build()
            .map_err(|_| Error::InvalidSelection)?;
        let mut authorization = header::HeaderValue::from_bytes(&[b"Bearer ", secret].concat())
            .map_err(|_| Error::InvalidToken)?;
        authorization.set_sensitive(true);
        for _ in 0..=MAX_REDIRECTS {
            if self.transport.cooling_down().map_err(|_| Error::Storage)? {
                return Err(Error::Throttled);
            }
            let request = Request::get(url)
                .header(header::AUTHORIZATION, authorization.clone())
                .header(header::ACCEPT, "application/vnd.github+json")
                .header(header::USER_AGENT, "Context-Fabricator")
                .header("X-GitHub-Api-Version", VERSION)
                .body(Full::default())
                .map_err(|_| Error::InvalidSelection)?;
            let reply = self
                .transport
                .send_guarded(
                    kind,
                    request,
                    Duration::from_secs(15),
                    || {
                        if let Some(scope) = scope {
                            self.scope_authority
                                .as_ref()
                                .ok_or(Failure::Storage)?
                                .check_scope(scope)
                                .map_err(|error| match error {
                                    Error::Conflict => Failure::Conflict,
                                    _ => Failure::Storage,
                                })?;
                        }
                        Ok(())
                    },
                    async |response| match response.status() {
                        StatusCode::UNAUTHORIZED => Reply::Credentials,
                        StatusCode::TOO_MANY_REQUESTS => Reply::Throttled(
                            cooldown(response.headers()).unwrap_or(Duration::from_secs(60)),
                        ),
                        StatusCode::FORBIDDEN => forbidden(response).await,
                        StatusCode::NOT_FOUND if scope.is_some() && path.contains("/git/") => {
                            Reply::ExpiredCursor
                        }
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
                Reply::Permanent => return Err(Error::Access),
                Reply::ExpiredCursor => return Err(Error::ExpiredPin),
                Reply::Temporary => return Err(Error::Upstream),
            }
        }
        Err(Error::Upstream)
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

async fn decode<T: DeserializeOwned>(response: &mut Response<Incoming>) -> Result<T, Error> {
    let body = Limited::new(response.body_mut(), BODY_LIMIT)
        .collect()
        .await
        .map_err(|_| Error::Upstream)?;
    serde_json::from_slice(&body.to_bytes()).map_err(|_| Error::Upstream)
}

pub(super) fn json<T: DeserializeOwned + Send + 'static>(
    response: &mut Response<Incoming>,
) -> Parser<'_, T> {
    Box::pin(decode(response))
}

pub(super) fn skip_body(_: &mut Response<Incoming>) -> Parser<'_, ()> {
    Box::pin(std::future::ready(Ok(())))
}

pub(super) fn read_page(
    response: &mut Response<Incoming>,
) -> Parser<'_, (Vec<GitHubRepository>, bool)> {
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
        Ok((decode(response).await?, next))
    })
}

async fn forbidden<T>(response: &mut Response<Incoming>) -> Reply<Hop<T>> {
    #[derive(serde::Deserialize)]
    struct Problem {
        message: String,
    }
    if let Some(delay) = cooldown(response.headers()) {
        return Reply::Throttled(delay);
    }
    if decode::<Problem>(response).await.is_ok_and(|problem| {
        problem
            .message
            .to_ascii_lowercase()
            .contains("secondary rate limit")
    }) {
        return Reply::Throttled(Duration::from_secs(60));
    }
    Reply::Permanent
}

fn cooldown(headers: &HeaderMap) -> Option<Duration> {
    retry_after(headers).max(remaining_reset(headers))
}

fn remaining_reset(headers: &HeaderMap) -> Option<Duration> {
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
