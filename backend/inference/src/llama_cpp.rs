//! Adapter for an authenticated llama.cpp embedding worker on local loopback.

use std::io::Read;
use std::net::SocketAddrV4;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::supervisor::{Client, ManagedWorker, WorkerError};
use crate::{Embedder, Embedding, EmbeddingError, Input, PROFILE};

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
        embed(&self.client, input, PROFILE)
    }

    fn embed_warm(&self, input: Input<'_>, deadline: Instant) -> Result<Embedding, EmbeddingError> {
        let _permit = self
            .admission
            .try_lock()
            .map_err(|_| EmbeddingError::Busy)?;
        embed(&self.client.with_deadline(Some(deadline)), input, PROFILE)
    }
}

impl From<WorkerError> for EmbeddingError {
    fn from(error: WorkerError) -> Self {
        match error {
            WorkerError::Busy => Self::Busy,
            WorkerError::Unavailable | WorkerError::Deadline => Self::Runtime,
        }
    }
}

impl Embedder for ManagedWorker {
    fn embed(&self, input: Input<'_>) -> Result<Embedding, EmbeddingError> {
        self.run(|client| embed(client, input, self.alias()))
    }

    fn embed_warm(&self, input: Input<'_>, deadline: Instant) -> Result<Embedding, EmbeddingError> {
        self.run_warm(|client| embed(&client.with_deadline(Some(deadline)), input, self.alias()))
    }
}

fn embed(client: &Client, input: Input<'_>, profile: &str) -> Result<Embedding, EmbeddingError> {
    let text = input.qwen_text()?;
    let payload = serde_json::json!({"input": text, "encoding_format": "float", "model": profile});
    let bytes = client.post("/v1/embeddings", &payload)?;
    let response: Response = serde_json::from_slice(&bytes).map_err(|_| EmbeddingError::Vector)?;
    if response.model != profile {
        return Err(EmbeddingError::Vector);
    }
    let [one] = <[Data; 1]>::try_from(response.data).map_err(|_| EmbeddingError::Vector)?;
    if one.index != 0 {
        return Err(EmbeddingError::Vector);
    }
    Embedding::new(one.embedding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::{Profile, Runtime};
    use std::sync::Arc;

    #[test]
    fn warm_embedding_does_not_start_a_cold_managed_worker()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = Arc::new(Runtime::with_weight_policy(|_, _| true));
        let worker =
            ManagedWorker::with_runtime(std::path::Path::new("/"), Profile::Embedding, runtime)
                .ok_or("expected eligible model")?;
        assert!(matches!(
            worker.embed_warm(
                Input::Query("test"),
                Instant::now() + Duration::from_secs(1)
            ),
            Err(EmbeddingError::Runtime)
        ));
        Ok(())
    }

    #[test]
    fn warm_embedding_honors_an_expired_deadline() -> Result<(), Box<dyn std::error::Error>> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let address = match listener.local_addr()? {
            std::net::SocketAddr::V4(address) => address,
            std::net::SocketAddr::V6(_) => unreachable!(),
        };
        let worker = LocalWorker::connect(address, &"a".repeat(32))?;
        assert!(matches!(
            worker.embed_warm(
                Input::Query("test"),
                Instant::now()
                    .checked_sub(Duration::from_secs(1))
                    .ok_or("clock")?
            ),
            Err(EmbeddingError::Runtime)
        ));
        listener.set_nonblocking(true)?;
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        Ok(())
    }
}
