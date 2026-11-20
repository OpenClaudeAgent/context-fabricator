use std::ffi::OsStr;

use ctxfab_server::adapters::outbound::sqlite::SqliteStore;
use ctxfab_server::application::Failure;
use ctxfab_server::application::credentials::{CredentialError, CredentialStore};
use ctxfab_server::runtime::entropy::random_uuid;
use uuid::Uuid;

const STORE: &str = "--fixture-store";
const ROTATE: &str = "--fixture-rotate";

#[derive(Clone, Copy)]
enum Phase {
    Store,
    Rotate,
}

pub struct CredentialFixture {
    phase: Phase,
    correlation: Uuid,
}

impl CredentialFixture {
    pub fn parse(flag: &OsStr, correlation: Option<&OsStr>) -> Result<Self, &'static str> {
        let phase = match flag.to_str() {
            Some(STORE) => Phase::Store,
            Some(ROTATE) => Phase::Rotate,
            _ => return Err("unknown fixture option"),
        };
        let correlation = correlation
            .and_then(OsStr::to_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or("invalid fixture correlation")?;
        Ok(Self { phase, correlation })
    }

    pub fn run(
        &self,
        store: Result<&dyn CredentialStore, CredentialError>,
        diagnostics: &SqliteStore,
    ) -> Result<(), Failure> {
        let result = store.and_then(|store| self.exercise(store));
        let (code, message) = match (&result, self.phase) {
            (Ok(()), Phase::Store) => ("fixture_credential_stored", "Fixture credential stored."),
            (Ok(()), Phase::Rotate) => (
                "fixture_credential_rotated",
                "Fixture credential rotated and removed.",
            ),
            (Err(error), _) => (
                "fixture_credential_failed",
                match error {
                    CredentialError::Missing => "Fixture credential is missing.",
                    CredentialError::Denied => "Keychain denied fixture credential access.",
                    CredentialError::Unavailable => "Fixture credential storage is unavailable.",
                },
            ),
        };
        diagnostics.record(
            code,
            &format!("{message} Correlation {}.", self.correlation),
        )
    }

    fn exercise(&self, store: &dyn CredentialStore) -> Result<(), CredentialError> {
        let reference = format!("fixture:{}", self.correlation);
        match self.phase {
            Phase::Store => {
                match store.read(&reference) {
                    Err(CredentialError::Missing) => {}
                    Err(error) => return Err(error),
                    Ok(_) => return Err(CredentialError::Denied),
                }
                let secret = random_uuid().map_err(|_| CredentialError::Unavailable)?;
                if let Err(error) = store_and_check(store, &reference, secret.as_bytes()) {
                    let _ = store.delete(&reference);
                    return Err(error);
                }
            }
            Phase::Rotate => {
                let old = store.read(&reference)?;
                let replacement = random_uuid().map_err(|_| CredentialError::Unavailable)?;
                let rotate = if old == replacement.as_bytes() {
                    Err(CredentialError::Unavailable)
                } else {
                    store_and_check(store, &reference, replacement.as_bytes())
                };
                let remove = store.delete(&reference);
                rotate?;
                remove?;
                if store.read(&reference) != Err(CredentialError::Missing) {
                    return Err(CredentialError::Unavailable);
                }
            }
        }
        Ok(())
    }
}

fn store_and_check(
    store: &dyn CredentialStore,
    reference: &str,
    value: &[u8],
) -> Result<(), CredentialError> {
    store.save(reference, value)?;
    if store.read(reference)? != value {
        return Err(CredentialError::Unavailable);
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/credential_fixture.rs"]
mod tests;
