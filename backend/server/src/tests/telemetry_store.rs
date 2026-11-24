use std::error::Error;

use crate::adapters::outbound::sqlite::SqliteStore;
use crate::application::telemetry::{LogFilter, LogLevel};
use crate::application::{ports::Repository, sync::TrafficLedger};
use crate::domain::sync::Admission;

use super::*;

#[test]
fn retained_traces_report_legacy_outcomes_and_classify_successful_background_checks()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("telemetry.sqlite3");
    let store = LocalStore::open(&path, Limits::default(), 1)?;
    let time = now();
    let trace_id = "f0010000000000000000000000000000";
    let database = Connection::open(path)?;
    for (span, name, level, metadata) in [
        (
            "0000000000000001",
            "api_answer",
            Some("ok"),
            serde_json::json!({
                "events": [{"name": "Answer withheld for insufficient evidence", "offsetMicros": 10}],
                "httpMethod": "POST", "httpRoute": "/v1/answers", "httpStatus": 200,
            }),
        ),
        (
            "0000000000000002",
            "api_request",
            Some("ok"),
            serde_json::json!({"events": [], "httpMethod": "GET", "httpRoute": "/v1/github", "httpStatus": 200}),
        ),
        (
            "0000000000000003",
            "api_request",
            Some("error"),
            serde_json::json!({"events": [], "httpMethod": "GET", "httpRoute": "/v1/github", "httpStatus": 500}),
        ),
        (
            "0000000000000004",
            "api_request",
            None,
            serde_json::json!({"events": [], "httpMethod": "GET", "httpRoute": "/v1/github"}),
        ),
        (
            "0000000000000005",
            "api_request",
            Some("ok"),
            serde_json::json!({"events": [], "httpMethod": "PUT", "httpRoute": "/v1/settings", "httpStatus": 200}),
        ),
        (
            "0000000000000006",
            "api_answer",
            Some("ok"),
            serde_json::json!({"events": [], "answerOutcome": "insufficientEvidence", "httpStatus": 200}),
        ),
    ] {
        let mut metadata = metadata;
        metadata["startedAtUnixMicros"] = serde_json::json!(time * 1_000_000);
        database.execute(
            "INSERT INTO signals (signal, timestamp, name, value, level, trace_id, span_id, span_timing)
             VALUES ('trace', ?1, ?2, 20, ?3, ?4, ?5, ?6)",
            rusqlite::params![time, name, level, trace_id, span, metadata.to_string()],
        )?;
    }
    let summary = serde_json::to_value(store.snapshot(time).traces)?;
    let detail = serde_json::to_value(store.trace(trace_id, None, time)?.spans)?;
    for samples in [&summary, &detail] {
        assert_eq!(samples[5]["spanOutcome"], "insufficientEvidence");
        assert_eq!(samples[5]["level"], "ok");
        assert_eq!(samples[4]["traceRequestKind"], "backgroundCheck");
        assert_eq!(samples[4]["spanOutcome"], "succeeded");
        assert!(samples[3].get("traceRequestKind").is_none());
        assert_eq!(samples[3]["spanOutcome"], "failed");
        assert!(samples[2].get("traceRequestKind").is_none());
        assert_eq!(samples[2]["spanOutcome"], "unknown");
        assert!(samples[1].get("traceRequestKind").is_none());
        assert_eq!(samples[0]["spanOutcome"], "insufficientEvidence");
        assert!(samples[0].get("answerOutcome").is_none());
    }
    assert_eq!(summary[5]["events"], serde_json::json!([]));
    assert_eq!(detail[5]["events"].as_array().map(Vec::len), Some(1));
    Ok(())
}

#[test]
fn metrics_keep_the_latest_retained_count_for_each_operation() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let store = LocalStore::open(
        &directory.path().join("telemetry.sqlite3"),
        Limits::default(),
        1,
    )?;
    let time = now();
    let metric = |stage, timestamp, value| {
        let mut row = Row::new(Signal::Metric, stage, timestamp);
        row.value = Some(value);
        row
    };
    let mut rows = vec![
        metric("answer_generation", time - 3601, 3),
        metric("api_query", time - 300, 7),
    ];
    rows.extend((455..=519).map(|value| metric("api_request", time - 1, value)));
    rows.push(metric("api_request", time, 4));
    store.write(&rows, time)?;

    let snapshot = store.snapshot(time);
    let observations = snapshot
        .metrics
        .iter()
        .map(|record| (record.name.as_str(), record.timestamp, record.value))
        .collect::<Vec<_>>();
    assert_eq!(
        observations,
        [
            ("api_request", time, Some(4)),
            ("api_query", time - 300, Some(7))
        ]
    );
    let descriptor = snapshot.metrics[0].span_details.as_ref();
    assert_eq!(
        descriptor.map(|details| details.display_name),
        Some("Local API request")
    );
    assert_eq!(
        descriptor.map(|details| details.operation),
        Some("http.request")
    );
    Ok(())
}

