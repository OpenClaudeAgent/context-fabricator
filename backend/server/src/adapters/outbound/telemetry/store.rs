use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, SyncSender};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};

use crate::application::{
    Failure,
    telemetry::{Health, Snapshot, SpanTiming},
};

mod read;

const PAGE_BYTES: u64 = 4_096;
const JOURNAL_HEADROOM: u64 = 1_024 * 1_024;
const BATCH: usize = 32;
const FLUSH_INTERVAL: Duration = Duration::from_millis(250);
const MAINTENANCE_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub queue_capacity: usize,
    pub max_rows_per_signal: u32,
    pub pages: u32,
    pub storage_limit_mb: Option<u16>,
    pub free_bytes_reserve: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            queue_capacity: 256,
            max_rows_per_signal: 500,
            pages: 1_024,
            storage_limit_mb: None,
            free_bytes_reserve: 12 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Signal {
    Log,
    Metric,
    Trace,
}

impl Signal {
    const fn code(self) -> &'static str {
        match self {
            Self::Log => "log",
            Self::Metric => "metric",
            Self::Trace => "trace",
        }
    }
}

#[derive(Debug)]
pub(super) struct Row {
    pub signal: Signal,
    pub timestamp: i64,
    pub name: &'static str,
    pub value: Option<i64>,
    pub level: Option<&'static str>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub span_timing: Option<SpanTiming>,
}

impl Row {
    pub fn new(signal: Signal, name: &'static str, timestamp: i64) -> Self {
        Self {
            signal,
            timestamp,
            name,
            value: None,
            level: None,
            trace_id: None,
            span_id: None,
            parent_span_id: None,
            span_timing: None,
        }
    }
}

#[derive(Debug)]
pub(super) struct LocalStore {
    path: PathBuf,
    writer: Mutex<Connection>,
    limits: Limits,
    policy: AtomicU64,
    pub debug_until: AtomicI64,
    dropped: AtomicU64,
    pruned: AtomicU64,
    queued: AtomicUsize,
    degraded: AtomicBool,
    pub stop: AtomicBool,
}

