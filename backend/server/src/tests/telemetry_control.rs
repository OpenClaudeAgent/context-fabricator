use super::*;

#[test]
fn a_debug_override_expires_and_shutdown_drains_correlated_signals()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let file = directory.path().join("telemetry.sqlite3");
    let telemetry = EmbeddedTelemetry::open(&file, 24, Limits::default())?;
    let enabled = telemetry.command(Command::EnableDebug)?;
    assert!(enabled.health.debug_until > now());
    let first = telemetry.start(Stage::ApiQuery, None);
    let debug_id = first.link().trace;
    first.finish(true);
    telemetry.store.debug_until.store(now() - 1, Relaxed);
    assert_eq!(telemetry.snapshot().health.debug_until, 0);
    let second = telemetry.start(Stage::ApiQuery, None);
    let normal_id = second.link().trace;
    let child = telemetry.start(Stage::LexicalLookup, Some(second.link()));
    child.finish(true);
    second.finish(true);
    telemetry.shutdown();

    let persisted = telemetry.snapshot();
    let debug_id = TraceId::from_bytes(debug_id).to_string();
    let normal_id = TraceId::from_bytes(normal_id).to_string();
    assert!(
        persisted
            .logs
            .iter()
            .any(|row| row.trace_id.as_deref() == Some(&debug_id)
                && row.level.as_deref() == Some("debug"))
    );
    assert!(
        !persisted
            .logs
            .iter()
            .any(|row| row.trace_id.as_deref() == Some(&normal_id)
                && row.level.as_deref() == Some("debug"))
    );
    assert!(
        persisted
            .traces
            .iter()
            .any(|row| row.trace_id.as_deref() == Some(&normal_id) && row.parent_span_id.is_some())
    );
    assert!(
        persisted
            .metrics
            .iter()
            .any(|row| row.name == "api_query" && row.value == Some(2))
    );
    assert_eq!(persisted.health.queued, 0);
    Ok(())
}
