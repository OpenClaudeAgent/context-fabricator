use rusqlite::{Connection, OpenFlags, Row, params};
use std::time::Duration;

use super::{LocalStore, Signal};
use crate::application::telemetry::{
    LogFilter, LogLevel, LogPage, Sample, SpanDetails, Stage, TracePage,
};

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
            "SELECT id, timestamp, name, value, level, trace_id, span_id, parent_span_id
             FROM signals WHERE signal = ?1 AND timestamp >= ?2 ORDER BY id DESC LIMIT 60",
        )?;
        let mut feed = |signal: Signal| {
            read.query_map(params![signal.code(), cutoff], sample)?
                .collect::<rusqlite::Result<Vec<_>>>()
        };
        Ok((
            feed(Signal::Log)?,
            feed(Signal::Metric)?,
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
            "SELECT id, timestamp, name, value, level, trace_id, span_id, parent_span_id
             FROM signals
             WHERE signal = 'log' AND timestamp >= ?1
               AND (?2 IS NULL OR id < ?2)
               AND (?3 IS NULL OR level = ?3)
               AND (?4 = '' OR instr(lower(name), lower(?4)) > 0
                 OR instr(trace_id, lower(?4)) > 0 OR instr(span_id, lower(?4)) > 0
                 OR instr(parent_span_id, lower(?4)) > 0)
             ORDER BY id DESC LIMIT ?5",
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
                sample,
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
                    signal = 'trace', span_timing
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
            let mut record = sample(row)?;
            last_id = Some(record.id);
            if row.get::<_, bool>(8)? {
                if let Some(stage) = Stage::from_code(&record.name) {
                    let (display_name, operation) = stage.descriptor();
                    let timing: Option<String> = row.get(9)?;
                    record.span_details = Some(SpanDetails {
                        display_name,
                        operation,
                        timing: timing
                            .as_deref()
                            .and_then(|timing| serde_json::from_str(timing).ok()),
                    });
                }
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