impl LocalStore {
    pub fn open(
        path: &Path,
        limits: Limits,
        hours: u16,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if free_bytes(path.parent().unwrap_or_else(|| Path::new(".")))? < limits.free_bytes_reserve
        {
            return Err(io::Error::other("telemetry storage reserve unavailable").into());
        }
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_millis(50))?;
        connection.pragma_update(None, "page_size", PAGE_BYTES.cast_signed())?;
        connection.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
        connection.pragma_update(None, "journal_mode", "DELETE")?;
        prepare_schema(&mut connection)?;
        let store = Self {
            path: path.to_path_buf(),
            writer: Mutex::new(connection),
            limits,
            policy: AtomicU64::new(pack_policy(0, hours, limits.storage_limit_mb)),
            debug_until: AtomicI64::new(0),
            dropped: AtomicU64::new(0),
            pruned: AtomicU64::new(0),
            queued: AtomicUsize::new(0),
            degraded: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        };
        let _ = store.maintain(now());
        Ok(store)
    }

    pub fn send(&self, sender: &SyncSender<Row>, row: Row) -> bool {
        self.queued.fetch_add(1, Relaxed);
        if sender.try_send(row).is_err() {
            self.queued.fetch_sub(1, Relaxed);
            self.dropped.fetch_add(1, Relaxed);
            return false;
        }
        true
    }

    pub fn worker(&self, receiver: &Receiver<Row>) {
        let mut maintenance = Instant::now();
        while !self.stop.load(Relaxed) {
            if let Ok(row) = receiver.recv_timeout(FLUSH_INTERVAL) {
                let mut batch = vec![row];
                let deadline = Instant::now() + FLUSH_INTERVAL;
                while batch.len() < BATCH && !self.stop.load(Relaxed) {
                    match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    {
                        Ok(row) => batch.push(row),
                        Err(_) => break,
                    }
                }
                if !self.stop.load(Relaxed) {
                    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                }
                self.flush(&batch);
                maintenance = Instant::now();
            } else if maintenance.elapsed() >= MAINTENANCE_INTERVAL {
                let _ = self.maintain(now());
                maintenance = Instant::now();
            }
        }
        loop {
            let batch: Vec<_> = receiver.try_iter().take(BATCH).collect();
            if batch.is_empty() {
                break;
            }
            self.flush(&batch);
        }
    }

    fn flush(&self, batch: &[Row]) {
        self.queued.fetch_sub(batch.len(), Relaxed);
        if self.write(batch, now()).is_err() {
            self.dropped.fetch_add(batch.len() as u64, Relaxed);
        }
    }

    pub fn maintain(&self, time: i64) -> Result<(), Failure> {
        self.write(&[], time)
    }

    pub fn write(&self, rows: &[Row], time: i64) -> Result<(), Failure> {
        let result = self.transact(rows, time);
        self.degraded.store(result.is_err(), Relaxed);
        result
    }

    fn transact(&self, rows: &[Row], time: i64) -> Result<(), Failure> {
        if free_bytes(&self.path).map_err(|_| Failure::Storage)? < self.limits.free_bytes_reserve {
            return Err(Failure::Storage);
        }
        let mut database = self.writer.lock().map_err(|_| Failure::Storage)?;
        let (pages, _) = self.budget();
        let trimmed = enforce_page_budget(&mut database, pages).map_err(|_| Failure::Storage)?;
        self.pruned.fetch_add(trimmed as u64, Relaxed);
        let tx = database.transaction().map_err(|_| Failure::Storage)?;
        {
            let mut insert = tx
                .prepare(
                    "INSERT INTO signals (signal, timestamp, name, value, level, trace_id, span_id,
                                      parent_span_id, span_timing)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )
                .map_err(|_| Failure::Storage)?;
            for row in rows {
                let timing = row
                    .span_timing
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|_| Failure::Storage)?;
                insert
                    .execute(params![
                        row.signal.code(),
                        row.timestamp,
                        row.name,
                        row.value,
                        row.level,
                        row.trace_id,
                        row.span_id,
                        row.parent_span_id,
                        timing,
                    ])
                    .map_err(|_| Failure::Storage)?;
            }
        }
        let expired = tx
            .execute(
                "DELETE FROM signals WHERE timestamp < ?1",
                [self.cutoff(time)],
            )
            .map_err(|_| Failure::Storage)?;
        let mut pruned = expired;
        for signal in [Signal::Log, Signal::Metric, Signal::Trace] {
            pruned += tx
                .execute(
                    "DELETE FROM signals WHERE signal = ?1 AND id < (
                     SELECT id FROM signals WHERE signal = ?1 ORDER BY id DESC LIMIT 1 OFFSET ?2
                 )",
                    params![
                        signal.code(),
                        self.limits.max_rows_per_signal.saturating_sub(1)
                    ],
                )
                .map_err(|_| Failure::Storage)?;
        }
        if pruned != 0 {
            vacuum_pages(&tx, 16).map_err(|_| Failure::Storage)?;
        }
        tx.commit().map_err(|_| Failure::Storage)?;
        self.pruned.fetch_add(pruned as u64, Relaxed);
        Ok(())
    }

    pub fn snapshot(&self, time: i64) -> Snapshot {
        let result = self.read(time);
        let was_unavailable = result.is_err();
        let (logs, metrics, traces, oldest_at) = result.unwrap_or_default();
        let dropped = self.dropped.load(Relaxed);
        let state = if was_unavailable || self.degraded.load(Relaxed) {
            "degraded"
        } else if dropped != 0 {
            "gaps"
        } else {
            "ready"
        };
        let deadline = self.debug_until.load(Relaxed);
        let mut journal = self.path.as_os_str().to_os_string();
        journal.push("-journal");
        Snapshot {
            logs,
            metrics,
            traces,
            health: Health {
                state,
                dropped,
                pruned: self.pruned.load(Relaxed),
                queued: self.queued.load(Relaxed),
                file_bytes: bytes(&self.path) + bytes(Path::new(&journal)),
                budget_bytes: self.budget().1,
                retention_hours: self.retention_hours(),
                debug_until: if deadline > time { deadline } else { 0 },
                oldest_at,
            },
        }
    }

    pub fn set_policy(&self, revision: u32, hours: u16, storage_limit_mb: Option<u16>) {
        let new = pack_policy(revision, hours, storage_limit_mb);
        if self
            .policy
            .try_update(Relaxed, Relaxed, |current| {
                ((current >> 32) < u64::from(revision)).then_some(new)
            })
            .is_ok()
        {
            let _ = self.maintain(now());
        }
    }

    fn budget(&self) -> (u32, u64) {
        let limit = self.policy.load(Relaxed) & u64::from(u16::MAX);
        if limit == 0 {
            return (
                self.limits.pages,
                u64::from(self.limits.pages) * PAGE_BYTES * 2 + JOURNAL_HEADROOM,
            );
        }
        let bytes = limit * 1_000_000;
        let pages = bytes.saturating_sub(JOURNAL_HEADROOM) / (2 * PAGE_BYTES);
        (u32::try_from(pages).unwrap_or(u32::MAX), bytes)
    }

    fn retention_hours(&self) -> u16 {
        let packed = self.policy.load(Relaxed);
        u16::try_from((packed >> 16) & u64::from(u16::MAX)).unwrap_or(24)
    }

    fn cutoff(&self, time: i64) -> i64 {
        time.saturating_sub(i64::from(self.retention_hours()) * 3600)
    }
}

