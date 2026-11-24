use rusqlite::{Connection, OpenFlags, Row, params};
use std::time::Duration;

use super::{LocalStore, Signal};
use crate::application::telemetry::{
    AnswerUsage, AnswerUsageRecord, LogFilter, LogLevel, LogPage, Sample, SpanDetails, SpanOutcome,
    SpanTiming, Stage, TracePage, TraceRequestKind,
};

mod metrics;

const PAGE_SIZE: u16 = 60;
type Feed = (Vec<Sample>, Vec<Sample>, Vec<Sample>, Option<i64>);

impl LocalStore {
    fn reader(&self) -> rusqlite::Result<Connection> {
        let database = Connection::open_with_flags(&self.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        database.busy_timeout(Duration::from_millis(50))?;
        Ok(database)
    }

    pub(super) fn read(&self, time: i64) -> rusqlite::Result<Feed> {
        let database = self.reader()?;
        let cutoff = self.cutoff(time);
        let oldest = database.query_row(
            "SELECT min(timestamp) FROM signals WHERE timestamp >= ?1",
            [cutoff],
            |row| row.get(0),
        )?;
        let mut read = database.prepare(
            "SELECT id, timestamp, name, value, level, trace_id, span_id, parent_span_id,
                    span_timing
             FROM signals WHERE signal = ?1 AND timestamp >= ?2 ORDER BY id DESC LIMIT 60",
        )?;
        let mut feed = |signal: Signal| {
            read.query_map(params![signal.code(), cutoff], |row| {
                if matches!(signal, Signal::Trace) {
                    span_sample(row, SampleView::Feed)
                } else {
                    sample(row)
                }
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
        };
        Ok((
            feed(Signal::Log)?,
            metrics::latest_counts(&database, cutoff)?,
            feed(Signal::Trace)?,
            oldest,
        ))
    }

    pub(in crate::adapters::outbound::telemetry) fn logs(
        &self,
        filter: &LogFilter,
        time: i64,
    ) -> rusqlite::Result<LogPage> {
        let database = self.reader()?;
        let needle = filter.query.as_deref().map_or("", str::trim);
        let mut read = database.prepare(
            "SELECT log.id, log.timestamp, log.name, coalesce(span.value, log.value),
                    log.level, log.trace_id, log.span_id, log.parent_span_id, span.span_timing
             FROM signals AS log
             LEFT JOIN signals AS span ON span.id = (
                 SELECT id FROM signals
                 WHERE signal = 'trace' AND trace_id = log.trace_id AND span_id = log.span_id
                   AND timestamp >= ?1
                 ORDER BY id DESC LIMIT 1
             )
             WHERE log.signal = 'log' AND log.timestamp >= ?1
               AND (?2 IS NULL OR log.id < ?2)
               AND (?3 IS NULL OR log.level = ?3)
               AND (?4 = '' OR instr(lower(log.name), lower(?4)) > 0
                 OR instr(log.trace_id, lower(?4)) > 0 OR instr(log.span_id, lower(?4)) > 0
                 OR instr(log.parent_span_id, lower(?4)) > 0
                 OR CASE WHEN json_valid(span.span_timing) THEN
                     instr(lower(coalesce(json_extract(span.span_timing, '$.httpMethod'), '') || ' '
                       || coalesce(json_extract(span.span_timing, '$.httpRoute'), '')), lower(?4)) > 0
                     OR instr(lower(json_extract(span.span_timing, '$.problemCode')), lower(?4)) > 0
                   END)
             ORDER BY log.id DESC LIMIT ?5",
        )?;
        let mut records = read
            .query_map(
                params![
                    self.cutoff(time),
                    filter.before_id,
                    filter.level.map(LogLevel::code),
                    needle,
                    PAGE_SIZE + 1
                ],
                |row| span_sample(row, SampleView::Log),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next_before_id = if records.len() > usize::from(PAGE_SIZE) {
            records.truncate(usize::from(PAGE_SIZE));
            records.last().map(|record| record.id)
        } else {
            None
        };
        Ok(LogPage {
            records,
            next_before_id,
        })
    }

    pub(in crate::adapters::outbound::telemetry) fn trace(
        &self,
        trace_id: &str,
        before_id: Option<i64>,
        time: i64,
    ) -> rusqlite::Result<TracePage> {
        let database = self.reader()?;
        let mut read = database.prepare(
            "SELECT id, timestamp, name, value, level, trace_id, span_id, parent_span_id,
                    span_timing, signal = 'trace'
             FROM signals
             WHERE trace_id = ?1 AND signal IN ('trace', 'log') AND timestamp >= ?2
               AND (?3 IS NULL OR id < ?3)
             ORDER BY id DESC LIMIT ?4",
        )?;
        let mut rows = read.query(params![
            trace_id,
            self.cutoff(time),
            before_id,
            PAGE_SIZE + 1
        ])?;
        let mut page = TracePage::default();
        let mut last_id = None;
        for _ in 0..PAGE_SIZE {
            let Some(row) = rows.next()? else {
                return Ok(page);
            };
            let is_span = row.get::<_, bool>(9)?;
            let record = if is_span {
                span_sample(row, SampleView::Trace)?
            } else {
                sample(row)?
            };
            last_id = Some(record.id);
            if is_span {
                page.spans.push(record);
            } else {
                page.logs.push(record);
            }
        }
        if rows.next()?.is_some() {
            page.next_before_id = last_id;
        }
        Ok(page)
    }

    pub(in crate::adapters::outbound::telemetry) fn answer_usage(
        &self,
        time: i64,
    ) -> rusqlite::Result<AnswerUsage> {
        let database = self.reader()?;
        let mut read = database.prepare(
            "SELECT timestamp, value, trace_id, span_timing
             FROM signals
             WHERE signal = 'trace' AND name = ?1 AND parent_span_id IS NULL AND timestamp >= ?2
               AND CASE WHEN json_valid(span_timing)
                   THEN json_extract(span_timing, '$.httpStatus') IS NOT 401 ELSE 1 END
             ORDER BY id DESC LIMIT 500",
        )?;
        let records = read
            .query_map(params![Stage::ApiAnswer.code(), self.cutoff(time)], |row| {
                let timing: Option<String> = row.get(3)?;
                Ok(AnswerUsageRecord {
                    timestamp: row.get(0)?,
                    duration_micros: row.get(1)?,
                    trace_id: row.get(2)?,
                    timing: timing.and_then(|timing| serde_json::from_str(&timing).ok()),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(AnswerUsage::from_records(records))
    }
}

fn sample(row: &Row<'_>) -> rusqlite::Result<Sample> {
    Ok(Sample {
        id: row.get(0)?,
        timestamp: row.get(1)?,
        name: row.get(2)?,
        value: row.get(3)?,
        level: row.get(4)?,
        trace_id: row.get(5)?,
        span_id: row.get(6)?,
        parent_span_id: row.get(7)?,
        span_details: None,
    })
}

#[derive(Clone, Copy)]
enum SampleView {
    Feed,
    Log,
    Trace,
}

fn span_sample(row: &Row<'_>, view: SampleView) -> rusqlite::Result<Sample> {
    let mut record = sample(row)?;
    if let Some(stage) = Stage::from_code(&record.name) {
        let (display_name, operation) = stage.descriptor();
        let raw: Option<String> = row.get(8)?;
        let mut timing: Option<SpanTiming> = raw
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok());
        let (span_outcome, trace_request_kind) = if matches!(view, SampleView::Log) {
            (None, None)
        } else {
            let level = record.level.as_deref();
            let timing = timing.as_ref();
            (
                Some(SpanOutcome::classify(stage, level, timing)),
                TraceRequestKind::classify(stage, level, timing),
            )
        };
        if let Some(timing) = timing.as_mut() {
            timing.answer_outcome = None;
            if !matches!(view, SampleView::Trace) {
                timing.context_budget = None;
                timing.fusion_ranking = None;
                timing.retrieval_counts = None;
                timing.inference_timing = None;
                timing.events.clear();
            }
        }
        record.span_details = Some(SpanDetails {
            display_name,
            operation,
            span_outcome,
            trace_request_kind,
            timing,
        });
    }
    Ok(record)
}
