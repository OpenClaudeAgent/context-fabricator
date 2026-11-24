//! Typed embedding inputs and outputs, plus adapters for local inference runtimes.

pub mod generation;
pub mod llama_cpp;
mod qwen3;
pub mod supervisor;

pub use qwen3::{
    ALTERNATE_EMBEDDING_PROFILE, ALTERNATE_EMBEDDING_SHA256, MAX_PASSAGE_BYTES, PROFILE,
};

pub const DIMENSIONS: usize = 1024;

#[derive(Clone, Copy)]
pub enum Input<'a> {
    Passage { title: &'a str, content: &'a str },
    Query(&'a str),
}

impl Input<'_> {
    /// Returns the text prepared for the Qwen embedding worker.
    ///
    /// # Errors
    /// Returns an error when a passage exceeds the profile's input size.
    pub fn qwen_text(self) -> Result<String, EmbeddingError> {
        qwen3::encode(self)
    }
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
    /// Whether this session currently has an eligible embedding worker.
    fn is_ready(&self) -> bool {
        true
    }

    /// Returns an embedding from the active local profile.
    ///
    /// # Errors
    /// Returns a typed error if the input, runtime or vector is incompatible.
    fn embed(&self, input: Input<'_>) -> Result<Embedding, EmbeddingError>;

    /// Attempts an embedding only if a worker is already warm, within the supplied deadline.
    ///
    /// # Errors
    /// Returns a typed error if no warm worker is available or the operation fails.
    fn embed_warm(
        &self,
        _input: Input<'_>,
        _deadline: std::time::Instant,
    ) -> Result<Embedding, EmbeddingError> {
        Err(EmbeddingError::Runtime)
    }
}
