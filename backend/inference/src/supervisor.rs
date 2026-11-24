//! Lazily starts one authenticated worker per profile and ties it to the app session.

mod guardian;
mod http;
mod runtime;
mod worker;

use crate::generation::Model;
use crate::{ALTERNATE_EMBEDDING_PROFILE, PROFILE};

pub use guardian::{GUARD_ARGUMENT, run_guard};
pub(crate) use http::Client;

pub(crate) const WORKER_CONTEXT_TOKENS: usize = 4_096;
pub use runtime::Runtime;
pub use worker::ManagedWorker;

pub const EMBEDDING_FILE: &str = "Qwen3-Embedding-0.6B-Q8_0.gguf";
pub const ALTERNATE_EMBEDDING_FILE: &str = "Qwen3-Embedding-0.6B-f16.gguf";

#[derive(Clone)]
pub enum Profile {
    Embedding,
    EmbeddingF16,
    ImportedEmbedding(ImportedEmbedding),
    Generation(Model),
}

#[derive(Clone)]
pub struct ImportedEmbedding {
    alias: String,
    file: String,
}

#[must_use]
pub fn valid_embedding_alias(alias: &str) -> bool {
    matches!(alias, PROFILE | ALTERNATE_EMBEDDING_PROFILE) || valid_imported_embedding_alias(alias)
}

fn valid_imported_embedding_alias(alias: &str) -> bool {
    alias.strip_prefix("hf-emb-").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

impl Profile {
    #[must_use]
    pub fn imported_embedding(alias: &str) -> Option<Self> {
        valid_imported_embedding_alias(alias).then(|| {
            Self::ImportedEmbedding(ImportedEmbedding {
                alias: alias.to_owned(),
                file: format!("{alias}.gguf"),
            })
        })
    }

    fn alias(&self) -> &str {
        match self {
            Self::Embedding => PROFILE,
            Self::EmbeddingF16 => ALTERNATE_EMBEDDING_PROFILE,
            Self::ImportedEmbedding(model) => &model.alias,
            Self::Generation(model) => &model.id,
        }
    }

    fn file(&self) -> &str {
        match self {
            Self::Embedding => EMBEDDING_FILE,
            Self::EmbeddingF16 => ALTERNATE_EMBEDDING_FILE,
            Self::ImportedEmbedding(model) => &model.file,
            Self::Generation(model) => &model.file,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum WorkerError {
    Busy,
    Deadline,
    Unavailable,
}