fn prepare_schema(connection: &mut Connection) -> rusqlite::Result<()> {
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS signals (
            id INTEGER PRIMARY KEY, signal TEXT NOT NULL, timestamp INTEGER NOT NULL,
            name TEXT NOT NULL, value INTEGER, level TEXT,
            trace_id TEXT, span_id TEXT, parent_span_id TEXT, span_timing TEXT
        ) STRICT;
        CREATE INDEX IF NOT EXISTS signals_kind ON signals (signal, id);",
    )?;
    let has_span_timing = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('signals') WHERE name = 'span_timing')",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !has_span_timing {
        tx.execute_batch("ALTER TABLE signals ADD COLUMN span_timing TEXT")?;
    }
    tx.commit()
}

fn pack_policy(revision: u32, hours: u16, storage_limit_mb: Option<u16>) -> u64 {
    (u64::from(revision) << 32)
        | (u64::from(hours) << 16)
        | u64::from(storage_limit_mb.unwrap_or(0))
}

fn enforce_page_budget(database: &mut Connection, max_pages: u32) -> rusqlite::Result<usize> {
    fn page_count(database: &Connection) -> rusqlite::Result<u32> {
        database.pragma_query_value(None, "page_count", |row| row.get(0))
    }

    let mut pruned = 0;
    let excess = page_count(database)?.saturating_sub(max_pages);
    if excess > 0 {
        let tx = database.transaction()?;
        vacuum_pages(&tx, excess)?;
        loop {
            let excess = page_count(&tx)?.saturating_sub(max_pages);
            if excess == 0 {
                break;
            }
            let removed = tx.execute(
                "DELETE FROM signals WHERE id IN (
                    SELECT id FROM signals ORDER BY timestamp, id
                    LIMIT (SELECT max(1, min(128, count(*) / 8)) FROM signals)
                 )",
                [],
            )?;
            if removed == 0 {
                return Err(rusqlite::Error::InvalidQuery);
            }
            pruned += removed;
            vacuum_pages(&tx, excess)?;
        }
        tx.commit()?;
    }
    let actual: u32 =
        database.pragma_update_and_check(None, "max_page_count", max_pages, |row| row.get(0))?;
    if actual != max_pages {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(pruned)
}

fn vacuum_pages(database: &Connection, pages: u32) -> rusqlite::Result<()> {
    let mut statement = database.prepare(&format!("PRAGMA incremental_vacuum({pages})"))?;
    let mut rows = statement.query([])?;
    while rows.next()?.is_some() {}
    Ok(())
}

fn free_bytes(path: &Path) -> io::Result<u64> {
    let stats = rustix::fs::statvfs(path)?;
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

fn bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |file| file.len())
}

pub(super) fn now() -> i64 {
    timestamp(SystemTime::now())
}

pub(super) fn timestamp(time: SystemTime) -> i64 {
    i64::try_from(
        time.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .unwrap_or(i64::MAX)
}

#[cfg(test)]
#[path = "../../../tests/telemetry_store.rs"]
mod tests;
