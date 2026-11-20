//! Adapter for an authenticated llama.cpp embedding worker on local loopback.

use std::io::Read;
use std::net::SocketAddrV4;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;

use crate::supervisor::{Client, ManagedWorker, WorkerError};
use crate::{Embedder, Embedding, EmbeddingError, Input, PROFILE, qwen3};

pub struct LocalWorker {
    client: Client,
    admission: Mutex<()>,
}

#[derive(Deserialize)]
struct Response {
    model: String,
    data: Vec<Data>,
}

#[derive(Deserialize)]
struct Data {
    index: u8,
    embedding: Vec<f32>,
}

impl LocalWorker {
    /// Reads explicit local-worker configuration. The default runtime is lexical only.
    ///
    /// # Errors
    /// Returns a runtime error for malformed or incomplete configuration.
    pub fn configured() -> Result<Option<Self>, EmbeddingError> {
        let address = std::env::var_os("CTXFAB_EMBEDDING_ADDRESS");
        let file = std::env::var_os("CTXFAB_EMBEDDING_KEY_FILE");
        if address.is_none() && file.is_none() {
            return Ok(None);
        }
        let address = address
            .and_then(|value| value.to_str()?.parse().ok())
            .ok_or(EmbeddingError::Runtime)?;
        let file = std::fs::File::open(file.ok_or(EmbeddingError::Runtime)?)
            .map_err(|_| EmbeddingError::Runtime)?;
        let mut secret = String::new();
        file.take(256)
            .read_to_string(&mut secret)
            .map_err(|_| EmbeddingError::Runtime)?;
        Self::connect(address, secret.trim()).map(Some)
    }

    /// Creates a worker client restricted to the configured loopback listener.
    ///
    /// # Errors
    /// Returns a runtime error for a non-loopback address or invalid session key.
    pub fn connect(address: SocketAddrV4, secret: &str) -> Result<Self, EmbeddingError> {
        let client =
            Client::new(address, secret, Duration::from_secs(15)).ok_or(EmbeddingError::Runtime)?;
        Ok(Self {
            client,
            admission: Mutex::new(()),
        })
    }
}

impl Embedder for LocalWorker {
    fn embed(&self, input: Input<'_>) -> Result<Embedding, EmbeddingError> {
        let _permit = self
            .admission
            .try_lock()
            .map_err(|_| EmbeddingError::Busy)?;
        embed(&self.client, input)
    }
}

impl From<WorkerError> for EmbeddingError {
    fn from(error: WorkerError) -> Self {
        match error {
            WorkerError::Busy => Self::Busy,
            WorkerError::Unavailable => Self::Runtime,
        }
    }
}

impl Embedder for ManagedWorker {
    fn embed(&self, input: Input<'_>) -> Result<Embedding, EmbeddingError> {
        self.run(|client| embed(client, input))
    }
}

fn embed(client: &Client, input: Input<'_>) -> Result<Embedding, EmbeddingError> {
    let text = qwen3::encode(input)?;
    let payload = serde_json::json!({"input": text, "encoding_format": "float", "model": PROFILE});
    let bytes = client.post("/v1/embeddings", &payload)?;
    let response: Response = serde_json::from_slice(&bytes).map_err(|_| EmbeddingError::Vector)?;
    if response.model != PROFILE {
        return Err(EmbeddingError::Vector);
    }
    let [one] = <[Data; 1]>::try_from(response.data).map_err(|_| EmbeddingError::Vector)?;
    if one.index != 0 {
        return Err(EmbeddingError::Vector);
    }
    Embedding::new(one.embedding)
}
