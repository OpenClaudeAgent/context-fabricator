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
        let current_github = github.clone();
        let current_observer = observer.clone();
        self.workers.spawn(async move {
            tokio::select! {
                biased;
                _ = stop.wait_for(|stopping| *stopping) => {}
                () = supervise(&current_github, clock, current_observer.as_deref()) => {}
            }
        });
        for (stream, github, observer) in [
            (Metadata::Actions, github.clone(), observer.clone()),
            (Metadata::History, github, observer),
        ] {
            let mut stop = self.stop.subscribe();
            self.workers.spawn(async move {
                tokio::select! {
                    biased;
                    _ = stop.wait_for(|stopping| *stopping) => {}
                    () = supervise_metadata(&github, clock, stream, observer.as_deref()) => {}
                }
            });
        }
        self
    }
}

#[derive(Clone, Copy)]
enum Metadata {
    Actions,
    History,
}

async fn supervise_metadata(
    github: &GitHub,
    clock: Clock,
    stream: Metadata,
    observer: Option<&dyn Observer>,
) {
    loop {
        let next = match stream {
            Metadata::Actions => github.next_actions(),
            Metadata::History => github.next_history(),
        };
        match next {
            Ok(Some(due)) if due.at_ms <= clock.now() => {
                let result = match stream {
                    Metadata::History => github.run_history(&due, observer).await,
                    Metadata::Actions => github.run_actions(&due, observer).await,
                };
                if matches!(result, Err(Error::Storage)) {
                    storage_failure().await;
                }
            }
            Ok(Some(due)) => {
                tokio::select! {
                    () = clock.wait_until(due.at_ms) => {}
                    () = sleep(Duration::from_secs(10)) => {}
                }
            }
            Ok(None) => sleep(Duration::from_secs(10)).await,
            Err(_) => storage_failure().await,
        }
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
