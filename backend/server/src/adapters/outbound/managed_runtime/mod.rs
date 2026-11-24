//! Verification and staging of a release-pinned macOS runtime package.
//!
//! The pin must come from trusted release configuration, never the downloaded archive.

#[cfg(target_os = "macos")]
pub mod bundled;
pub mod network;

use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

use super::runtime_files::{err, hash_file, hex_digest, open_regular};

const BUNDLE: &str = "Context Fabricator Runtime.app";
const BUNDLE_ID: &str = "org.contextfabricator.runtime.llama";
const WORKER: &str = "Contents/MacOS/llama-server";
const MAX_ARCHIVE: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ManagedRuntimePin {
    pub archive_bytes: u64,
    pub archive_expanded_bytes: u64,
    pub bundle_bytes: u64,
    pub bundle_manifest_sha256: String,
    pub archive_sha256: String,
    pub apple_team_id: String,
}

/// Copy and authenticate the exact input bytes before extraction. Returns the canonical
/// path to a worker in a newly published, private, read-only directory.
///
/// # Errors
/// Returns an error if the archive, signature, or filesystem validation fails.
pub fn verify_and_stage(
    input: &Path,
    app_owned_root: &Path,
    pin: &ManagedRuntimePin,
) -> Result<PathBuf, String> {
    fs::create_dir_all(app_owned_root).map_err(err)?;
    let _staging_lock = lock_staging(app_owned_root)?;
    verify_and_stage_with(input, app_owned_root, pin, &SystemVerifier, true)
}

