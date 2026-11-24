//! Shared admission control for inference artifact downloads and installation.
//!
//! A budget must be shared by all operations that compete for the same space.
//! Snapshots may already include bytes written by an active reservation; counting
//! its full reservation again is intentional and conservative until it is dropped.

use std::sync::{Arc, Mutex};

use serde::Serialize;

mod plan;
pub use plan::StoragePlan;

/// A finite snapshot of the storage policy and filesystem capacity, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CapacitySnapshot {
    pub logical_limit: u64,
    pub logical_usage: u64,
    pub physical_available: u64,
    /// Space reserved for SQLite and other system needs.
    pub system_reserve: u64,
}

/// Application port for obtaining a fresh capacity snapshot.
pub trait CapacityProbe: Send + Sync {
    type Error;

    /// # Errors
    /// Returns an error when a reliable capacity snapshot cannot be obtained.
    fn snapshot(&self) -> Result<CapacitySnapshot, Self::Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageCapacity {
    pub limit_bytes: u64,
    pub used_bytes: u64,
    pub reserved_bytes: u64,
    pub available_bytes: u64,
    pub volume_free_bytes: u64,
    pub system_reserve_bytes: u64,
}

#[derive(Debug, thiserror::Error)]
#[error("Inference storage capacity is unavailable.")]
pub struct CapacityUnavailable;

/// Read-side port for the same capacity used by write admission.
pub trait StorageReport: Send + Sync {
    /// # Errors
    /// Returns an error if current capacity cannot be sampled.
    fn capacity(&self) -> Result<StorageCapacity, CapacityUnavailable>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum AdmissionError<E> {
    Probe(E),
    Unavailable { required: u64, available: u64 },
    Poisoned,
}

struct State<P> {
    probe: P,
    reserved: u64,
}

impl<P: CapacityProbe> State<P> {
    fn capacity(&self) -> Result<StorageCapacity, AdmissionError<P::Error>> {
        let snapshot = self.probe.snapshot().map_err(AdmissionError::Probe)?;
        let available_bytes = snapshot
            .logical_limit
            .saturating_sub(snapshot.logical_usage)
            .min(
                snapshot
                    .physical_available
                    .saturating_sub(snapshot.system_reserve),
            )
            .saturating_sub(self.reserved);
        Ok(StorageCapacity {
            limit_bytes: snapshot.logical_limit,
            used_bytes: snapshot.logical_usage,
            reserved_bytes: self.reserved,
            available_bytes,
            volume_free_bytes: snapshot.physical_available,
            system_reserve_bytes: snapshot.system_reserve,
        })
    }
}

/// Clones share reservations and serialize probing with admission.
pub struct ArtifactStorageBudget<P> {
    state: Arc<Mutex<State<P>>>,
}

impl<P> Clone for ArtifactStorageBudget<P> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
        }
    }
}

impl<P: CapacityProbe> ArtifactStorageBudget<P> {
    pub fn new(probe: P) -> Self {
        Self {
            state: Arc::new(Mutex::new(State { probe, reserved: 0 })),
        }
    }

    /// Atomically check capacity and reserve it until the returned guard drops.
    ///
    /// # Errors
    /// Returns a probe failure, a poisoned lock, or the required and available
    /// byte counts when the operation cannot be admitted.
    pub fn try_reserve(&self, required: u64) -> Result<Reservation<P>, AdmissionError<P::Error>> {
        let mut state = self.state.lock().map_err(|_| AdmissionError::Poisoned)?;
        let available = state.capacity()?.available_bytes;
        if required > available {
            return Err(AdmissionError::Unavailable {
                required,
                available,
            });
        }
        // Since required <= available, the sum cannot exceed either finite capacity.
        state.reserved += required;
        drop(state);
        Ok(Reservation {
            state: Arc::clone(&self.state),
            bytes: required,
        })
    }
}

impl<P: CapacityProbe> StorageReport for ArtifactStorageBudget<P> {
    fn capacity(&self) -> Result<StorageCapacity, CapacityUnavailable> {
        self.state
            .lock()
            .map_err(|_| CapacityUnavailable)?
            .capacity()
            .map_err(|_| CapacityUnavailable)
    }
}

