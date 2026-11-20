use std::error::Error;

use crate::adapters::outbound::sqlite::SqliteStore;
use crate::application::telemetry::{LogFilter, LogLevel};
use crate::application::{ports::Repository, sync::TrafficLedger};
use crate::domain::sync::Admission;

use super::*;

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
    store.write(&[first], time)?;
    for _ in 0..64 {
        let mut log = Row::new(Signal::Log, "api_query", time);
        log.level = Some("info");
        store.write(&[log], time)?;
    }
    store.write(
        &[
            Row::new(Signal::Metric, "api_query", time),
            Row::new(Signal::Trace, "api_query", time),
        ],
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
    for _ in 0..100 {
        store.write(
            &[
                Row::new(Signal::Log, "api_query", time),
                Row::new(Signal::Metric, "api_query", time),
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
        (8, 8, 8)
    );
    assert!(snapshot.health.file_bytes <= 12 * PAGE_BYTES);
    assert!(snapshot.health.file_bytes <= snapshot.health.budget_bytes);
    let database = Connection::open(path)?;
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
