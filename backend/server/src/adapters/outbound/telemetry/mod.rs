//! OpenTelemetry SDK with a bounded in-process SQLite sink.

use std::path::Path;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use opentelemetry::Context;
use opentelemetry::logs::{LogRecord, Logger, LoggerProvider, Severity};
use opentelemetry::metrics::{Counter, MeterProvider};
use opentelemetry::trace::{
    Span, SpanContext, SpanId, Status, TraceContextExt, TraceFlags, TraceId, TraceState, Tracer,
    TracerProvider,
};
use opentelemetry_sdk::logs::{SdkLogger, SdkLoggerProvider};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{Sampler, SdkTracer, SdkTracerProvider};

use crate::application::{
    Failure, Settings,
    telemetry::{
        Command, Event, LogFilter, LogPage, Observation, Observer, Snapshot, Stage, Telemetry,
        TraceLink, TracePage,
    },
};

mod exporters;
mod store;
pub use store::Limits;

use exporters::{Ingress, Metrics};
use store::{LocalStore, now};

const SCOPE: &str = "context_fabricator";

pub struct EmbeddedTelemetry {
    store: Arc<LocalStore>,
    worker: Mutex<Option<JoinHandle<()>>>,
    tracer_provider: SdkTracerProvider,
    logger_provider: SdkLoggerProvider,
    meter_provider: SdkMeterProvider,
    tracer: SdkTracer,
    logger: SdkLogger,
    counters: [Counter<u64>; Stage::ALL.len()],
}

impl EmbeddedTelemetry {
    /// # Errors
    /// Returns a startup error if the isolated store or worker cannot be created.
    pub fn open(
        path: &Path,
        retention_hours: u16,
        limits: Limits,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let store = Arc::new(LocalStore::open(path, limits, retention_hours)?);
        let (sender, receiver) = std::sync::mpsc::sync_channel(limits.queue_capacity);
        let ingress = Ingress::new(store.clone(), sender);
        let writer = store.clone();
        let worker = thread::Builder::new()
            .name("telemetry-writer".into())
            .spawn(move || writer.worker(&receiver))?;
        let tracer_provider = SdkTracerProvider::builder()
            .with_sampler(Sampler::AlwaysOn)
            .with_max_events_per_span(Event::MAX_PER_SPAN)
            .with_simple_exporter(ingress.clone())
            .build();
        let logger_provider = SdkLoggerProvider::builder()
            .with_simple_exporter(ingress.clone())
            .build();
        let meter_provider = SdkMeterProvider::builder()
            .with_reader(
                PeriodicReader::builder(Metrics::new(ingress))
                    .with_interval(Duration::from_secs(2))
                    .build(),
            )
            .build();
        let tracer = tracer_provider.tracer(SCOPE);
        let logger = logger_provider.logger(SCOPE);
        let meter = meter_provider.meter(SCOPE);
        let counters = Stage::ALL.map(|stage| meter.u64_counter(stage.code()).build());
        Ok(Self {
            store,
            worker: Mutex::new(Some(worker)),
            tracer_provider,
            logger_provider,
            meter_provider,
            tracer,
            logger,
            counters,
        })
    }

    pub fn shutdown(&self) {
        let worker = self.worker.lock().ok().and_then(|mut worker| worker.take());
        if let Some(worker) = worker {
            let _ = self.tracer_provider.shutdown();
            let _ = self.logger_provider.shutdown();
            let _ = self.meter_provider.shutdown();
            self.store.stop.store(true, Relaxed);
            let _ = worker.join();
        }
    }
}

impl Drop for EmbeddedTelemetry {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl Observer for EmbeddedTelemetry {
    fn start(&self, stage: Stage, parent: Option<TraceLink>) -> Box<dyn Observation> {
        let span = match parent {
            Some(link) => {
                let context = Context::new().with_remote_span_context(SpanContext::new(
                    TraceId::from_bytes(link.trace),
                    SpanId::from_bytes(link.span),
                    TraceFlags::SAMPLED,
                    false,
                    TraceState::default(),
                ));
                self.tracer.start_with_context(stage.code(), &context)
            }
            None => self.tracer.start(stage.code()),
        };
        Box::new(Operation {
            span,
            stage,
            counter: self.counters[stage as usize].clone(),
            logger: self.logger.clone(),
            store: self.store.clone(),
        })
    }
}

impl Telemetry for EmbeddedTelemetry {
    fn snapshot(&self) -> Snapshot {
        self.store.snapshot(now())
    }

    fn logs(&self, filter: &LogFilter) -> Result<LogPage, Failure> {
        self.store.logs(filter, now()).map_err(|_| Failure::Storage)
    }

    fn trace(&self, trace_id: &str, before_id: Option<i64>) -> Result<TracePage, Failure> {
        self.store
            .trace(trace_id, before_id, now())
            .map_err(|_| Failure::Storage)
    }

    fn command(&self, command: Command) -> Result<Snapshot, Failure> {
        let time = now();
        match command {
            Command::EnableDebug => self
                .store
                .debug_until
                .store(time.saturating_add(300), Relaxed),
            Command::DisableDebug => self.store.debug_until.store(0, Relaxed),
            Command::PurgeExpired => self.store.maintain(time)?,
        }
        Ok(self.store.snapshot(time))
    }

    fn apply_settings(&self, settings: &Settings) {
        self.store.set_policy(
            settings.revision,
            settings.diagnostic_retention_hours,
            settings.diagnostic_storage_limit_mb,
        );
    }
}

struct Operation {
    span: <SdkTracer as Tracer>::Span,
    stage: Stage,
    counter: Counter<u64>,
    logger: SdkLogger,
    store: Arc<LocalStore>,
}

impl Observation for Operation {
    fn link(&self) -> TraceLink {
        let context = self.span.span_context();
        TraceLink {
            trace: context.trace_id().to_bytes(),
            span: context.span_id().to_bytes(),
        }
    }

    fn event(&mut self, event: Event) {
        self.span.add_event(event.name(), Vec::new());
    }

    fn finish(mut self: Box<Self>, success: bool) {
        self.span.set_status(if success {
            Status::Ok
        } else {
            Status::error("operation failed")
        });
        self.log(if success {
            Severity::Info
        } else {
            Severity::Error
        });
        if self.store.debug_until.load(Relaxed) > now() {
            self.log(Severity::Debug);
        }
        self.counter.add(1, &[]);
        self.span.end();
    }
}

impl Operation {
    fn log(&self, severity: Severity) {
        let mut record = self.logger.create_log_record();
        let context = self.span.span_context();
        record.set_body(self.stage.code().into());
        record.set_severity_number(severity);
        record.set_trace_context(
            context.trace_id(),
            context.span_id(),
            Some(context.trace_flags()),
        );
        self.logger.emit(record);
    }
}

#[cfg(test)]
#[path = "../../../tests/telemetry_control.rs"]
mod tests;
