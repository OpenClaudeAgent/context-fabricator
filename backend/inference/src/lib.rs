//! Typed embedding inputs and outputs, plus adapters for local inference runtimes.

pub mod artifacts;
pub mod generation;
pub mod llama_cpp;
mod qwen3;
pub mod supervisor;

pub use qwen3::{MAX_PASSAGE_BYTES, PROFILE};

pub const DIMENSIONS: usize = 1024;

#[derive(Clone, Copy)]
pub enum Input<'a> {
    Passage { title: &'a str, content: &'a str },
    Query(&'a str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum EmbeddingError {
    #[error("The local embedding runtime is unavailable.")]
    Runtime,
    #[error("The local embedding runtime is busy.")]
    Busy,
    #[error("The embedding vector is incompatible with the active profile.")]
    Vector,
    #[error("A source passage exceeds the embedding profile input budget.")]
    PassageTooLarge,
}

#[derive(Debug)]
pub struct Embedding(Vec<f32>);

impl Embedding {
    /// Validates and normalizes a vector for cosine retrieval.
    ///
    /// # Errors
    /// Returns an error for a vector with an invalid dimension or norm.
    pub fn new(mut values: Vec<f32>) -> Result<Self, EmbeddingError> {
        if values.len() != DIMENSIONS {
            return Err(EmbeddingError::Vector);
        }
        let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
        if !norm.is_finite() || norm <= f32::EPSILON {
            return Err(EmbeddingError::Vector);
        }
        for value in &mut values {
            *value /= norm;
        }
        Ok(Self(values))
    }

    #[must_use]
    pub fn values(&self) -> &[f32] {
        &self.0
    }
}

pub trait Embedder: Send + Sync {
    /// Returns an embedding from the active local profile.
    ///
    /// # Errors
    /// Returns a typed error if the input, runtime or vector is incompatible.
    fn embed(&self, input: Input<'_>) -> Result<Embedding, EmbeddingError>;
}
