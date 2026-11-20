# Embedded observability

This adapter embeds the OpenTelemetry SDK and owns the local SQLite sidecar, bounded ingestion and purge. The [application port](../../../application/telemetry.rs) defines safe stages, observation context and controls. The desktop Diagnostics view consumes the authenticated telemetry API and the backend lifecycle and settings timeline.
