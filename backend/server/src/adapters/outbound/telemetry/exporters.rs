use std::collections::HashMap;
use std::future::ready;
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use opentelemetry::logs::{AnyValue, Severity};
use opentelemetry::trace::{SpanId, Status};
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{LogBatch, LogExporter};
use opentelemetry_sdk::metrics::Temporality;
use opentelemetry_sdk::metrics::data::{
    AggregatedMetrics, MetricData, ResourceMetrics, ScopeMetrics,
};
use opentelemetry_sdk::metrics::exporter::PushMetricExporter;
use opentelemetry_sdk::trace::{SpanData, SpanExporter};

use crate::application::telemetry::{ClientKind, Event, SpanEvent, SpanOutcome, SpanTiming, Stage};

use super::{
    ANSWER_OUTCOME, CLIENT_KIND, HTTP_METHOD, HTTP_ROUTE, HTTP_STATUS, MODEL_PROFILE, PROBLEM_CODE,
    WORK_TRACE, fusion, generation,
    store::{LocalStore, Row, Signal, now, timestamp},
};

#[derive(Clone, Debug)]
pub(super) struct Ingress {
    store: Arc<LocalStore>,
    sender: SyncSender<Row>,
}

impl Ingress {
    pub fn new(store: Arc<LocalStore>, sender: SyncSender<Row>) -> Self {
        Self { store, sender }
    }

    fn send(&self, row: Row) -> bool {
        self.store.send(&self.sender, row)
    }
}

impl SpanExporter for Ingress {
    fn export(&self, batch: Vec<SpanData>) -> impl Future<Output = OTelSdkResult> + Send {
        for span in batch {
            if let Some(name) = safe_name(&span.name) {
                let mut row = Row::new(Signal::Trace, name, timestamp(span.end_time));
                let duration = elapsed_micros(span.start_time, span.end_time);
                row.value = Some(duration);
                row.level = match span.status {
                    Status::Ok => Some("ok"),
                    Status::Error { .. } => Some("error"),
                    Status::Unset => None,
                };
                row.trace_id = Some(span.span_context.trace_id().to_string());
                row.span_id = Some(span.span_context.span_id().to_string());
                if span.parent_span_id != SpanId::INVALID {
                    row.parent_span_id = Some(span.parent_span_id.to_string());
                }
                row.span_timing = Some(SpanTiming {
                    started_at_unix_micros: elapsed_micros(UNIX_EPOCH, span.start_time),
                    events: span
                        .events
                        .iter()
                        .filter_map(|event| {
                            Event::from_name(&event.name).map(|kind| SpanEvent {
                                name: kind.name().to_owned(),
                                offset_micros: elapsed_micros(span.start_time, event.timestamp)
                                    .min(duration),
                            })
                        })
                        .take(Event::MAX_PER_SPAN as usize)
                        .collect(),
                    answer_outcome: text_attribute(&span.attributes, ANSWER_OUTCOME, 24)
                        .as_deref()
                        .and_then(SpanOutcome::from_code),
                    inference_timing: if name == Stage::AnswerGeneration.code()
                        && row.level == Some("ok")
                    {
                        generation::read(&span.attributes)
                    } else {
                        None
                    },
                    context_budget: if name == Stage::AnswerGeneration.code() {
                        generation::read_context(&span.attributes)
                    } else {
                        None
                    },
                    fusion_ranking: if name == Stage::AnswerRetrieval.code()
                        || name == Stage::VectorRetrieval.code()
                    {
                        fusion::read(&span.attributes)
                    } else {
                        None
                    },
                    retrieval_counts: if name == Stage::AnswerRetrieval.code() {
                        fusion::read_counts(&span.attributes)
                    } else {
                        None
                    },
                    http_status: span.attributes.iter().find_map(|attribute| {
                        if attribute.key.as_str() != HTTP_STATUS {
                            return None;
                        }
                        if let opentelemetry::Value::I64(value @ 100..=599) = attribute.value {
                            return u16::try_from(value).ok();
                        }
                        None
                    }),
                    http_method: text_attribute(&span.attributes, HTTP_METHOD, 7),
                    http_route: text_attribute(&span.attributes, HTTP_ROUTE, 160),
                    client_kind: text_attribute(&span.attributes, CLIENT_KIND, 7)
                        .as_deref()
                        .and_then(ClientKind::from_code),
                    model_profile: text_attribute(&span.attributes, MODEL_PROFILE, 67)
                        .filter(|profile| super::valid_generation_alias(profile)),
                    problem_code: text_attribute(&span.attributes, PROBLEM_CODE, 80),
                    work_trace_id: text_attribute(&span.attributes, WORK_TRACE, 32),
                });
                let _ = self.send(row);
            }
        }
        ready(Ok(()))
    }
}

fn text_attribute(
    attributes: &[opentelemetry::KeyValue],
    key: &str,
    max_len: usize,
) -> Option<String> {
    let attribute = attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == key)?;
    let opentelemetry::Value::String(value) = &attribute.value else {
        return None;
    };
    (value.as_str().len() <= max_len).then(|| value.as_str().to_owned())
}

impl LogExporter for Ingress {
    fn export(&self, batch: LogBatch<'_>) -> impl Future<Output = OTelSdkResult> + Send {
        for (record, _) in batch.iter() {
            let Some(AnyValue::String(body)) = record.body() else {
                continue;
            };
            let Some(name) = safe_name(body.as_str()) else {
                continue;
            };
            let mut row = Row::new(
                Signal::Log,
                name,
                record.timestamp().map_or_else(now, timestamp),
            );
            row.level = Some(match record.severity_number() {
                Some(Severity::Debug) => "debug",
                Some(Severity::Error) => "error",
                _ => "info",
            });
            if let Some(context) = record.trace_context() {
                row.trace_id = Some(context.trace_id.to_string());
                row.span_id = Some(context.span_id.to_string());
            }
            let _ = self.send(row);
        }
        ready(Ok(()))
    }
}

pub(super) struct Metrics {
    ingress: Ingress,
    previous: Mutex<HashMap<&'static str, u64>>,
}

impl Metrics {
    pub fn new(ingress: Ingress) -> Self {
        Self {
            ingress,
            previous: Mutex::default(),
        }
    }
}

impl PushMetricExporter for Metrics {
    fn export(&self, metrics: &ResourceMetrics) -> impl Future<Output = OTelSdkResult> + Send {
        if let Ok(mut previous) = self.previous.lock() {
            for metric in metrics.scope_metrics().flat_map(ScopeMetrics::metrics) {
                let Some(name) = safe_name(metric.name()) else {
                    continue;
                };
                let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                    continue;
                };
                for point in sum.data_points() {
                    let value = point.value();
                    if previous.get(name) != Some(&value) {
                        let mut row = Row::new(Signal::Metric, name, now());
                        row.value = Some(i64::try_from(value).unwrap_or(i64::MAX));
                        if self.ingress.send(row) {
                            previous.insert(name, value);
                        }
                    }
                }
            }
        }
        ready(Ok(()))
    }

    fn force_flush(&self) -> OTelSdkResult {
        Ok(())
    }

    fn shutdown_with_timeout(&self, _: Duration) -> OTelSdkResult {
        Ok(())
    }

    fn temporality(&self) -> Temporality {
        Temporality::Cumulative
    }
}

fn safe_name(value: &str) -> Option<&'static str> {
    Stage::from_code(value).map(Stage::code)
}

fn elapsed_micros(start: SystemTime, end: SystemTime) -> i64 {
    let duration = end.duration_since(start).unwrap_or_default();
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}
