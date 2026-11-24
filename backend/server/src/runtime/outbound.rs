//! Shared pacing and accounting for one quota bucket.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::num::NonZeroU16;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as RegistryMutex};
use std::time::Duration;

use tokio::sync::{Mutex, Semaphore};
use tokio::time::timeout;

use super::clock::{Clock, milliseconds};
use crate::application::{Failure, sync::TrafficLedger};
use crate::domain::sync::{
    Admission, Outcome, Reply, RequestKind, Traffic, UNCERTAIN_COOLDOWN_MS, minute_spacing,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuotaPolicy {
    pub concurrency: NonZeroU16,
    pub spacing: Duration,
}

#[derive(Debug, Eq, PartialEq, thiserror::Error)]
pub enum QuotaRegistrationError {
    #[error("Quota bucket already has a different policy.")]
    Conflict,
    #[error("Quota registry is unavailable.")]
    Unavailable,
}

/// The composition root gives all transports in one session the same pool.
pub struct OutboundPool {
    ledger: Arc<dyn TrafficLedger>,
    clock: Clock,
    buckets: RegistryMutex<HashMap<String, (QuotaPolicy, Arc<OutboundGate>)>>,
}

impl OutboundPool {
    #[must_use]
    pub fn new(ledger: Arc<dyn TrafficLedger>, clock: Clock) -> Self {
        Self {
            ledger,
            clock,
            buckets: RegistryMutex::default(),
        }
    }

    /// Gets the unique gateway for a configured quota bucket.
    ///
    /// # Errors
    /// Returns an error if the pool is unavailable or a caller supplies a conflicting policy.
    pub fn gate(
        &self,
        bucket: &str,
        policy: QuotaPolicy,
    ) -> Result<Arc<OutboundGate>, QuotaRegistrationError> {
        let mut buckets = self
            .buckets
            .lock()
            .map_err(|_| QuotaRegistrationError::Unavailable)?;
        match buckets.entry(bucket.to_owned()) {
            Entry::Occupied(entry) => {
                let (active, gate) = entry.get();
                if *active != policy {
                    return Err(QuotaRegistrationError::Conflict);
                }
                Ok(gate.clone())
            }
            Entry::Vacant(entry) => {
                let gate = Arc::new(OutboundGate::new(
                    self.ledger.clone(),
                    bucket.to_owned(),
                    self.clock,
                    policy,
                ));
                entry.insert((policy, gate.clone()));
                Ok(gate)
            }
        }
    }
}

pub struct OutboundGate {
    ledger: Arc<dyn TrafficLedger>,
    bucket: String,
    clock: Clock,
    concurrency: Semaphore,
    concurrency_limit: u16,
    spacing: i64,
    next_handoff: Mutex<i64>,
    queued: AtomicUsize,
}

pub(crate) struct GateStatus {
    pub traffic: Traffic,
    pub queued: usize,
    pub concurrency: u16,
    pub requests_per_minute: i64,
    pub cooldown_until_ms: Option<i64>,
}

impl OutboundGate {
    fn new(
        ledger: Arc<dyn TrafficLedger>,
        bucket: String,
        clock: Clock,
        policy: QuotaPolicy,
    ) -> Self {
        let spacing = milliseconds(policy.spacing).max(1);
        Self {
            ledger,
            bucket,
            clock,
            spacing,
            concurrency: Semaphore::new(usize::from(policy.concurrency.get())),
            concurrency_limit: policy.concurrency.get(),
            // A new process also ramps in with one full interval before its first handoff.
            next_handoff: Mutex::new(clock.now().saturating_add(spacing)),
            queued: AtomicUsize::new(0),
        }
    }

    #[must_use]
    pub fn queued(&self) -> usize {
        self.queued.load(Ordering::Relaxed)
    }

    pub(crate) fn pause(&self, duration: Duration) -> Result<(), Failure> {
        let until = self.clock.now().saturating_add(milliseconds(duration));
        self.ledger.defer(&self.bucket, until)
    }

    pub(crate) fn cooling_down(&self) -> Result<bool, Failure> {
        Ok(self.ledger.traffic(&self.bucket)?.cooldown_until > self.clock.now())
    }

    pub(crate) fn status(&self) -> Result<GateStatus, Failure> {
        let traffic = self.ledger.traffic(&self.bucket)?;
        let spacing = traffic
            .requests_per_minute
            .map_or(self.spacing, |rate| self.spacing.max(minute_spacing(rate)));
        Ok(GateStatus {
            traffic,
            queued: self.queued(),
            concurrency: self.concurrency_limit,
            requests_per_minute: 60_000 / spacing + i64::from(60_000 % spacing != 0),
            cooldown_until_ms: (traffic.cooldown_until > self.clock.now())
                .then_some(traffic.cooldown_until),
        })
    }

    /// Admits one explicit hop and records its reply. A retry or redirect is another hop.
    ///
    /// # Errors
    /// Returns a storage error if the handoff cannot be accounted for.
    pub async fn handoff<T>(
        &self,
        kind: RequestKind,
        operation: impl AsyncFnOnce(RequestKind) -> Reply<T>,
    ) -> Result<Reply<T>, Failure> {
        self.handoff_with_deadline(kind, Duration::from_secs(15), operation)
            .await
    }

    /// Holds the same permit and reservation until a streaming hop completes.
    ///
    /// # Errors
    /// Returns a storage error if accounting cannot be persisted.
    pub async fn handoff_with_deadline<T>(
        &self,
        kind: RequestKind,
        deadline: Duration,
        operation: impl AsyncFnOnce(RequestKind) -> Reply<T>,
    ) -> Result<Reply<T>, Failure> {
        self.handoff_guarded(kind, deadline, || Ok(()), operation)
            .await
    }

    pub(crate) async fn handoff_guarded<T>(
        &self,
        kind: RequestKind,
        deadline: Duration,
        authorize: impl Fn() -> Result<(), Failure> + Sync,
        operation: impl AsyncFnOnce(RequestKind) -> Reply<T>,
    ) -> Result<Reply<T>, Failure> {
        self.handoff_artifact(kind, deadline, authorize, None, None, operation)
            .await
    }

    pub(crate) async fn handoff_artifact<T>(
        &self,
        kind: RequestKind,
        deadline: Duration,
        authorize: impl Fn() -> Result<(), Failure> + Sync,
        shared: Option<&Semaphore>,
        mut pause: Option<tokio::sync::watch::Receiver<bool>>,
        operation: impl AsyncFnOnce(RequestKind) -> Reply<T>,
    ) -> Result<Reply<T>, Failure> {
        let admit = async {
            self.queued.fetch_add(1, Ordering::Relaxed);
            let waiting = Queued(&self.queued);
            let provider = self
                .concurrency
                .acquire()
                .await
                .map_err(|_| Failure::Storage)?;
            let mut next = self.next_handoff.lock().await;
            self.clock.wait_until(*next).await;
            let shared_permit = loop {
                let shared_permit = match shared {
                    Some(slot) => Some(slot.acquire().await.map_err(|_| Failure::Storage)?),
                    None => None,
                };
                authorize()?;
                match self
                    .ledger
                    .reserve(&self.bucket, self.clock.now(), self.spacing)?
                {
                    Admission::Granted => break shared_permit,
                    Admission::WaitUntil(due_at) => {
                        drop(shared_permit);
                        self.clock.wait_until(due_at).await;
                    }
                }
            };
            *next = self.clock.now().saturating_add(self.spacing);
            drop(next);
            let reservation = Reservation(Some(self));
            drop(waiting);
            Ok::<_, Failure>((provider, shared_permit, reservation))
        };
        let (_provider, _shared, reservation) = if let Some(pause) = &mut pause {
            tokio::select! {
                biased;
                changed = pause.wait_for(|paused| *paused) => {
                    return Err(if changed.is_ok() { Failure::Conflict } else { Failure::Storage });
                }
                result = admit => result?,
            }
        } else {
            admit.await?
        };
        let reply = timeout(deadline, operation(kind))
            .await
            .unwrap_or(Reply::Temporary);
        let cool = if let Reply::Throttled(duration) = reply {
            self.clock
                .now()
                .saturating_add(milliseconds(duration.max(Duration::from_secs(1))))
        } else {
            0
        };
        let outcome = if matches!(reply, Reply::Success(_)) {
            Outcome::Succeeded
        } else {
            Outcome::Failed
        };
        reservation.settle(outcome, cool)?;
        Ok(reply)
    }
}

// Owned by the dispatched future and dropped before the concurrency permit.
struct Reservation<'a>(Option<&'a OutboundGate>);

impl Reservation<'_> {
    fn settle(mut self, outcome: Outcome, cooldown_until: i64) -> Result<(), Failure> {
        let gate = self.0.ok_or(Failure::Storage)?;
        gate.ledger
            .complete(&gate.bucket, outcome, cooldown_until)?;
        self.0 = None;
        Ok(())
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if let Some(gate) = self.0 {
            let cooldown = gate.clock.now().saturating_add(UNCERTAIN_COOLDOWN_MS);
            if gate
                .ledger
                .complete(&gate.bucket, Outcome::Uncertain, cooldown)
                .is_err()
            {
                tracing::warn!(
                    code = "outbound_accounting_unavailable",
                    "A transport outcome could not be recorded"
                );
            }
        }
    }
}

struct Queued<'a>(&'a AtomicUsize);

impl Drop for Queued<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}