/// Reverify retained archive, signatures and exact bundle bytes before reuse.
///
/// # Errors
/// Returns an error if a candidate exists but cannot be verified.
pub fn reopen(app_owned_root: &Path, pin: &ManagedRuntimePin) -> Result<Option<PathBuf>, String> {
    if !app_owned_root.exists() {
        return Ok(None);
    }
    {
        let _staging_lock = lock_staging(app_owned_root)?;
        recover_staging(app_owned_root)?;
    }
    let mut candidates = Vec::new();
    for entry in fs::read_dir(app_owned_root).map_err(err)? {
        let entry = entry.map_err(err)?;
        if entry.file_name().to_string_lossy().starts_with("runtime-")
            && entry.file_type().map_err(err)?.is_dir()
        {
            candidates.push(entry.path().join(BUNDLE).join(WORKER));
        }
    }
    candidates.sort();
    if candidates.is_empty() {
        return Ok(None);
    }
    let mut last_error = String::new();
    for candidate in candidates {
        match verify_worker(&candidate, pin) {
            Ok(path) => return Ok(Some(path)),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// Revalidate a specific worker against the release manifest in place.
///
/// # Errors
/// Returns an error if worker, manifest, signature, or filesystem validation fails.
pub fn verify_worker(worker: &Path, pin: &ManagedRuntimePin) -> Result<PathBuf, String> {
    let worker = fs::canonicalize(worker).map_err(err)?;
    let bundle = worker
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or("Invalid worker path")?;
    if bundle.file_name().and_then(|s| s.to_str()) != Some(BUNDLE) || worker != bundle.join(WORKER)
    {
        return Err("Invalid worker path".into());
    }
    let published = bundle.parent().ok_or("Invalid runtime path")?;
    if !published
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.starts_with("runtime-"))
    {
        return Err("Invalid runtime directory".into());
    }
    validate_pin(pin)?;
    let libraries = inspect_layout(bundle)?;
    verify_bundle_manifest(bundle, pin)?;
    SystemVerifier.verify(bundle, &libraries, &pin.apple_team_id)?;
    Ok(worker)
}

// Canonical manifest records every entry as type, hex-encoded relative path,
// and, for files, size plus SHA-256; symlinks contain hex-encoded link bytes.
fn verify_bundle_manifest(bundle: &Path, pin: &ManagedRuntimePin) -> Result<(), String> {
    let (total, digest) = bundle_measure(bundle, pin.bundle_bytes)?;
    if total != pin.bundle_bytes || digest != pin.bundle_manifest_sha256.to_ascii_lowercase() {
        return Err("Runtime bundle does not match release manifest".into());
    }
    Ok(())
}
fn bundle_measure(bundle: &Path, maximum: u64) -> Result<(u64, String), String> {
    use std::os::unix::ffi::OsStrExt;
    fn collect(
        root: &Path,
        dir: &Path,
        entries: &mut Vec<PathBuf>,
        depth: usize,
    ) -> Result<(), String> {
        if depth > 64 {
            return Err("Runtime bundle directory depth exceeds limit".into());
        }
        for entry in fs::read_dir(dir).map_err(err)? {
            let entry = entry.map_err(err)?;
            let path = entry.path();
            if entries.len() >= 10_000 {
                return Err("Runtime bundle entry count exceeds limit".into());
            }
            entries.push(path.strip_prefix(root).map_err(err)?.to_path_buf());
            if entry.file_type().map_err(err)?.is_dir() {
                collect(root, &path, entries, depth + 1)?;
            }
        }
        Ok(())
    }
    let mut entries = Vec::new();
    collect(bundle, bundle, &mut entries, 0)?;
    entries.sort_by(|a, b| a.as_os_str().as_bytes().cmp(b.as_os_str().as_bytes()));
    let mut digest = Sha256::new();
    let mut total = 0u64;
    for relative in entries {
        let path = bundle.join(&relative);
        let meta = fs::symlink_metadata(&path).map_err(err)?;
        let name = hex_digest(relative.as_os_str().as_bytes());
        let record = if meta.is_dir() {
            format!("D {name}\n")
        } else if meta.file_type().is_symlink() {
            let target = fs::read_link(&path).map_err(err)?;
            let bytes = target.as_os_str().as_bytes();
            total = total
                .checked_add(bytes.len() as u64)
                .ok_or("Bundle size overflow")?;
            format!("L {name} {}\n", hex_digest(bytes))
        } else if meta.is_file() {
            total = total
                .checked_add(meta.len())
                .ok_or("Bundle size overflow")?;
            if total > maximum {
                return Err("Runtime bundle exceeds pinned size".into());
            }
            format!(
                "F {name} {} {}\n",
                meta.len(),
                hex_digest(&hash_file(&path, meta.len())?)
            )
        } else {
            return Err("Unsafe runtime bundle entry".into());
        };
        if total > maximum {
            return Err("Runtime bundle exceeds pinned size".into());
        }
        digest.update(record.as_bytes());
    }
    Ok((total, hex_digest(&digest.finalize())))
}

fn lock_staging(root: &Path) -> Result<File, String> {
    let directory = File::open(root).map_err(err)?;
    rustix::fs::flock(&directory, rustix::fs::FlockOperation::LockExclusive).map_err(err)?;
    Ok(directory)
}
fn recover_staging(root: &Path) -> Result<(), String> {
    for entry in fs::read_dir(root).map_err(err)? {
        let entry = entry.map_err(err)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let directory = name
            .strip_prefix(".staging-")
            .or_else(|| name.strip_prefix(".verification-"));
        let download = name
            .strip_prefix(".runtime-download-")
            .and_then(|s| s.strip_suffix(".zip"));
        let valid = |suffix: &str| {
            suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        };
        if directory.is_some_and(valid) && entry.file_type().map_err(err)?.is_dir() {
            remove_readonly_tree(&entry.path())?;
        } else if download.is_some_and(valid) && entry.file_type().map_err(err)?.is_file() {
            fs::remove_file(entry.path()).map_err(err)?;
        }
    }
    Ok(())
}

fn remove_readonly_tree(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    fn unlock(path: &Path) -> Result<(), String> {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(err)?;
        for entry in fs::read_dir(path).map_err(err)? {
            let entry = entry.map_err(err)?;
            if entry.file_type().map_err(err)?.is_dir() {
                unlock(&entry.path())?;
            }
        }
        Ok(())
    }
    #[cfg(unix)]
    unlock(path)?;
    fs::remove_dir_all(path).map_err(err)
}

trait Verifier {
    fn expanded_size(&self, archive: &Path) -> Result<u64, String>;
    fn extract(&self, archive: &Path, destination: &Path) -> Result<(), String>;
    fn verify(&self, bundle: &Path, libraries: &[PathBuf], team: &str) -> Result<(), String>;
}

struct SystemVerifier;
impl Verifier for SystemVerifier {
    fn expanded_size(&self, archive: &Path) -> Result<u64, String> {
        let output = Command::new("/usr/bin/unzip")
            .env_clear()
            .env("LC_ALL", "C")
            .args(["-Z", "-t"])
            .arg(archive)
            .output()
            .map_err(err)?;
        if !output.status.success() || output.stdout.len() > 1024 {
            return Err("Unable to inspect runtime archive".into());
        }
        let text = std::str::from_utf8(&output.stdout).map_err(err)?;
        let words: Vec<_> = text.split_ascii_whitespace().collect();
        if words.len() < 5
            || !matches!(words[1], "file," | "files,")
            || words[3] != "bytes"
            || words[4] != "uncompressed,"
        {
            return Err("Invalid runtime archive listing".into());
        }
        let count: u64 = words[0].parse().map_err(err)?;
        if count == 0 || count > 10_000 {
            return Err("Invalid runtime archive entry count".into());
        }
        words[2].parse().map_err(err)
    }
    fn extract(&self, archive: &Path, destination: &Path) -> Result<(), String> {
        run("/usr/bin/ditto", &["-x", "-k"], &[archive, destination])
    }
    fn verify(&self, bundle: &Path, libraries: &[PathBuf], team: &str) -> Result<(), String> {
        let requirement = format!(
            "anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"{team}\""
        );
        for library in libraries {
            verify_code_signature(library, &requirement, &[])?;
        }
        let bundle_requirement = format!("{requirement} and identifier \"{BUNDLE_ID}\"");
        verify_code_signature(bundle, &bundle_requirement, &["--deep"])
    }
}

fn verify_code_signature(path: &Path, requirement: &str, options: &[&str]) -> Result<(), String> {
    let inline_requirement = format!("-R={requirement}");
    let mut args = vec!["--verify", "--strict", inline_requirement.as_str()];
    args.extend_from_slice(options);
    run("/usr/bin/codesign", &args, &[path])
}

fn run(program: &str, args: &[&str], paths: &[&Path]) -> Result<(), String> {
    let output = Command::new(program)
        .args(args)
        .args(paths)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn validate_pin(pin: &ManagedRuntimePin) -> Result<(), String> {
    if pin.archive_bytes == 0
        || pin.archive_bytes > MAX_ARCHIVE
        || pin.archive_expanded_bytes == 0
        || pin.archive_expanded_bytes > 4 * 1024 * 1024 * 1024
        || pin.bundle_bytes == 0
        || pin.bundle_bytes > pin.archive_expanded_bytes
        || [
            pin.archive_sha256.as_str(),
            pin.bundle_manifest_sha256.as_str(),
        ]
        .iter()
        .any(|value| value.len() != 64 || !value.bytes().all(|c| c.is_ascii_hexdigit()))
        || pin.apple_team_id.len() != 10
        || !pin
            .apple_team_id
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return Err("Invalid trusted runtime package pin".into());
    }
    Ok(())
}

fn verify_and_stage_with(
    input: &Path,
    root: &Path,
    pin: &ManagedRuntimePin,
    verifier: &dyn Verifier,
    publish: bool,
) -> Result<PathBuf, String> {
    validate_pin(pin)?;
    fs::create_dir_all(root).map_err(err)?;
    let root = fs::canonicalize(root).map_err(err)?;
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(err)?;
    let id = hex_digest(&random);
    let staging = root.join(format!(".staging-{id}"));
    fs::create_dir(&staging).map_err(err)?;
    #[cfg(unix)]
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o700)).map_err(err)?;
    let result = (|| {
        let archive = staging.join("package.zip");
        let mut source = open_regular(input, pin.archive_bytes)?;
        if source.metadata().map_err(err)?.len() != pin.archive_bytes {
            return Err("Runtime archive does not match pinned size".into());
        }
        let mut destination = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&archive)
            .map_err(err)?;
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut buf = [0u8; 16384];
        loop {
            let count = source.read(&mut buf).map_err(err)?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or("Archive size overflow")?;
            if total > pin.archive_bytes {
                return Err("Runtime archive exceeds pinned size".into());
            }
            hash.update(&buf[..count]);
            destination.write_all(&buf[..count]).map_err(err)?;
        }
        destination.sync_all().map_err(err)?;
        if total != pin.archive_bytes
            || hex_digest(&hash.finalize()) != pin.archive_sha256.to_ascii_lowercase()
        {
            return Err("Runtime archive does not match the release pin".into());
        }
        drop(destination);
        if verifier.expanded_size(&archive)? != pin.archive_expanded_bytes {
            return Err("Runtime archive expanded size does not match release pin".into());
        }
        let extracted = staging.join("extracted");
        fs::create_dir(&extracted).map_err(err)?;
        verifier.extract(&archive, &extracted)?;
        let entries = fs::read_dir(&extracted)
            .map_err(err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(err)?;
        if entries.len() != 1 || entries[0].file_name() != BUNDLE {
            return Err("Unexpected runtime archive layout".into());
        }
        let bundle = extracted.join(BUNDLE);
        let libs = inspect_layout(&bundle)?;
        verify_bundle_manifest(&bundle, pin)?;
        verifier.verify(&bundle, &libs, &pin.apple_team_id)?;
        fs::rename(&archive, extracted.join("package.zip")).map_err(err)?;
        make_readonly(&bundle)?;
        let prefix = if publish { "runtime" } else { ".verification" };
        let published = root.join(format!("{prefix}-{id}"));
        fs::rename(&extracted, &published).map_err(err)?;
        let worker = fs::canonicalize(published.join(BUNDLE).join(WORKER)).map_err(err)?;
        if !worker.starts_with(&published) {
            return Err("Runtime worker escaped its package".into());
        }
        Ok(worker)
    })();
    let _ = remove_readonly_tree(&staging);
    result
}

