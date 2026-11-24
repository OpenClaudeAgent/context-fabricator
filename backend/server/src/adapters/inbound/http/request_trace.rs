use std::sync::{Arc, Mutex};

use axum::{
    extract::{MatchedPath, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::Response,
};

use super::{
    Api,
    response::{ProblemCode, traced},
};
use crate::application::telemetry::{ClientKind, Event, Observation, Stage, TraceLink};

#[derive(Clone, Default)]
pub(super) struct RequestTrace(Option<Arc<Active>>);

#[derive(Clone, Copy)]
pub(super) struct WorkTrace(pub TraceLink);

struct Active {
    link: TraceLink,
    span: Mutex<Option<Box<dyn Observation>>>,
}

impl RequestTrace {
    pub(super) fn link(&self) -> Option<TraceLink> {
        self.0.as_ref().map(|active| active.link)
    }

    pub(super) fn event(&self, event: Event) {
        self.update(|span| span.event(event));
    }

    pub(super) fn model_profile(&self, profile: &str) {
        self.update(|span| span.model_profile(profile));
    }

    fn update(&self, write: impl FnOnce(&mut dyn Observation)) {
        if let Some(active) = &self.0
            && let Ok(mut span) = active.span.lock()
            && let Some(span) = span.as_mut()
        {
            write(span.as_mut());
        }
    }
}

pub(super) async fn observe(State(api): State<Api>, mut request: Request, next: Next) -> Response {
    // MatchedPath is the route registered by our router, never the request URI.
    let route = request.extensions().get::<MatchedPath>().cloned();
    let route = route.as_ref().map(MatchedPath::as_str);
    let method = method_name(request.method());
    let trace = match &api.telemetry {
        Some(telemetry) if !reads_telemetry(method, route) => {
            let mut span = telemetry.start(stage(method, route), None);
            span.http_request(method, route);
            span.client_kind(
                match request
                    .headers()
                    .get(header::USER_AGENT)
                    .map(HeaderValue::as_bytes)
                {
                    Some(b"ContextFabricator") => ClientKind::Desktop,
                    Some(b"ctxfab") => ClientKind::Cli,
                    _ => ClientKind::Other,
                },
            );
            RequestTrace(Some(Arc::new(Active {
                link: span.link(),
                span: Mutex::new(Some(span)),
            })))
        }
        _ => RequestTrace::default(),
    };
    request.extensions_mut().insert(trace.clone());
    let response = next.run(request).await;
    let mut span = trace.0.and_then(|active| active.span.lock().ok()?.take());
    if let Some(span) = span.as_mut() {
        if let Some(work) = response.extensions().get::<WorkTrace>()
            && work.0.trace != span.link().trace
        {
            span.work_trace(work.0);
        }
        let problem = response
            .extensions()
            .get::<ProblemCode>()
            .map(|code| code.0)
            .or(match (response.status(), route) {
                (StatusCode::NOT_FOUND, None) => Some("route_not_found"),
                (StatusCode::METHOD_NOT_ALLOWED, _) => Some("method_not_allowed"),
                _ => None,
            });
        if let Some(problem) = problem {
            span.problem_code(problem);
        }
    }
    traced(response, span)
}

fn reads_telemetry(method: &str, route: Option<&str>) -> bool {
    matches!(method, "GET" | "HEAD")
        && matches!(
            route,
            Some(
                "/v1/telemetry"
                    | "/v1/telemetry/logs"
                    | "/v1/telemetry/traces/{traceId}"
                    | "/v1/telemetry/answer-usage"
            )
        )
}

fn stage(method: &str, route: Option<&str>) -> Stage {
    match (method, route) {
        ("GET" | "HEAD", Some("/v1/search")) => Stage::ApiQuery,
        ("POST", Some("/v1/answers" | "/v1/benchmarks/answers")) => Stage::ApiAnswer,
        ("DELETE", Some("/v1/answers/{requestId}")) => Stage::AnswerCancellation,
        ("PUT", Some("/v1/generation/models")) => Stage::GenerationSelection,
        ("POST", Some("/v1/retrieval/rebuild")) => Stage::EmbeddingRebuild,
        ("POST", Some("/v1/fixtures/import")) => Stage::ApiImport,
        ("POST", Some("/v1/github/repositories/{id}/refresh")) => Stage::GitHubCurrentCode,
        ("POST", Some("/v1/github/repositories/{id}/history")) => Stage::GitHubHistory,
        ("POST", Some("/v1/github/repositories/{id}/actions")) => Stage::GitHubActions,
        _ => Stage::ApiRequest,
    }
}

fn method_name(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        Method::PATCH => "PATCH",
        Method::OPTIONS => "OPTIONS",
        Method::CONNECT => "CONNECT",
        Method::TRACE => "TRACE",
        _ => "OTHER",
    }
}
