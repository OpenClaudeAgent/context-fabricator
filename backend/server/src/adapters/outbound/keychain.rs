use security_framework::base::Error;
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

use crate::application::credentials::{CredentialError, CredentialStore};

pub struct Keychain {
    service: &'static str,
}

const SERVICE: &str = "org.contextfabricator.providers";

impl Keychain {
    /// Configures the helper process to return an error when Keychain requires interaction.
    ///
    /// # Errors
    /// Returns a typed failure if Keychain cannot be configured.
    pub fn noninteractive() -> Result<Self, CredentialError> {
        // SAFETY: Security.framework takes a Boolean by value and changes only this process.
        let status =
            unsafe { security_framework_sys::keychain::SecKeychainSetUserInteractionAllowed(0) };
        if status == 0 {
            Ok(Self { service: SERVICE })
        } else {
            Err(map_error(Error::from_code(status)))
        }
    }

    #[cfg(debug_assertions)]
    #[must_use]
    pub fn github_fixture(mut self) -> Self {
        self.service = "org.contextfabricator.providers.github-fixture";
        self
    }
}

impl CredentialStore for Keychain {
    fn save(&self, reference: &str, secret: &[u8]) -> Result<(), CredentialError> {
        set_generic_password(self.service, reference, secret).map_err(map_error)
    }

    fn read(&self, reference: &str) -> Result<Vec<u8>, CredentialError> {
        get_generic_password(self.service, reference).map_err(map_error)
    }

    fn delete(&self, reference: &str) -> Result<(), CredentialError> {
        delete_generic_password(self.service, reference).map_err(map_error)
    }
}

fn map_error(error: Error) -> CredentialError {
    match error.code() {
        -25300 => CredentialError::Missing,
        -25293 | -25308 | -128 => CredentialError::Denied,
        _ => CredentialError::Unavailable,
    }
}
