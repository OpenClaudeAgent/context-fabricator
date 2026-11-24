use crate::domain::{github::CollectionActivity, telemetry::TraceLink};
use rusqlite::Row;

pub(super) fn encode(link: TraceLink) -> [u8; 24] {
    let mut bytes = [0; 24];
    let (trace, span) = bytes.split_at_mut(16);
    trace.copy_from_slice(&link.trace);
    span.copy_from_slice(&link.span);
    bytes
}

pub(super) fn decode(bytes: [u8; 24]) -> TraceLink {
    let (trace, span) = bytes.split_at(16);
    let mut link = TraceLink {
        trace: [0; 16],
        span: [0; 8],
    };
    link.trace.copy_from_slice(trace);
    link.span.copy_from_slice(span);
    link
}

pub(super) fn activity(
    row: &Row<'_>,
    latest: &str,
    published: &str,
) -> rusqlite::Result<Option<CollectionActivity>> {
    let latest_attempt = row.get::<_, Option<[u8; 24]>>(latest)?.map(decode);
    let published_snapshot = row.get::<_, Option<[u8; 24]>>(published)?.map(decode);
    Ok(
        (latest_attempt.is_some() || published_snapshot.is_some()).then_some(CollectionActivity {
            latest_attempt,
            published_snapshot,
        }),
    )
}
