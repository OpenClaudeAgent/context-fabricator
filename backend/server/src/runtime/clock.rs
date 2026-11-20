use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::time::{Instant, sleep};

#[derive(Clone, Copy)]
pub struct Clock {
    epoch_ms: i64,
    started: Instant,
}

impl Clock {
    #[must_use]
    pub fn system() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self::anchored(milliseconds(now))
    }

    #[must_use]
    pub fn anchored(epoch_ms: i64) -> Self {
        Self {
            epoch_ms,
            started: Instant::now(),
        }
    }

    #[must_use]
    pub fn now(self) -> i64 {
        self.epoch_ms
            .saturating_add(milliseconds(self.started.elapsed()))
    }

    pub async fn wait_until(self, due_at: i64) {
        loop {
            let delay = due_at.saturating_sub(self.now());
            if delay <= 0 {
                return;
            }
            sleep(Duration::from_millis(delay.cast_unsigned().min(60_000))).await;
        }
    }
}

pub(crate) fn milliseconds(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}