#[test]
fn retained_logs_filter_before_paginating() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let store = LocalStore::open(
        &directory.path().join("telemetry.sqlite3"),
        Limits::default(),
        1,
    )?;
    let time = now();
    let mut first = Row::new(Signal::Log, "answer_generation", time);
    first.level = Some("error");
    first.trace_id = Some("abcdef0123456789abcdef0123456789".into());
    first.span_id = Some("abcdef0123456789".into());
    store.write(&[first], time)?;
    for _ in 0..64 {
        let mut log = Row::new(Signal::Log, "api_query", time);
        log.level = Some("info");
        store.write(&[log], time)?;
    }
    let mut unrelated_span = Row::new(Signal::Trace, "answer_generation", time);
    unrelated_span.trace_id = Some("11111111111111111111111111111111".into());
    unrelated_span.span_id = Some("abcdef0123456789".into());
    unrelated_span.value = Some(10);
    store.write(
        &[Row::new(Signal::Metric, "api_query", time), unrelated_span],
        time,
    )?;
    let filter = LogFilter {
        query: Some("ABCDEF".into()),
        level: Some(LogLevel::Error),
        before_id: None,
    };
    let matched = store.logs(&filter, time)?;
    assert_eq!(matched.records.len(), 1);
    assert_eq!(matched.records[0].name, "answer_generation");
    assert_eq!(matched.records[0].level, Some("error".into()));
    assert_eq!(matched.records[0].value, None);
    assert!(
        matched.records[0]
            .span_details
            .as_ref()
            .is_some_and(|details| {
                details.display_name == "Generate answer" && details.timing.is_none()
            })
    );
    assert!(matched.next_before_id.is_none());
    assert!(store.logs(&filter, time + 3601)?.records.is_empty());

    let only_level = store.logs(
        &LogFilter {
            query: None,
            ..filter
        },
        time,
    )?;
    assert!(only_level.records.iter().map(|record| record.id).eq([1]));
    let wrong_query = store.logs(
        &LogFilter {
            query: Some("missing".into()),
            ..filter
        },
        time,
    )?;
    assert!(wrong_query.records.is_empty());

    let head = store.logs(&LogFilter::default(), time)?;
    assert_eq!(head.next_before_id, Some(6));
    assert!(
        head.records
            .iter()
            .map(|record| record.id)
            .eq((6..=65).rev())
    );
    let tail = store.logs(
        &LogFilter {
            before_id: head.next_before_id,
            ..LogFilter::default()
        },
        time,
    )?;
    assert!(
        tail.records
            .iter()
            .map(|record| record.id)
            .eq((1..=5).rev())
    );
    assert!(tail.next_before_id.is_none());
    Ok(())
}

#[test]
fn age_purge_and_signal_caps_bound_the_sidecar() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("sidecar.sqlite3");
    let store = LocalStore::open(
        &path,
        Limits {
            pages: 12,
            max_rows_per_signal: 8,
            ..Limits::default()
        },
        1,
    )?;
    let time = now();
    for signal in [Signal::Log, Signal::Metric, Signal::Trace] {
        store.write(&[Row::new(signal, "old", time - 7200)], time - 7200)?;
    }
    assert_eq!(
        store.snapshot(time - 7200).health.oldest_at,
        Some(time - 7200)
    );
    store.maintain(time)?;
    let cleared = store.snapshot(time);
    assert_eq!(cleared.health.pruned, 3);
    assert!(cleared.logs.is_empty() && cleared.metrics.is_empty() && cleared.traces.is_empty());
    for count in 1..=100 {
        let mut metric = Row::new(Signal::Metric, "api_query", time);
        metric.value = Some(count);
        store.write(
            &[
                Row::new(Signal::Log, "api_query", time),
                metric,
                Row::new(Signal::Trace, "api_query", time),
            ],
            time,
        )?;
    }
    let snapshot = store.snapshot(time);
    assert_eq!(
        (
            snapshot.logs.len(),
            snapshot.metrics.len(),
            snapshot.traces.len()
        ),
        (8, 1, 8)
    );
    assert_eq!(snapshot.metrics[0].value, Some(100));
    assert!(snapshot.health.file_bytes <= 12 * PAGE_BYTES);
    assert!(snapshot.health.file_bytes <= snapshot.health.budget_bytes);
    let database = Connection::open(path)?;
    let retained_metrics: i64 = database.query_row(
        "SELECT count(*) FROM signals WHERE signal = 'metric'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(retained_metrics, 8);
    let failures: i64 = database.query_row(
        "SELECT count(*) FROM signals WHERE name = 'old'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(
        failures, 0,
        "expired rows were deleted, not merely filtered"
    );
    println!(
        "Age purge deleted three old records; ninety-two records per signal rolled out under the file cap."
    );
    Ok(())
}

