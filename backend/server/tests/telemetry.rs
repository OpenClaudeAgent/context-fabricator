use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use opentelemetry::KeyValue;
use opentelemetry::logs::{LogRecord, Logger, LoggerProvider};
use opentelemetry::metrics::MeterProvider;
use opentelemetry::trace::{Span, Tracer, TracerProvider};
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
use opentelemetry_sdk::logs::{
    BatchConfigBuilder, BatchLogProcessor, InMemoryLogExporter, LogBatch, LogExporter,
    SdkLoggerProvider,
};
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

#[test]
fn embedded_sdk_exports_correlated_signals_and_flushes() -> Result<(), Box<dyn Error>> {
    let span_exporter = InMemorySpanExporter::default();
    let log_exporter = InMemoryLogExporter::default();
    let metric_exporter = InMemoryMetricExporter::default();
    let tracer = SdkTracerProvider::builder()
        .with_batch_exporter(span_exporter.clone())
        .build();
    let logs = SdkLoggerProvider::builder()
        .with_batch_exporter(log_exporter.clone())
        .build();
    let metrics = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(metric_exporter.clone()).build())
        .build();

    let mut span = tracer.tracer("local_backend").start("fixture");
    let category = KeyValue::new("operation", "fixture");
    span.set_attribute(category.clone());
    let context = span.span_context().clone();
    let logger = logs.logger("local_backend");
    let mut record = logger.create_log_record();
    record.set_body("fixture completed".into());
    record.add_attribute("operation", "fixture");
    record.set_trace_context(
        context.trace_id(),
        context.span_id(),
        Some(context.trace_flags()),
    );
    logger.emit(record);
    metrics
        .meter("local_backend")
        .u64_counter("local.operations")
        .build()
        .add(1, std::slice::from_ref(&category));
    span.end();
    tracer.force_flush()?;
    logs.force_flush()?;
    metrics.force_flush()?;

    let exported_spans = span_exporter.get_finished_spans()?;
    let exported_logs = log_exporter.get_emitted_logs()?;
    let exported_metrics = metric_exporter.get_finished_metrics()?;
    assert_eq!(exported_spans.len(), 1);
    assert!(
        exported_spans
            .iter()
            .any(|span| span.attributes.contains(&category))
    );
    assert_eq!(
        exported_spans
            .first()
            .map(|exported| exported.span_context.trace_id()),
        Some(context.trace_id())
    );
    assert_eq!(exported_logs.len(), 1);
    assert!(exported_logs.iter().any(|log| {
        log.record
            .attributes_iter()
            .any(|(key, value)| key.as_str() == "operation" && *value == "fixture".into())
    }));
    assert_eq!(
        exported_logs
            .first()
            .and_then(|exported| exported.record.trace_context())
            .map(|trace| trace.trace_id),
        Some(context.trace_id())
    );
    assert!(
        exported_metrics
            .iter()
            .flat_map(opentelemetry_sdk::metrics::data::ResourceMetrics::scope_metrics)
            .flat_map(opentelemetry_sdk::metrics::data::ScopeMetrics::metrics)
            .filter(|metric| metric.name() == "local.operations")
            .any(|metric| {
                let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                    return false;
                };
                sum.data_points().any(|point| {
                    point.value() == 1 && point.attributes().any(|attribute| *attribute == category)
                })
            })
    );
    metrics.shutdown()?;
    logs.shutdown()?;
    tracer.shutdown()?;
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct Unavailable(Arc<AtomicUsize>);

impl LogExporter for Unavailable {
    fn export(&self, _batch: LogBatch<'_>) -> impl Future<Output = OTelSdkResult> {
        self.0.fetch_add(1, Ordering::Relaxed);
        std::future::ready(Err(OTelSdkError::InternalFailure(
            "fixture sink unavailable".to_owned(),
        )))
    }
}

#[test]
fn failed_export_is_reported_on_flush() -> Result<(), Box<dyn Error>> {
    let exporter = Unavailable::default();
    let logs = SdkLoggerProvider::builder()
        .with_batch_exporter(exporter.clone())
        .build();
    let logger = logs.logger("local_backend");
    let mut record = logger.create_log_record();
    record.set_body("unavailable sink exercise".into());
    logger.emit(record);
    let Err(OTelSdkError::InternalFailure(message)) = logs.force_flush() else {
        return Err("the logger did not report the failed export on flush".into());
    };
    assert!(message.contains("fixture sink unavailable"));
    eprintln!("OpenTelemetry reported the fixture export failure: {message}");
    assert_eq!(exporter.0.load(Ordering::Relaxed), 1);
    logs.shutdown()?;
    Ok(())
}

#[derive(Clone, Debug, Default)]
struct DrainSink(Arc<AtomicUsize>);

impl LogExporter for DrainSink {
    fn export(&self, batch: LogBatch<'_>) -> impl Future<Output = OTelSdkResult> {
        self.0.fetch_add(batch.iter().count(), Ordering::Relaxed);
        std::future::ready(Ok(()))
    }
}

#[test]
fn shutdown_drains_a_buffered_log_batch() -> Result<(), Box<dyn Error>> {
    let sink = DrainSink::default();
    let processor = BatchLogProcessor::builder(sink.clone())
        .with_batch_config(
            BatchConfigBuilder::default()
                .with_scheduled_delay(Duration::from_secs(60))
                .with_max_export_batch_size(8)
                .build(),
        )
        .build();
    let logs = SdkLoggerProvider::builder()
        .with_log_processor(processor)
        .build();
    let logger = logs.logger("local_backend");
    let mut record = logger.create_log_record();
    record.set_body("queued until shutdown".into());
    logger.emit(record);
    assert_eq!(sink.0.load(Ordering::Relaxed), 0);
    logs.shutdown()?;
    assert_eq!(sink.0.load(Ordering::Relaxed), 1);
    Ok(())
}
