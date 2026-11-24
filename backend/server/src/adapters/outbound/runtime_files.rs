//! Bounded reads and digests for installed runtime assets.

use rustix::fs::OFlags;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Read as _;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

pub(super) fn open_regular(path: &Path, max_bytes: u64) -> Result<File, String> {
    let before = fs::symlink_metadata(path).map_err(err)?;
    if !before.file_type().is_file() || before.len() > max_bytes {
        return Err("Expected a bounded regular file".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags((OFlags::NOFOLLOW | OFlags::NONBLOCK).bits().cast_signed());
    let file = options.open(path).map_err(err)?;
    let after = file.metadata().map_err(err)?;
    if !after.is_file() || after.len() > max_bytes {
        return Err("Expected a bounded regular file".into());
    }
    if before.dev() != after.dev() || before.ino() != after.ino() {
        return Err("File changed during open".into());
    }
    Ok(file)
}

pub(super) fn hash_file(path: &Path, expected: u64) -> Result<Vec<u8>, String> {
    let mut file = open_regular(path, expected)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 16384];
    let mut total = 0u64;
    loop {
        let count = file.read(&mut buffer).map_err(err)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or("File size overflow")?;
        if total > expected {
            return Err("Runtime file exceeds size limit".into());
        }
        hash.update(&buffer[..count]);
    }
    if total != expected {
        return Err("Runtime file size changed".into());
    }
    Ok(hash.finalize().to_vec())
}
pub(super) fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 15)]));
    }
    output
}

pub(super) fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
