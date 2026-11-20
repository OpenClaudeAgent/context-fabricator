//! Lazily starts one authenticated worker per profile and ties it to the app session.

mod guardian;
mod http;
mod worker;

use crate::PROFILE;
use crate::generation::Model;

pub use guardian::{GUARD_ARGUMENT, run_guard};
pub(crate) use http::Client;
pub use worker::ManagedWorker;

pub const EMBEDDING_FILE: &str = "Qwen3-Embedding-0.6B-Q8_0.gguf";

#[derive(Clone, Copy)]
pub enum Profile {
    Embedding,
    Generation(Model),
}

impl Profile {
    const fn alias(self) -> &'static str {
        match self {
            Self::Embedding => PROFILE,
            Self::Generation(model) => model.id,
        }
    }

    const fn file(self) -> &'static str {
        match self {
            Self::Embedding => EMBEDDING_FILE,
            Self::Generation(model) => model.file,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum WorkerError {
    Busy,
    Unavailable,
}
