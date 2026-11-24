//! Linux credentials in the user's Secret Service session.

use std::{collections::HashMap, future::Future, thread, time::Duration};

use secret_service::{EncryptionType, Error, Item, SecretService};
use tokio::runtime::{Builder, Handle, RuntimeFlavor};

use crate::application::credentials::{CredentialError, CredentialStore};

const SERVICE: &str = "org.contextfabricator.providers";
const CONTENT_TYPE: &str = "application/octet-stream";
const TIMEOUT: Duration = Duration::from_secs(3);

pub struct SecretServiceStore {
    service: &'static str,
}

impl SecretServiceStore {
    #[must_use]
    pub const fn providers() -> Self {
        Self { service: SERVICE }
    }

    #[cfg(debug_assertions)]
    #[must_use]
    pub const fn github_fixture(self) -> Self {
        Self {
            service: "org.contextfabricator.providers.github-fixture",
        }
    }

    fn attributes<'a>(&self, reference: &'a str) -> HashMap<&'a str, &'a str> {
        HashMap::from([("application", self.service), ("reference", reference)])
    }
}

impl CredentialStore for SecretServiceStore {
    fn save(&self, reference: &str, secret: &[u8]) -> Result<(), CredentialError> {
        on_session_bus(async {
            let service = connect().await?;
            let attributes = self.attributes(reference);
            if let Some(existing) = find(&service, attributes.clone()).await? {
                return existing
                    .set_secret(secret, CONTENT_TYPE)
                    .await
                    .map_err(|error| map_error(&error));
            }
            let collection = service
                .get_default_collection()
                .await
                .map_err(|error| map_error(&error))?;
            collection
                .ensure_unlocked()
                .await
                .map_err(|error| map_error(&error))?;
            collection
                .create_item("Context Fabricator", attributes, secret, true, CONTENT_TYPE)
                .await
                .map(|_| ())
                .map_err(|error| map_error(&error))
        })
    }

    fn read(&self, reference: &str) -> Result<Vec<u8>, CredentialError> {
        on_session_bus(async {
            let service = connect().await?;
            let item = find(&service, self.attributes(reference))
                .await?
                .ok_or(CredentialError::Missing)?;
            item.get_secret().await.map_err(|error| map_error(&error))
        })
    }

    fn delete(&self, reference: &str) -> Result<(), CredentialError> {
        on_session_bus(async {
            let service = connect().await?;
            let item = find(&service, self.attributes(reference))
                .await?
                .ok_or(CredentialError::Missing)?;
            item.delete().await.map_err(|error| map_error(&error))
        })
    }
}

async fn connect() -> Result<SecretService<'static>, CredentialError> {
    SecretService::connect(EncryptionType::Dh)
        .await
        .map_err(|error| map_error(&error))
}

async fn find<'a>(
    service: &'a SecretService<'_>,
    attributes: HashMap<&str, &str>,
) -> Result<Option<Item<'a>>, CredentialError> {
    let matches = service
        .search_items(attributes)
        .await
        .map_err(|error| map_error(&error))?;
    if !matches.locked.is_empty() || matches.unlocked.len() > 1 {
        return Err(CredentialError::Denied);
    }
    Ok(matches.unlocked.into_iter().next())
}

fn map_error(error: &Error) -> CredentialError {
    match error {
        Error::Locked | Error::Prompt => CredentialError::Denied,
        _ => CredentialError::Unavailable,
    }
}

fn on_session_bus<T: Send>(
    operation: impl Future<Output = Result<T, CredentialError>> + Send,
) -> Result<T, CredentialError> {
    // The credential port is synchronous. Bounding the entire D-Bus exchange
    // includes provider startup and any prompt dispatched by the service.
    let run = || {
        thread::scope(|scope| {
            thread::Builder::new()
                .name("secret-service".into())
                .spawn_scoped(scope, || {
                    let runtime = Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|_| CredentialError::Unavailable)?;
                    runtime.block_on(async {
                        tokio::time::timeout(TIMEOUT, operation)
                            .await
                            .map_err(|_| CredentialError::Unavailable)?
                    })
                })
                .map_err(|_| CredentialError::Unavailable)?
                .join()
                .map_err(|_| CredentialError::Unavailable)?
        })
    };
    if Handle::try_current()
        .is_ok_and(|handle| handle.runtime_flavor() == RuntimeFlavor::MultiThread)
    {
        tokio::task::block_in_place(run)
    } else {
        run()
    }
}
