//! Pinned weight artifacts for the supported local inference profiles.

use serde::Serialize;

use crate::PROFILE;
use crate::generation::{
    ALTERNATE_FILE, ALTERNATE_LABEL, ALTERNATE_PROFILE, GENERATION_FILE, GENERATION_LABEL,
    GENERATION_PROFILE,
};
use crate::supervisor::EMBEDDING_FILE;

pub struct PinnedArtifact {
    pub profile: &'static str,
    pub label: &'static str,
    pub kind: ArtifactKind,
    pub file: &'static str,
    pub repository: &'static str,
    pub revision: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactKind {
    Generation,
    Embedding,
}

const GENERATION_REPO: &str = "unsloth/Qwen3-4B-Instruct-2507-GGUF";
const GENERATION_REVISION: &str = "a06e946bb6b655725eafa393f4a9745d460374c9";

pub const ARTIFACTS: [PinnedArtifact; 3] = [
    PinnedArtifact {
        profile: PROFILE,
        label: "Qwen3 Embedding 0.6B · Q8_0",
        kind: ArtifactKind::Embedding,
        file: EMBEDDING_FILE,
        repository: "Qwen/Qwen3-Embedding-0.6B-GGUF",
        revision: "370f27d7550e0def9b39c1f16d3fbaa13aa67728",
        bytes: 639_150_592,
        sha256: "06507c7b42688469c4e7298b0a1e16deff06caf291cf0a5b278c308249c3e439",
    },
    PinnedArtifact {
        profile: GENERATION_PROFILE,
        label: GENERATION_LABEL,
        kind: ArtifactKind::Generation,
        file: GENERATION_FILE,
        repository: GENERATION_REPO,
        revision: GENERATION_REVISION,
        bytes: 2_497_281_120,
        sha256: "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597",
    },
    PinnedArtifact {
        profile: ALTERNATE_PROFILE,
        label: ALTERNATE_LABEL,
        kind: ArtifactKind::Generation,
        file: ALTERNATE_FILE,
        repository: GENERATION_REPO,
        revision: GENERATION_REVISION,
        bytes: 2_889_514_080,
        sha256: "5bde5e9d883622acb02bf77fe7dcbc56a8b9a9ad4be78a72ca23a532658b4ecb",
    },
];