#[allow(clippy::items_after_statements)]
fn inspect_layout(bundle: &Path) -> Result<Vec<PathBuf>, String> {
    fn directory(path: &Path) -> Result<(), String> {
        if !fs::symlink_metadata(path)
            .map_err(err)?
            .file_type()
            .is_dir()
        {
            return Err(format!("Expected directory: {}", path.display()));
        }
        Ok(())
    }
    fn regular(path: &Path) -> Result<(), String> {
        if !fs::symlink_metadata(path)
            .map_err(err)?
            .file_type()
            .is_file()
        {
            return Err(format!("Expected regular file: {}", path.display()));
        }
        Ok(())
    }
    directory(bundle)?;
    for path in [
        "Contents",
        "Contents/MacOS",
        "Contents/Frameworks",
        "Contents/Resources",
    ] {
        directory(&bundle.join(path))?;
    }
    for path in [
        WORKER,
        "Contents/Info.plist",
        "Contents/Resources/LICENSE-llama.cpp",
    ] {
        regular(&bundle.join(path))?;
    }
    let frameworks = bundle.join("Contents/Frameworks");
    let mut libraries = Vec::new();
    for entry in fs::read_dir(&frameworks).map_err(err)? {
        let entry = entry.map_err(err)?;
        let name = entry.file_name();
        let name = name.to_str().ok_or("Invalid library name")?;
        if !library_name(name) {
            return Err("Unexpected runtime framework".into());
        }
        let meta = fs::symlink_metadata(entry.path()).map_err(err)?;
        if meta.file_type().is_symlink() {
            let target = fs::read_link(entry.path()).map_err(err)?;
            if target.components().count() != 1 || !target.to_str().is_some_and(library_name) {
                return Err("Unsafe runtime library link".into());
            }
            let resolved = fs::canonicalize(frameworks.join(target)).map_err(err)?;
            if resolved.parent() != Some(fs::canonicalize(&frameworks).map_err(err)?.as_path())
                || !resolved
                    .file_name()
                    .and_then(|s| s.to_str())
                    .is_some_and(library_name)
            {
                return Err("Unsafe runtime library target".into());
            }
            regular(&resolved)?;
        } else if meta.is_file() {
            libraries.push(entry.path());
        } else {
            return Err("Invalid runtime framework entry".into());
        }
    }
    if libraries.is_empty() {
        return Err("Runtime contains no native libraries".into());
    }
    // No untrusted symlinks or special files elsewhere in the signed bundle.
    fn walk(path: &Path, frameworks: &Path) -> Result<(), String> {
        for entry in fs::read_dir(path).map_err(err)? {
            let entry = entry.map_err(err)?;
            let meta = fs::symlink_metadata(entry.path()).map_err(err)?;
            if meta.is_dir() {
                walk(&entry.path(), frameworks)?;
            } else if !(meta.is_file() || meta.file_type().is_symlink() && path == frameworks) {
                return Err("Unsafe runtime bundle entry".into());
            }
        }
        Ok(())
    }
    walk(bundle, &frameworks)?;
    Ok(libraries)
}
fn library_name(name: &str) -> bool {
    name.strip_prefix("lib")
        .and_then(|n| n.strip_suffix(".dylib"))
        .is_some_and(|n| {
            !n.is_empty()
                && n.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        })
}
fn make_readonly(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        for entry in fs::read_dir(path).map_err(err)? {
            let entry = entry.map_err(err)?;
            let meta = fs::symlink_metadata(entry.path()).map_err(err)?;
            if meta.is_dir() {
                make_readonly(&entry.path())?;
            } else if meta.is_file() {
                let mode = if meta.permissions().mode() & 0o111 != 0 {
                    0o555
                } else {
                    0o444
                };
                fs::set_permissions(entry.path(), fs::Permissions::from_mode(mode)).map_err(err)?;
            }
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o555)).map_err(err)?;
    }
    Ok(())
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn codesign_evaluates_the_inline_requirement() {
        let executable = Path::new("/usr/bin/true");
        verify_code_signature(executable, "identifier \"com.apple.true\"", &[]).unwrap();
        assert!(
            verify_code_signature(executable, "identifier \"org.contextfabricator.test\"", &[])
                .is_err()
        );
    }

    struct FixtureVerifier {
        reject: bool,
    }
    impl Verifier for FixtureVerifier {
        fn expanded_size(&self, _: &Path) -> Result<u64, String> {
            Ok(28)
        }
        fn extract(&self, _: &Path, destination: &Path) -> Result<(), String> {
            let bundle = destination.join(BUNDLE);
            for dir in [
                "Contents/MacOS",
                "Contents/Frameworks",
                "Contents/Resources",
            ] {
                fs::create_dir_all(bundle.join(dir)).map_err(err)?;
            }
            for file in [
                WORKER,
                "Contents/Info.plist",
                "Contents/Resources/LICENSE-llama.cpp",
                "Contents/Frameworks/libtest.dylib",
            ] {
                fs::write(bundle.join(file), b"fixture").map_err(err)?;
            }
            Ok(())
        }
        fn verify(&self, _: &Path, libraries: &[PathBuf], team: &str) -> Result<(), String> {
            if self.reject || libraries.len() != 1 || team != "ABCDEFGHIJ" {
                Err("signature rejected".into())
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn pinned_bytes_and_verifier_gate_publication() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("input.zip");
        fs::write(&archive, b"fixture archive").unwrap();
        let root = temp.path().join("owned");
        let pin = ManagedRuntimePin {
            archive_bytes: 15,
            archive_expanded_bytes: 28,
            bundle_bytes: 28,
            bundle_manifest_sha256:
                "42ebbfad10d18800372bc904d6a8e4057593db1f2d8fb4d95bf7eb17e618956c".into(),
            archive_sha256: hex_digest(&Sha256::digest(b"fixture archive")),
            apple_team_id: "ABCDEFGHIJ".into(),
        };
        let rejected = FixtureVerifier { reject: true };
        assert!(verify_and_stage_with(&archive, &root, &pin, &rejected, true).is_err());
        let good = FixtureVerifier { reject: false };
        let worker = verify_and_stage_with(&archive, &root, &pin, &good, true).unwrap();
        assert!(worker.is_file());
        fs::write(&archive, b"tampered bytes!").unwrap();
        assert!(verify_and_stage_with(&archive, &root, &pin, &good, true).is_err());
    }

    #[test]
    fn expanded_size_and_member_manifest_detect_tampering() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("input.zip");
        fs::write(&archive, b"fixture archive").unwrap();
        let root = temp.path().join("owned");
        let mut pin = ManagedRuntimePin {
            archive_bytes: 15,
            archive_expanded_bytes: 29,
            bundle_bytes: 28,
            bundle_manifest_sha256:
                "42ebbfad10d18800372bc904d6a8e4057593db1f2d8fb4d95bf7eb17e618956c".into(),
            archive_sha256: hex_digest(&Sha256::digest(b"fixture archive")),
            apple_team_id: "ABCDEFGHIJ".into(),
        };
        let verifier = FixtureVerifier { reject: false };
        assert!(verify_and_stage_with(&archive, &root, &pin, &verifier, true).is_err());
        pin.archive_expanded_bytes = 28;
        let worker = verify_and_stage_with(&archive, &root, &pin, &verifier, true).unwrap();
        let bundle = worker.parent().unwrap().parent().unwrap().parent().unwrap();
        assert!(verify_bundle_manifest(bundle, &pin).is_ok());
        fs::set_permissions(&worker, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(&worker, b"altered").unwrap();
        assert!(verify_bundle_manifest(bundle, &pin).is_err());
        assert!(!fs::read_dir(&root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".verification-")
        }));
    }

    #[test]
    fn startup_removes_abandoned_private_staging_and_leaves_other_directories() {
        let root = tempfile::tempdir().unwrap();
        let pin = ManagedRuntimePin {
            archive_bytes: 15,
            archive_expanded_bytes: 28,
            bundle_bytes: 28,
            bundle_manifest_sha256:
                "42ebbfad10d18800372bc904d6a8e4057593db1f2d8fb4d95bf7eb17e618956c".into(),
            archive_sha256: hex_digest(&Sha256::digest(b"fixture archive")),
            apple_team_id: "ABCDEFGHIJ".into(),
        };
        let staging = root
            .path()
            .join(".staging-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let verification = root
            .path()
            .join(".verification-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let download = root
            .path()
            .join(".runtime-download-cccccccccccccccccccccccccccccccc.zip");
        let unrelated = root.path().join(".verification-owned-by-user");
        for path in [&staging, &verification, &unrelated] {
            fs::create_dir(path).unwrap();
            fs::write(path.join("package.zip"), b"fixture archive").unwrap();
            make_readonly(path).unwrap();
        }
        fs::write(&download, b"interrupted runtime download").unwrap();
        assert!(reopen(root.path(), &pin).unwrap().is_none());
        assert!(!staging.exists());
        assert!(!verification.exists());
        assert!(!download.exists());
        assert!(unrelated.join("package.zip").is_file());
        remove_readonly_tree(&unrelated).unwrap();
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn real_adhoc_package_is_rejected_when_fixture_is_supplied() {
        let Some(path) = std::env::var_os("CTXFAB_ADHOC_RUNTIME_FIXTURE") else {
            return;
        };
        let path = PathBuf::from(path);
        let bytes = fs::read(&path).unwrap();
        let bundle = path.parent().unwrap().join(BUNDLE);
        let (bundle_bytes, bundle_manifest_sha256) =
            bundle_measure(&bundle, 4 * 1024 * 1024 * 1024).unwrap();
        let archive_expanded_bytes = SystemVerifier.expanded_size(&path).unwrap();
        if let Ok(expected) = std::env::var("CTXFAB_ADHOC_EXPECTED_MANIFEST") {
            assert_eq!(bundle_manifest_sha256, expected);
        }
        let pin = ManagedRuntimePin {
            archive_bytes: bytes.len() as u64,
            archive_expanded_bytes,
            bundle_bytes,
            bundle_manifest_sha256,
            archive_sha256: hex_digest(&Sha256::digest(&bytes)),
            apple_team_id: "ABCDEFGHIJ".into(),
        };
        let temp = tempfile::tempdir().unwrap();
        let error = verify_and_stage(&path, temp.path(), &pin).unwrap_err();
        assert!(error.contains("/usr/bin/codesign failed"), "{error}");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}
