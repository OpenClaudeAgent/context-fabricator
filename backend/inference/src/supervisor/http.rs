//! Authenticated loopback requests and bounded readiness probes.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{Duration, Instant};

use super::WorkerError;

impl From<ureq::Error> for WorkerError {
    fn from(error: ureq::Error) -> Self {
        match error {
            ureq::Error::Timeout(_) => Self::Deadline,
            _ => Self::Unavailable,
        }
    }
}

#[derive(Clone)]
pub(crate) struct Client {
    address: SocketAddrV4,
    secret: String,
    agent: ureq::Agent,
    timeout: Duration,
    deadline: Option<Instant>,
}

impl Client {
    pub(crate) fn new(address: SocketAddrV4, secret: &str, timeout: Duration) -> Option<Self> {
        if address.ip() != &Ipv4Addr::LOCALHOST
            || address.port() == 0
            || !(32..=64).contains(&secret.len())
            || !secret.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return None;
        }
        let agent = ureq::Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .timeout_global(Some(timeout))
            .build()
            .into();
        Some(Self {
            address,
            secret: secret.to_owned(),
            agent,
            timeout,
            deadline: None,
        })
    }

    pub(crate) fn with_deadline(&self, deadline: Option<Instant>) -> Self {
        Self {
            deadline,
            ..self.clone()
        }
    }

    fn remaining(&self) -> Option<Duration> {
        match self.deadline {
            Some(deadline) => deadline
                .checked_duration_since(Instant::now())
                .map(|left| left.min(self.timeout)),
            None => Some(self.timeout),
        }
    }

    pub(crate) fn post(
        &self,
        path: &str,
        payload: &serde_json::Value,
    ) -> Result<Vec<u8>, WorkerError> {
        let mut response = self
            .agent
            .post(format!("http://{}{path}", self.address))
            .header("Authorization", format!("Bearer {}", self.secret))
            .header("Content-Type", "application/json")
            .config()
            .timeout_global(Some(self.remaining().ok_or(WorkerError::Deadline)?))
            .build()
            .send(payload.to_string())?;
        if !response.status().is_success() {
            return Err(WorkerError::Unavailable);
        }
        response
            .body_mut()
            .with_config()
            .limit(64 * 1024)
            .read_to_vec()
            .map_err(WorkerError::from)
    }

    pub(super) fn ready(&self, alias: &str) -> bool {
        let limit = Duration::from_millis(400);
        let Some(timeout) = self.remaining().map(|left| left.min(limit)) else {
            return false;
        };
        let Some(probe) = Self::new(self.address, &self.secret, timeout) else {
            return false;
        };
        probe.model(alias).is_ok()
    }

    pub(crate) fn context_tokens(&self, alias: &str) -> Result<usize, WorkerError> {
        self.model(alias)?["meta"]["n_ctx"]
            .as_u64()
            .and_then(|context| usize::try_from(context).ok())
            .filter(|count| *count > 0)
            .ok_or(WorkerError::Unavailable)
    }

    fn model(&self, alias: &str) -> Result<serde_json::Value, WorkerError> {
        let mut response = self
            .agent
            .get(format!("http://{}/v1/models", self.address))
            .header("Authorization", format!("Bearer {}", self.secret))
            .config()
            .timeout_global(Some(self.remaining().ok_or(WorkerError::Deadline)?))
            .build()
            .call()?;
        if !response.status().is_success() {
            return Err(WorkerError::Unavailable);
        }
        let bytes = response
            .body_mut()
            .with_config()
            .limit(16 * 1024)
            .read_to_vec()?;
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .map_err(|_| WorkerError::Unavailable)?
            .get_mut("data")
            .and_then(|data| data.get_mut(0))
            .filter(|model| model["id"] == alias)
            .map(serde_json::Value::take)
            .ok_or(WorkerError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{SocketAddr, TcpListener};

    #[test]
    fn stalled_model_metadata_reports_the_deadline() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let SocketAddr::V4(address) = listener.local_addr()? else {
            return Err("expected IPv4 fixture".into());
        };
        let client = Client::new(address, &"a".repeat(32), Duration::from_secs(3))
            .ok_or("fixture client")?
            .with_deadline(Some(Instant::now() + Duration::from_millis(300)));
        assert!(matches!(
            client.context_tokens("fixture"),
            Err(WorkerError::Deadline)
        ));
        listener.set_nonblocking(true)?;
        assert!(
            listener.accept().is_ok(),
            "the metadata request reached the origin"
        );
        Ok(())
    }
}
