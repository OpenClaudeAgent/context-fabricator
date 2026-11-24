use rusqlite::Connection;

use crate::application::telemetry::{Sample, SpanDetails, Stage};

pub(super) fn latest_counts(database: &Connection, cutoff: i64) -> rusqlite::Result<Vec<Sample>> {
    // Counters can restart at a lower value. Select the latest stored observation, not the maximum.
    let mut read = database.prepare(
        "SELECT id, timestamp, name, value, level, trace_id, span_id, parent_span_id
         FROM signals
         WHERE id IN (
             SELECT max(id) FROM signals
             WHERE signal = 'metric' AND timestamp >= ?1 AND value IS NOT NULL
             GROUP BY name
         )
         ORDER BY id DESC LIMIT 60",
    )?;
    read.query_map([cutoff], |row| {
        let mut observation = super::sample(row)?;
        if let Some(stage) = Stage::from_code(&observation.name) {
            let (display_name, operation) = stage.descriptor();
            observation.span_details = Some(SpanDetails {
                display_name,
                operation,
                span_outcome: None,
                trace_request_kind: None,
                timing: None,
            });
        }
        Ok(observation)
    })?
    .collect()
}
