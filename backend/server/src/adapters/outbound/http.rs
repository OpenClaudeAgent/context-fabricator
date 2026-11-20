//! A quota-admitted HTTP transport. A send represents one request, including its response body.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use http_body_util::Full;
use hyper::{
    HeaderMap, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header,
};
use hyper_tls::HttpsConnector;
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use tokio::time::timeout;

use crate::application::Failure;
use crate::domain::sync::{Reply, RequestKind};
use crate::runtime::outbound::{GateStatus, OutboundGate};

pub struct HttpTransport {
    gate: Arc<OutboundGate>,
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
}

impl HttpTransport {
    /// # Errors
    /// Returns an error if the platform TLS client cannot be initialized.
    pub fn new(gate: Arc<OutboundGate>) -> Result<Self, native_tls::Error> {
        let tls = native_tls::TlsConnector::new()?;
        let mut http = HttpConnector::new();
        http.enforce_http(false);
        http.set_connect_timeout(Some(Duration::from_secs(10)));
        let client = Client::builder(TokioExecutor::new())
            .retry_canceled_requests(false)
            .pool_max_idle_per_host(0)
            .build(HttpsConnector::from((http, tls.into())));
        Ok(Self { gate, client })
    }

    /// Sends one request. Redirects and retries are explicit calls so each gets its own permit
    /// and ledger entry. The response handler runs under the same permit as the header exchange.
    ///
    /// # Errors
    /// Returns a storage error if the hop cannot be accounted for.
    pub async fn send<T>(
        &self,
        kind: RequestKind,
        request: Request<Full<Bytes>>,
        deadline: Duration,
        handle: impl AsyncFnOnce(&mut Response<Incoming>) -> Reply<T>,
    ) -> Result<Reply<T>, Failure> {
        self.send_with(kind, request, deadline, async |response| {
            match response.status() {
                StatusCode::TOO_MANY_REQUESTS => Reply::Throttled(
                    retry_after(response.headers()).unwrap_or(Duration::from_secs(2)),
                ),
                StatusCode::SERVICE_UNAVAILABLE => {
                    retry_after(response.headers()).map_or(Reply::Temporary, Reply::Throttled)
                }
                _ => handle(response).await,
            }
        })
        .await
    }

    pub(crate) fn pause(&self, duration: Duration) -> Result<(), Failure> {
        self.gate.pause(duration)
    }

    pub(crate) fn cooling_down(&self) -> Result<bool, Failure> {
        self.gate.cooling_down()
    }

    pub(crate) fn status(&self) -> Result<GateStatus, Failure> {
        self.gate.status()
    }

    pub(crate) async fn send_with<T>(
        &self,
        kind: RequestKind,
        request: Request<Full<Bytes>>,
        deadline: Duration,
        handle: impl AsyncFnOnce(&mut Response<Incoming>) -> Reply<T>,
    ) -> Result<Reply<T>, Failure> {
        self.send_guarded(kind, request, deadline, || Ok(()), handle)
            .await
    }

    pub(crate) async fn send_guarded<T>(
        &self,
        kind: RequestKind,
        request: Request<Full<Bytes>>,
        deadline: Duration,
        authorize: impl Fn() -> Result<(), Failure> + Sync,
        handle: impl AsyncFnOnce(&mut Response<Incoming>) -> Reply<T>,
    ) -> Result<Reply<T>, Failure> {
        self.gate
            .handoff_guarded(kind, deadline, authorize, async |_| {
                let Ok(Ok(mut response)) =
                    timeout(Duration::from_secs(30), self.client.request(request)).await
                else {
                    return Reply::Temporary;
                };
                handle(&mut response).await
            })
            .await
    }
}

pub(super) fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(header::RETRY_AFTER)?.to_str().ok()?;
    value.parse().map(Duration::from_secs).ok().or_else(|| {
        httpdate::parse_http_date(value)
            .ok()
            .map(|date| date.duration_since(SystemTime::now()).unwrap_or_default())
    })
}
