use std::collections::HashMap;
use std::sync::Mutex;

use ctxfab_server::application::credentials::{CredentialError, CredentialStore};

#[derive(Default)]
pub struct Credentials(Mutex<HashMap<String, Vec<u8>>>);

impl CredentialStore for Credentials {
    fn save(&self, reference: &str, secret: &[u8]) -> Result<(), CredentialError> {
        self.0
            .lock()
            .map_err(|_| CredentialError::Unavailable)?
            .insert(reference.to_owned(), secret.to_vec());
        Ok(())
    }

    fn read(&self, reference: &str) -> Result<Vec<u8>, CredentialError> {
        self.0
            .lock()
            .map_err(|_| CredentialError::Unavailable)?
            .get(reference)
            .cloned()
            .ok_or(CredentialError::Missing)
    }

    fn delete(&self, reference: &str) -> Result<(), CredentialError> {
        self.0
            .lock()
            .map_err(|_| CredentialError::Unavailable)?
            .remove(reference)
            .map(|_| ())
            .ok_or(CredentialError::Missing)
    }
}
