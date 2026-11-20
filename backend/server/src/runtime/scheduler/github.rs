use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;

use super::{Clock, Scheduler};
use crate::application::{github::GitHub, telemetry::Observer};
use crate::domain::github::Error;

impl Scheduler {
    #[must_use]
    pub fn with_github(
        mut self,
        github: Arc<GitHub>,
        clock: Clock,
        observer: Option<Arc<dyn Observer>>,
    ) -> Self {
        let mut stop = self.stop.subscribe();
        self.workers.spawn(async move {
            tokio::select! {
                biased;
                _ = stop.wait_for(|stopping| *stopping) => {}
                () = supervise(&github, clock, observer.as_deref()) => {}
            }
        });
        self
    }
}

async fn supervise(github: &GitHub, clock: Clock, observer: Option<&dyn Observer>) {
    loop {
        let due = match github.next_current_code() {
            Ok(Some(due)) => due,
            Ok(None) => {
                github.wait_for_current_code_change().await;
                continue;
            }
            Err(_) => {
                storage_failure().await;
                continue;
            }
        };
        if due.at_ms > clock.now() {
            tokio::select! {
                () = clock.wait_until(due.at_ms) => {}
                () = github.wait_for_current_code_change() => {}
            }
            continue;
        }
        let result = github.run_current_code(&due, observer).await;
        if matches!(result, Err(Error::Storage)) {
            storage_failure().await;
        }
    }
}

async fn storage_failure() {
    tracing::warn!(
        code = "github_job_storage_unavailable",
        "The GitHub collection job store is unavailable"
    );
    sleep(Duration::from_secs(1)).await;
}
