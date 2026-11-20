//! Authenticated loopback requests and bounded readiness probes.

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;

use super::WorkerError;

#[derive(Clone)]
pub(crate) struct Client {
    address: SocketAddrV4,
    secret: String,
    agent: ureq::Agent,
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
        })
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
            .send(payload.to_string())
            .map_err(|_| WorkerError::Unavailable)?;
        if !response.status().is_success() {
            return Err(WorkerError::Unavailable);
        }
        response
            .body_mut()
            .with_config()
            .limit(64 * 1024)
            .read_to_vec()
            .map_err(|_| WorkerError::Unavailable)
    }

    pub(super) fn ready(&self, alias: &str) -> bool {
        let Some(probe) = Self::new(self.address, &self.secret, Duration::from_millis(400)) else {
            return false;
        };
        let mut response = match probe
            .agent
            .get(format!("http://{}/v1/models", probe.address))
            .header("Authorization", format!("Bearer {}", probe.secret))
            .call()
        {
            Ok(response) if response.status().is_success() => response,
            _ => return false,
        };
        let Ok(bytes) = response
            .body_mut()
            .with_config()
            .limit(16 * 1024)
            .read_to_vec()
        else {
            return false;
        };
        serde_json::from_slice::<serde_json::Value>(&bytes)
            .is_ok_and(|model| model["data"][0]["id"] == alias)
    }
}
