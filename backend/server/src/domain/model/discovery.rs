//! Validated identities and display metadata for publicly discoverable weights.

use std::fmt::Write as _;

use serde::Serialize;

use super::{Artifact, ArtifactKind};
use sha2::{Digest, Sha256};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    pub repository: String,
    pub revision: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<ArtifactKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Variant {
    pub file: String,
    pub quantization: &'static str,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DiscoveryError {
    #[error("Enter a search of at most 80 characters or select a catalog result.")]
    Invalid,
    #[error("This repository or file is outside the supported Hugging Face catalog.")]
    Unsupported,
    #[error("The selected file changed. Review its details again before downloading.")]
    StaleApproval,
    #[error("Hugging Face has rate limited the catalog. Try again shortly.")]
    RateLimited,
    #[error("The Hugging Face catalog is unavailable. Try again.")]
    Unavailable,
}

impl DiscoveryError {
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Invalid => "invalid_model_discovery",
            Self::Unsupported => "unsupported_model_artifact",
            Self::StaleApproval => "model_catalog_pin_changed",
            Self::RateLimited => "model_catalog_rate_limited",
            Self::Unavailable => "model_catalog_unavailable",
        }
    }
}

fn component(value: &str, maximum: usize) -> bool {
    let bytes = value.as_bytes();
    bytes.len() <= maximum
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && !value.contains("..")
}

#[must_use]
pub fn repository_valid(value: &str) -> bool {
    value
        .split_once('/')
        .is_some_and(|(owner, name)| component(owner, 96) && component(name, 96))
}

#[must_use]
pub fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[must_use]
pub fn managed_profile(repository: &str, revision: &str, file: &str) -> String {
    profile_hash("hf-", &format!("{repository}/{revision}/{file}"))
}

#[must_use]
pub fn managed_embedding_profile(
    repository: &str,
    revision: &str,
    file: &str,
    pin: &str,
) -> String {
    profile_hash("hf-emb-", &format!("{repository}/{revision}/{file}/{pin}"))
}

fn profile_hash(prefix: &str, value: &str) -> String {
    let digest = Sha256::digest(value);
    let mut profile = String::with_capacity(prefix.len() + 64);
    profile.push_str(prefix);
    for byte in digest {
        let _ = write!(profile, "{byte:02x}");
    }
    profile
}

#[must_use]
pub fn variant_quantization(file: &str) -> Option<&'static str> {
    if !component(file, 180) {
        return None;
    }
    let stem = file.strip_suffix(".gguf")?;
    ["Q4_K_M", "Q5_K_M", "Q8_0"]
        .into_iter()
        .find(|quantization| {
            stem.strip_suffix(quantization)
                .is_some_and(|base| base.ends_with('-') || base.ends_with('_'))
        })
}

#[must_use]
pub fn approved_imported_embedding(model: &Artifact) -> bool {
    let (Some(source), Some(file)) = (&model.source, &model.remote_file) else {
        return false;
    };
    model.kind == Some(ArtifactKind::Embedding)
        && source.license.as_deref() == Some("apache-2.0")
        && repository_valid(&source.repository)
        && hex(&source.revision, 40)
        && variant_quantization(file).is_some()
        && hex(&model.sha256, 64)
        && model.profile
            == managed_embedding_profile(&source.repository, &source.revision, file, &model.sha256)
        && model.file == format!("{}.gguf", model.profile)
}
