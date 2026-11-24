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
use crate::application::telemetry::{Event, Stage, TraceContext};
use crate::domain::sync::{Reply, RequestKind};
use crate::runtime::outbound::{GateStatus, OutboundGate};

#[derive(Default)]
pub(crate) struct ArtifactAdmission<'a> {
    pub shared: Option<&'a tokio::sync::Semaphore>,
    pub pause: Option<tokio::sync::watch::Receiver<bool>>,
    pub trace: Option<DispatchTrace<'a>>,
}

#[derive(Clone, Copy)]
pub(crate) struct DispatchTrace<'a> {
    pub context: TraceContext<'a>,
    pub stage: Stage,
    pub route: &'static str,
}

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
        self.send_checked(
            kind,
            request,
            deadline,
            || Ok(()),
            ArtifactAdmission::default(),
            handle,
        )
        .await
    }

    pub(crate) async fn send_checked<T>(
        &self,
        kind: RequestKind,
        request: Request<Full<Bytes>>,
        deadline: Duration,
        authorize: impl Fn() -> Result<(), Failure> + Sync,
        admission: ArtifactAdmission<'_>,
        handle: impl AsyncFnOnce(&mut Response<Incoming>) -> Reply<T>,
    ) -> Result<Reply<T>, Failure> {
        self.send_guarded_artifact(
            kind,
            request,
            deadline,
            authorize,
            admission,
            async |response| match response.status() {
                StatusCode::TOO_MANY_REQUESTS => Reply::Throttled(
                    retry_after(response.headers()).unwrap_or(Duration::from_secs(2)),
                ),
                StatusCode::SERVICE_UNAVAILABLE => {
                    retry_after(response.headers()).map_or(Reply::Temporary, Reply::Throttled)
                }
                _ => handle(response).await,
            },
        )
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

    pub(crate) async fn send_guarded<T>(
        &self,
        kind: RequestKind,
        request: Request<Full<Bytes>>,
        deadline: Duration,
        authorize: impl Fn() -> Result<(), Failure> + Sync,
        trace: Option<DispatchTrace<'_>>,
        handle: impl AsyncFnOnce(&mut Response<Incoming>) -> Reply<T>,
    ) -> Result<Reply<T>, Failure> {
        self.send_guarded_artifact(
            kind,
            request,
            deadline,
            authorize,
            ArtifactAdmission {
                trace,
                ..ArtifactAdmission::default()
            },
            handle,
        )
        .await
    }

    async fn send_guarded_artifact<T>(
        &self,
        kind: RequestKind,
        request: Request<Full<Bytes>>,
        deadline: Duration,
        authorize: impl Fn() -> Result<(), Failure> + Sync,
        admission: ArtifactAdmission<'_>,
        handle: impl AsyncFnOnce(&mut Response<Incoming>) -> Reply<T>,
    ) -> Result<Reply<T>, Failure> {
        let method = match *request.method() {
            hyper::Method::GET => Some("GET"),
            hyper::Method::POST => Some("POST"),
            _ => None,
        };
        let mut dispatched = None;
        let mut exchange_finished = false;
        let result = self
            .gate
            .handoff_artifact(
                kind,
                deadline,
                authorize,
                admission.shared,
                admission.pause,
                async |_| {
                    // The gate has reserved this handoff before the operation starts.
                    dispatched = admission.trace.map(|trace| {
                        let mut span = trace
                            .context
                            .observer
                            .start(trace.stage, Some(trace.context.parent));
                        if let Some(method) = method {
                            span.http_request(method, Some(trace.route));
                        }
                        span
                    });
                    let reply = match timeout(Duration::from_secs(30), self.client.request(request))
                        .await
                    {
                        Ok(Ok(mut response)) => {
                            if let Some(span) = &mut dispatched {
                                span.http_status(response.status().as_u16());
                            }
                            handle(&mut response).await
                        }
                        _ => Reply::Temporary,
                    };
                    exchange_finished = true;
                    reply
                },
            )
            .await;
        if let Some(mut span) = dispatched {
            if !exchange_finished {
                span.event(Event::ProviderRequestDeadline);
            }
            span.finish(matches!(&result, Ok(Reply::Success(_))));
        }
        result
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
