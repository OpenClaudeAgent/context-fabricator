//! Authenticate the CPU runtime shipped beside the Linux helper.

use std::fs;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::runtime_files::{err, hash_file, hex_digest, open_regular};

const MAX_WORKER: u64 = 512 * 1024 * 1024;

const MANIFEST_LIMIT: u64 = 4096;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    bytes: u64,
    sha256: String,
    build: String,
}

pub struct BundledRuntime {
    pub binary: PathBuf,
    pub manifest_sha256: String,
    pub build: String,
}

/// Verify the packaged worker against the manifest pin embedded in the helper.
///
/// # Errors
/// Rejects missing, modified or redirected package files when a pin is present.
pub fn verify(helper: &Path) -> Result<Option<BundledRuntime>, String> {
    option_env!("CTXFAB_LINUX_RUNTIME_MANIFEST_SHA256")
        .map(|pin| verify_pinned(helper, pin))
        .transpose()
}

fn verify_pinned(helper: &Path, pin: &str) -> Result<BundledRuntime, String> {
    let parent = helper.parent().ok_or("Invalid Linux helper path")?;
    let directory = parent.join("runtime");
    let metadata = fs::symlink_metadata(&directory).map_err(err)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o022 != 0 {
        return Err("Invalid Linux runtime directory".into());
    }

    let mut raw = Vec::new();
    open_regular(&directory.join("manifest.json"), MANIFEST_LIMIT)?
        .take(MANIFEST_LIMIT + 1)
        .read_to_end(&mut raw)
        .map_err(err)?;
    if hex_digest(&Sha256::digest(&raw)) != pin {
        return Err("Linux runtime manifest does not match the embedded pin".into());
    }
    let manifest: Manifest = serde_json::from_slice(&raw).map_err(err)?;
    if manifest.bytes == 0
        || manifest.bytes > MAX_WORKER
        || manifest.build.is_empty()
        || manifest.build.len() > 24
        || !manifest.build.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("Invalid Linux runtime manifest".into());
    }

    let binary = directory.join("llama-server");
    let mode = fs::symlink_metadata(&binary)
        .map_err(err)?
        .permissions()
        .mode();
    if mode & 0o111 == 0
        || mode & 0o7022 != 0
        || hex_digest(&hash_file(&binary, manifest.bytes)?) != manifest.sha256
    {
        return Err("Linux runtime worker does not match the embedded manifest".into());
    }
    Ok(BundledRuntime {
        binary,
        manifest_sha256: pin.into(),
        build: manifest.build,
    })
}
