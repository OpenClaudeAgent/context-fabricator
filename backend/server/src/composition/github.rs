use std::num::NonZeroU16;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ctxfab_server::adapters::outbound::{
    github::{BUCKET, GitHubRest},
    sqlite::SqliteStore,
};
use ctxfab_server::application::{github::GitHub, telemetry::Observer};
use ctxfab_server::domain::github::Error;
use ctxfab_server::runtime::{
    clock::Clock,
    entropy::random_uuid,
    outbound::{OutboundPool, QuotaPolicy},
    scheduler::Scheduler,
};

#[cfg(debug_assertions)]
#[path = "../qualification/github_fixture.rs"]
mod github_fixture;

pub(crate) fn connect(
    outbound: &OutboundPool,
    store: Arc<SqliteStore>,
    directory: &Path,
    fixture: Option<(u16, bool)>,
    clock: Clock,
) -> Result<Arc<GitHub>, Box<dyn std::error::Error>> {
    let gate = outbound.gate(
        BUCKET,
        QuotaPolicy {
            concurrency: NonZeroU16::MIN,
            spacing: Duration::from_millis(if fixture.is_some() { 30 } else { 1_000 }),
        },
    )?;
    #[cfg(debug_assertions)]
    let provider = if let Some((port, _)) = fixture {
        GitHubRest::fixture(gate, port)?
    } else {
        GitHubRest::production(gate, directory)?
    };
    #[cfg(not(debug_assertions))]
    let provider = GitHubRest::production(gate, directory)?;
    #[cfg(target_os = "macos")]
    let credentials = {
        use ctxfab_server::adapters::outbound::keychain::Keychain;
        Keychain::noninteractive().ok().map(|keychain| {
            #[cfg(debug_assertions)]
            let keychain = if fixture.is_some() {
                keychain.github_fixture()
            } else {
                keychain
            };
            Arc::new(keychain)
                as Arc<dyn ctxfab_server::application::credentials::CredentialStore + Send + Sync>
        })
    };
    #[cfg(target_os = "linux")]
    let credentials = {
        use ctxfab_server::adapters::outbound::secret_service::SecretServiceStore;
        let store = SecretServiceStore::providers();
        #[cfg(debug_assertions)]
        let store = if fixture.is_some() {
            store.github_fixture()
        } else {
            store
        };
        Some(Arc::new(store)
            as Arc<
                dyn ctxfab_server::application::credentials::CredentialStore + Send + Sync,
            >)
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let credentials = None;
    #[cfg(debug_assertions)]
    let credentials = if matches!(fixture, Some((_, false))) {
        Some(Arc::new(github_fixture::Credentials::default())
            as Arc<
                dyn ctxfab_server::application::credentials::CredentialStore + Send + Sync,
            >)
    } else {
        credentials
    };
    let provider = Arc::new(provider.with_scope_authority(store.clone()));
    Ok(Arc::new(
        GitHub::new(provider.clone(), store.clone(), credentials, || {
            random_uuid()
                .map(|nonce| format!("github/{nonce}"))
                .map_err(|_| Error::Credentials)
        })
        .with_current_code(provider.clone(), store.clone(), move || clock.now())
        .with_history(provider.clone(), store.clone(), move || clock.now())
        .with_actions(provider, store, move || clock.now()),
    ))
}

pub(crate) fn scheduler(
    github: &Arc<GitHub>,
    clock: Clock,
    observer: Option<Arc<dyn Observer>>,
    enabled: bool,
) -> Scheduler {
    let scheduler = Scheduler::default();
    if enabled {
        scheduler.with_github(github.clone(), clock, observer)
    } else {
        scheduler
    }
}