#[test]
fn saturated_queue_and_low_space_surface_gaps_without_changing_admission()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let mut store = LocalStore::open(
        &directory.path().join("telemetry.sqlite3"),
        Limits::default(),
        24,
    )?;
    let corpus = SqliteStore::open(&directory.path().join("corpus.sqlite3"))?;
    let time = now();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    assert!(store.send(&sender, Row::new(Signal::Log, "api_query", time)));
    assert!(!store.send(&sender, Row::new(Signal::Trace, "api_query", time)));
    assert_eq!(store.snapshot(time).health.queued, 1);
    assert_eq!(store.snapshot(time).health.state, "gaps");
    store.limits.free_bytes_reserve = u64::MAX;
    store.flush(&[receiver.recv()?]);
    let degraded = store.snapshot(time);
    assert_eq!(degraded.health.state, "degraded");
    assert_eq!(degraded.health.dropped, 2);
    assert_eq!(degraded.health.queued, 0);
    assert_eq!(
        corpus.reserve("local-qualification", time * 1000, 1000)?,
        Admission::Granted
    );
    assert!(matches!(
        corpus.reserve("local-qualification", time * 1000, 1000)?,
        Admission::WaitUntil(_)
    ));
    corpus.complete(
        "local-qualification",
        crate::domain::sync::Outcome::Succeeded,
        0,
    )?;
    assert_eq!(corpus.status()?.outbound_attempts, 1);
    assert!(store.maintain(time).is_err());
    store.limits.free_bytes_reserve = 0;
    assert!(store.send(&sender, Row::new(Signal::Log, "api_query", time)));
    store.flush(&[receiver.recv()?]);
    let recovered = store.snapshot(time);
    assert_eq!(recovered.health.state, "gaps");
    assert_eq!(recovered.logs.len(), 1);
    println!(
        "Queue and low-space drops remained visible after recovery; request accounting continued."
    );
    Ok(())
}

#[test]
fn slow_reader_blocks_only_the_diagnostic_sink() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("telemetry.sqlite3");
    let store = LocalStore::open(&path, Limits::default(), 24)?;
    let corpus = SqliteStore::open(&directory.path().join("corpus.sqlite3"))?;
    let mut reader = Connection::open(path)?;
    let transaction = reader.transaction()?;
    let rows: i64 = transaction.query_row("SELECT count(*) FROM signals", [], |row| row.get(0))?;
    assert_eq!(rows, 0);
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let time = now();
    assert!(store.send(&sender, Row::new(Signal::Trace, "api_query", time)));
    store.flush(&[receiver.recv()?]);
    assert_eq!(store.snapshot(time).health.dropped, 1);
    corpus.record("fixture", "Corpus diagnostics remain available.")?;
    assert_eq!(corpus.diagnostics()?.len(), 1);
    transaction.commit()?;
    Ok(())
}

#[test]
fn a_busy_queue_still_respects_the_background_flush_interval() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("telemetry.sqlite3");
    let store = LocalStore::open(&path, Limits::default(), 24)?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(BATCH * 2);
    for _ in 0..BATCH * 2 {
        assert!(store.send(&sender, Row::new(Signal::Log, "api_query", now())));
    }
    let started = Instant::now();
    let (rows, elapsed) = std::thread::scope(|scope| {
        let writer = &store;
        scope.spawn(move || writer.worker(&receiver));
        let rows = loop {
            let visible = store.snapshot(now()).logs.len();
            if visible == 60 || started.elapsed() > Duration::from_secs(3) {
                break visible;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let elapsed = started.elapsed();
        store.stop.store(true, Relaxed);
        (rows, elapsed)
    });
    assert_eq!(rows, 60);
    assert!(elapsed >= FLUSH_INTERVAL * 2);
    println!("A full queue still needed two 250 ms flush intervals for two batches.");
    Ok(())
}

#[test]
fn reducing_the_disk_budget_prunes_oldest_and_preserves_the_newest_records()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("telemetry.sqlite3");
    let store = LocalStore::open(&path, Limits::default(), 24)?;
    let time = now();
    for offset in 0..16 {
        let mut row = Row::new(Signal::Log, "older", time - 17 + offset);
        row.trace_id = Some("x".repeat(80_000));
        store.write(&[row], time)?;
    }
    store.write(&[Row::new(Signal::Log, "newest", time)], time)?;
    assert!(store.snapshot(time).health.file_bytes > 1_000_000);

    store.set_policy(2, 24, Some(2));
    let snapshot = store.snapshot(time);
    assert_eq!(snapshot.health.budget_bytes, 2_000_000);
    assert!(snapshot.health.file_bytes <= 2_000_000);
    assert!(snapshot.health.pruned > 0);
    assert_eq!(snapshot.health.state, "ready");
    assert_eq!(snapshot.logs[0].name, "newest");
    assert!(
        store
            .writer
            .lock()
            .map_err(|_| "telemetry writer unavailable")?
            .pragma_query_value(None, "page_count", |row| row.get::<_, u32>(0))?
            <= 116
    );

    store.set_policy(1, 1, Some(50));
    assert_eq!(store.snapshot(time).health.budget_bytes, 2_000_000);
    assert_eq!(store.snapshot(time).health.retention_hours, 24);
    Ok(())
}
