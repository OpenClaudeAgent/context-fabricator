//! Verification for the runtime sealed inside the desktop application.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
};

use super::{
    BUNDLE, BUNDLE_ID, MAX_ARCHIVE, WORKER, bundle_measure, err, inspect_layout,
    verify_code_signature,
};

const DESKTOP_REQUIREMENT: &str =
    r#"identifier "org.contextfabricator.desktop" or identifier "org.contextfabricator.qt""#;

pub struct BundledRuntime {
    pub binary: PathBuf,
    pub manifest_sha256: String,
}

impl BundledRuntime {
    /// Read the build number from the signed runtime bundle.
    #[must_use]
    pub fn build_version(&self) -> Option<String> {
        let info = self.binary.parent()?.parent()?.join("Info.plist");
        let output = Command::new("/usr/bin/plutil")
            .args(["-extract", "CFBundleVersion", "raw", "-o", "-"])
            .arg(info)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let version = std::str::from_utf8(&output.stdout).ok()?.trim();
        (!version.is_empty()
            && version.len() <= 24
            && version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())))
        .then(|| version.to_owned())
    }
}

/// Resolve the fixed nested runtime from the backend's own executable location.
///
/// # Errors
/// Rejects an invalid bundle location, layout, or enclosing code signature.
pub fn verify(helper: &Path) -> Result<Option<BundledRuntime>, String> {
    let Some(contents) = helper.parent().and_then(Path::parent) else {
        return Ok(None);
    };
    let Some(desktop) = contents
        .parent()
        .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
    else {
        return Ok(None);
    };
    if helper != contents.join("MacOS/ctxfab-server") || contents != desktop.join("Contents") {
        return Ok(None);
    }

    verify_code_signature(desktop, DESKTOP_REQUIREMENT, &["--deep"])?;
    let helpers = contents.join("Helpers");
    let bundle = helpers.join(BUNDLE);
    match fs::symlink_metadata(&bundle) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(err(error)),
        Ok(_) => {}
    }
    if !fs::symlink_metadata(&helpers)
        .map_err(err)?
        .file_type()
        .is_dir()
    {
        return Err("Invalid desktop helper directory".into());
    }

    inspect_layout(&bundle)?;
    verify_code_signature(&bundle, &format!("identifier \"{BUNDLE_ID}\""), &[])?;
    let (_, manifest_sha256) = bundle_measure(&bundle, MAX_ARCHIVE)?;
    Ok(Some(BundledRuntime {
        binary: bundle.join(WORKER),
        manifest_sha256,
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::adapters::outbound::managed_runtime::run;

    fn signed_fixture(root: &Path, identifier: &str) -> PathBuf {
        let bundle = root.join(format!("{identifier}.app"));
        let contents = bundle.join("Contents");
        let helper = contents.join("MacOS/ctxfab-server");
        fs::create_dir_all(helper.parent().unwrap()).unwrap();
        fs::create_dir(contents.join("Resources")).unwrap();
        fs::write(contents.join("Resources/marker.txt"), "sealed").unwrap();
        fs::copy("/usr/bin/true", &helper).unwrap();
        fs::write(
            contents.join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{identifier}</string>
<key>CFBundleExecutable</key><string>ctxfab-server</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>"#
            ),
        )
        .unwrap();
        run("/usr/bin/codesign", &["--force", "--sign", "-"], &[&bundle]).unwrap();
        helper
    }

    #[test]
    fn accepts_both_signed_desktop_bundles() {
        let directory = tempfile::tempdir().unwrap();
        for identifier in ["org.contextfabricator.desktop", "org.contextfabricator.qt"] {
            let helper = signed_fixture(directory.path(), identifier);
            assert!(verify(&helper).unwrap().is_none(), "{identifier}");
        }
    }

    #[test]
    fn rejects_unrecognized_identity_and_tampered_qt_bundle() {
        let directory = tempfile::tempdir().unwrap();
        let other = signed_fixture(directory.path(), "org.contextfabricator.qt.other");
        assert!(verify(&other).is_err());

        let qt = signed_fixture(directory.path(), "org.contextfabricator.qt");
        let contents = qt.parent().unwrap().parent().unwrap();
        fs::write(contents.join("Resources/marker.txt"), "changed").unwrap();
        assert!(verify(&qt).is_err());
    }
}
