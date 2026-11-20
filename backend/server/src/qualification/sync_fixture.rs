use std::num::NonZeroU16;
use std::sync::Arc;
use std::time::Duration;

use ctxfab_server::adapters::outbound::sqlite::SqliteStore;
use ctxfab_server::application::{
    sync::{CollectionJobs, PageFetch, PageSource},
    telemetry::Observer,
};
use ctxfab_server::domain::corpus::{Record, RecordKind};
use ctxfab_server::domain::sync::{JobTransition, Page, Reply, RequestKind};
use ctxfab_server::runtime::{
    clock::Clock,
    outbound::{OutboundGate, OutboundPool, QuotaPolicy},
    scheduler::{CollectionJob, Refresh, Scheduler},
};

pub fn register(
    mut scheduler: Scheduler,
    store: Arc<SqliteStore>,
    pool: &OutboundPool,
    clock: Clock,
    observer: Option<Arc<dyn Observer>>,
    manual: bool,
) -> Result<(Scheduler, Refresh), Box<dyn std::error::Error>> {
    if manual {
        store.transition_job(
            "qualification",
            0,
            JobTransition::Queue(clock.now().saturating_add(60_000)),
        )?;
    }
    let gate = pool.gate(
        "qualification",
        QuotaPolicy {
            concurrency: NonZeroU16::MIN,
            spacing: Duration::from_millis(150),
        },
    )?;
    let refresh = scheduler.register(
        CollectionJob::new(
            "qualification".into(),
            Arc::new(Fixture { manual, gate }),
            store,
            clock,
            Duration::from_secs(if manual { 60 } else { 1 }),
        )
        .with_observer(observer),
    );
    Ok((scheduler, refresh))
}

struct Fixture {
    manual: bool,
    gate: Arc<OutboundGate>,
}

impl PageSource for Fixture {
    fn fetch<'a>(&'a self, cursor: Option<&'a str>, kind: RequestKind) -> PageFetch<'a> {
        Box::pin(self.gate.handoff(kind, async move |_| {
            let (next, document, more) = match cursor {
                None if self.manual => (
                    "fixture-3",
                    Some(("second", RecordKind::History, "incrementalfixture")),
                    false,
                ),
                None => (
                    "fixture-1",
                    Some(("initial", RecordKind::History, "baselinefixture")),
                    true,
                ),
                Some("fixture-1") => (
                    "fixture-2",
                    Some((
                        "first",
                        RecordKind::Current {
                            tracked_ref: "refs/heads/main".into(),
                        },
                        "firstfixture",
                    )),
                    false,
                ),
                Some("fixture-2") => (
                    "fixture-3",
                    Some((
                        "second",
                        RecordKind::Current {
                            tracked_ref: "refs/heads/main".into(),
                        },
                        "incrementalfixture",
                    )),
                    false,
                ),
                Some("fixture-3") => return std::future::pending().await,
                Some(_) => return Reply::ExpiredCursor,
            };
            let documents = document
                .into_iter()
                .map(|(revision, kind, content)| Record {
                    source: "fixture:scope:src/main.rs".into(),
                    revision: revision.into(),
                    kind,
                    title: "Local collection fixture".into(),
                    content: content.into(),
                })
                .collect();
            Reply::Success(Page {
                cursor: next.into(),
                documents,
                more,
            })
        }))
    }
}
