//! Platform credentials are addressed by an opaque provider-owned reference.

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialError {
    #[error("The credential does not exist.")]
    Missing,
    #[error("The credential store denied access.")]
    Denied,
    #[error("The credential store is unavailable.")]
    Unavailable,
}

pub trait CredentialStore {
    /// # Errors
    /// Returns a typed failure when the credential cannot be written.
    fn save(&self, reference: &str, secret: &[u8]) -> Result<(), CredentialError>;
    /// # Errors
    /// Returns a typed failure when the credential cannot be read.
    fn read(&self, reference: &str) -> Result<Vec<u8>, CredentialError>;
    /// # Errors
    /// Returns a typed failure when the credential cannot be deleted.
    fn delete(&self, reference: &str) -> Result<(), CredentialError>;
}
