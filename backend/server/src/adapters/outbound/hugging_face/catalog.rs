//! Pinned weight artifacts for the supported local inference profiles.

use std::borrow::Cow::Borrowed;

use ctxfab_inference::generation::{
    ALTERNATE_FILE, ALTERNATE_LABEL, ALTERNATE_PROFILE, ALTERNATE_SHA256, GENERATION_FILE,
    GENERATION_LABEL, GENERATION_PROFILE, GENERATION_SHA256,
};
use ctxfab_inference::supervisor::{ALTERNATE_EMBEDDING_FILE, EMBEDDING_FILE};
use ctxfab_inference::{ALTERNATE_EMBEDDING_PROFILE, ALTERNATE_EMBEDDING_SHA256, PROFILE};

use crate::domain::model::{Artifact, ArtifactKind, ModelSource};

const EMBEDDING_SOURCE: ModelSource = ModelSource {
    license: None,
    label: Borrowed("Hugging Face · Qwen"),
    repository: Borrowed("Qwen/Qwen3-Embedding-0.6B-GGUF"),
    revision: Borrowed("370f27d7550e0def9b39c1f16d3fbaa13aa67728"),
    url: Borrowed(
        "https://huggingface.co/Qwen/Qwen3-Embedding-0.6B-GGUF/tree/370f27d7550e0def9b39c1f16d3fbaa13aa67728",
    ),
};
const GENERATION_SOURCE: ModelSource = ModelSource {
    license: None,
    label: Borrowed("Hugging Face · Unsloth"),
    repository: Borrowed("unsloth/Qwen3-4B-Instruct-2507-GGUF"),
    revision: Borrowed("a06e946bb6b655725eafa393f4a9745d460374c9"),
    url: Borrowed(
        "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/tree/a06e946bb6b655725eafa393f4a9745d460374c9",
    ),
};

pub(super) const ARTIFACTS: [Artifact; 4] = [
    Artifact {
        profile: Borrowed(PROFILE),
        label: Some(Borrowed("Qwen3 Embedding 0.6B · Q8_0")),
        kind: Some(ArtifactKind::Embedding),
        source: Some(EMBEDDING_SOURCE),
        file: Borrowed(EMBEDDING_FILE),
        remote_file: None,
        bytes: 639_150_592,
        sha256: Borrowed("06507c7b42688469c4e7298b0a1e16deff06caf291cf0a5b278c308249c3e439"),
    },
    Artifact {
        profile: Borrowed(ALTERNATE_EMBEDDING_PROFILE),
        label: Some(Borrowed("Qwen3 Embedding 0.6B · F16")),
        kind: Some(ArtifactKind::Embedding),
        source: Some(EMBEDDING_SOURCE),
        file: Borrowed(ALTERNATE_EMBEDDING_FILE),
        remote_file: None,
        bytes: 1_197_629_632,
        sha256: Borrowed(ALTERNATE_EMBEDDING_SHA256),
    },
    Artifact {
        profile: Borrowed(GENERATION_PROFILE),
        label: Some(Borrowed(GENERATION_LABEL)),
        kind: Some(ArtifactKind::Generation),
        source: Some(GENERATION_SOURCE),
        file: Borrowed(GENERATION_FILE),
        remote_file: None,
        bytes: 2_497_281_120,
        sha256: Borrowed(GENERATION_SHA256),
    },
    Artifact {
        profile: Borrowed(ALTERNATE_PROFILE),
        label: Some(Borrowed(ALTERNATE_LABEL)),
        kind: Some(ArtifactKind::Generation),
        source: Some(GENERATION_SOURCE),
        file: Borrowed(ALTERNATE_FILE),
        remote_file: None,
        bytes: 2_889_514_080,
        sha256: Borrowed(ALTERNATE_SHA256),
    },
];