/// Owns an admission; dropping it releases the reservation, including on failure
/// or cancellation of the operation that owns it.
pub struct Reservation<P> {
    state: Arc<Mutex<State<P>>>,
    bytes: u64,
}

impl<P> Drop for Reservation<P> {
    fn drop(&mut self) {
        // Releasing capacity is safe even if another holder panicked.
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.reserved = state.reserved.saturating_sub(self.bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Barrier,
        atomic::{AtomicBool, Ordering},
    };
    use std::thread;

    struct Probe {
        snapshot: CapacitySnapshot,
        fail: AtomicBool,
    }

    impl CapacityProbe for Probe {
        type Error = &'static str;
        fn snapshot(&self) -> Result<CapacitySnapshot, Self::Error> {
            if self.fail.load(Ordering::SeqCst) {
                Err("probe failed")
            } else {
                Ok(self.snapshot)
            }
        }
    }

    fn budget() -> ArtifactStorageBudget<Probe> {
        ArtifactStorageBudget::new(Probe {
            snapshot: CapacitySnapshot {
                logical_limit: 200,
                logical_usage: 40,
                physical_available: 130,
                system_reserve: 30,
            },
            fail: AtomicBool::new(false),
        })
    }

    fn failed_operation(budget: &ArtifactStorageBudget<Probe>) -> Result<(), &'static str> {
        let _reservation = budget.try_reserve(100).map_err(|_| "admission failed")?;
        Err("download failed")
    }

    #[test]
    fn concurrent_operations_cannot_oversubscribe() -> Result<(), &'static str> {
        let budget = budget();
        let start = Arc::new(Barrier::new(2));
        let held = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let budget = budget.clone();
                let start = Arc::clone(&start);
                let held = Arc::clone(&held);
                thread::spawn(move || {
                    start.wait();
                    let result = budget.try_reserve(60);
                    held.wait();
                    result.is_ok()
                })
            })
            .collect();
        let successes = handles
            .into_iter()
            .map(|h| h.join().map_err(|_| "thread panicked"))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(successes.into_iter().filter(|success| *success).count(), 1);
        Ok(())
    }

    #[test]
    fn rejection_reports_exact_remaining_capacity() -> Result<(), &'static str> {
        let budget = budget();
        let _first = budget.try_reserve(65).map_err(|_| "admission failed")?;
        assert!(matches!(
            budget.try_reserve(36),
            Err(AdmissionError::Unavailable {
                required: 36,
                available: 35
            })
        ));
        Ok(())
    }

    #[test]
    fn dropping_on_failure_or_cancellation_releases_reservation() -> Result<(), &'static str> {
        let budget = budget();
        assert_eq!(failed_operation(&budget), Err("download failed"));
        let cancelled = budget.try_reserve(100).map_err(|_| "admission failed")?;
        drop(cancelled);
        assert!(budget.try_reserve(100).is_ok());
        Ok(())
    }

    #[test]
    fn probe_failure_does_not_reserve_space_and_subtraction_is_conservative()
    -> Result<(), &'static str> {
        let budget = budget();
        budget
            .state
            .lock()
            .map_err(|_| "lock poisoned")?
            .probe
            .fail
            .store(true, Ordering::SeqCst);
        assert!(matches!(
            budget.try_reserve(1),
            Err(AdmissionError::Probe("probe failed"))
        ));
        budget
            .state
            .lock()
            .map_err(|_| "lock poisoned")?
            .probe
            .fail
            .store(false, Ordering::SeqCst);
        assert!(budget.try_reserve(100).is_ok());
        let depleted = ArtifactStorageBudget::new(Probe {
            snapshot: CapacitySnapshot {
                logical_limit: 1,
                logical_usage: 2,
                physical_available: 3,
                system_reserve: 4,
            },
            fail: AtomicBool::new(false),
        });
        assert!(matches!(
            depleted.try_reserve(1),
            Err(AdmissionError::Unavailable {
                required: 1,
                available: 0
            })
        ));
        Ok(())
    }
}
