use std::cell::RefCell;
use std::collections::HashMap;

use super::*;
use ctxfab_server::application::ports::Repository;

#[derive(Default)]
struct MemoryStore(RefCell<HashMap<String, Vec<u8>>>);

impl CredentialStore for MemoryStore {
    fn save(&self, reference: &str, secret: &[u8]) -> Result<(), CredentialError> {
        self.0
            .borrow_mut()
            .insert(reference.to_owned(), secret.to_vec());
        Ok(())
    }

    fn read(&self, reference: &str) -> Result<Vec<u8>, CredentialError> {
        self.0
            .borrow()
            .get(reference)
            .cloned()
            .ok_or(CredentialError::Missing)
    }

    fn delete(&self, reference: &str) -> Result<(), CredentialError> {
        self.0
            .borrow_mut()
            .remove(reference)
            .map(|_| ())
            .ok_or(CredentialError::Missing)
    }
}

#[test]
fn fixture_requires_an_owned_reference_and_removes_it_after_rotation()
-> Result<(), Box<dyn std::error::Error>> {
    let store = MemoryStore::default();
    let id = random_uuid()?.to_string();
    let other = "fixture:unrelated";
    store.save(other, b"unrelated")?;
    let configure = CredentialFixture::parse(OsStr::new(STORE), Some(OsStr::new(&id)))?;
    let rotate = CredentialFixture::parse(OsStr::new(ROTATE), Some(OsStr::new(&id)))?;
    assert_eq!(rotate.exercise(&store), Err(CredentialError::Missing));
    configure.exercise(&store)?;
    assert_eq!(configure.exercise(&store), Err(CredentialError::Denied));
    rotate.exercise(&store)?;
    assert_eq!(rotate.exercise(&store), Err(CredentialError::Missing));
    assert_eq!(store.read(other)?, b"unrelated");
    Ok(())
}

#[test]
fn fixture_diagnostics_report_correlated_outcomes() -> Result<(), Box<dyn std::error::Error>> {
    let id = random_uuid()?;
    let argument = id.to_string();
    let keychain = MemoryStore::default();
    let database = SqliteStore::open(std::path::Path::new(":memory:"))?;
    let store = CredentialFixture::parse(OsStr::new(STORE), Some(OsStr::new(&argument)))?;
    let rotate = CredentialFixture::parse(OsStr::new(ROTATE), Some(OsStr::new(&argument)))?;
    rotate.run(Ok(&keychain), &database)?;
    store.run(Ok(&keychain), &database)?;
    rotate.run(Err(CredentialError::Denied), &database)?;
    rotate.run(Ok(&keychain), &database)?;
    store.run(Err(CredentialError::Unavailable), &database)?;

    let diagnostics = database.diagnostics()?;
    let expected = [
        (
            "fixture_credential_failed",
            "Fixture credential storage is unavailable.",
        ),
        (
            "fixture_credential_rotated",
            "Fixture credential rotated and removed.",
        ),
        (
            "fixture_credential_failed",
            "Keychain denied fixture credential access.",
        ),
        ("fixture_credential_stored", "Fixture credential stored."),
        (
            "fixture_credential_failed",
            "Fixture credential is missing.",
        ),
    ];
    assert_eq!(diagnostics.len(), expected.len());
    for (event, (code, message)) in diagnostics.iter().zip(expected) {
        assert_eq!(event.code, code);
        assert_eq!(event.message, format!("{message} Correlation {id}."));
    }
    Ok(())
}
