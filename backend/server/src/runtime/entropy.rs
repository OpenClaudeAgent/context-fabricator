use std::io;

use uuid::{Builder, Uuid};

/// Creates a UUID from the operating system's secure random source.
///
/// # Errors
/// Returns an I/O error rather than issuing an identifier after entropy failure.
pub fn random_uuid() -> io::Result<Uuid> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| io::Error::other("secure random source unavailable"))?;
    Ok(Builder::from_random_bytes(bytes).into_uuid())
}
