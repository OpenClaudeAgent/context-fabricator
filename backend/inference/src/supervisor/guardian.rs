//! Launches and reaps the native worker and follows the backend's parent pipe.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::io::{self, BufRead, Read, Write as _};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::Profile;
use crate::PROFILE;

pub const GUARD_ARGUMENT: &str = "--guard-inference-worker";

pub(super) struct Guardian {
    child: Child,
    pipe: Option<ChildStdin>,
}

pub(super) struct Endpoint {
    pub(super) address: SocketAddrV4,
    pub(super) secret: String,
}

impl Guardian {
    pub(super) fn spawn(
        binary: &Path,
        model: &Path,
        profile: Profile,
    ) -> io::Result<(Self, Endpoint)> {
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
            .local_addr()?
            .port();
        let mut random = [0u8; 24];
        getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
        let mut secret = String::with_capacity(48);
        for byte in random {
            write!(secret, "{byte:02x}").map_err(io::Error::other)?;
        }
        let mut child = Command::new(std::env::current_exe()?)
            .arg(GUARD_ARGUMENT)
            .arg(match profile {
                Profile::Embedding => "embedding",
                Profile::Generation(_) => "generation",
            })
            .arg(profile.alias())
            .arg(binary)
            .arg(model)
            .arg(port.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let pipe = child.stdin.take();
        let mut guard = Self { child, pipe };
        let pipe = guard
            .pipe
            .as_mut()
            .ok_or_else(|| io::Error::other("missing worker session pipe"))?;
        writeln!(pipe, "{secret}")?;
        let endpoint = Endpoint {
            address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, port),
            secret,
        };
        Ok((guard, endpoint))
    }

    pub(super) fn is_running(&mut self) -> io::Result<bool> {
        self.child.try_wait().map(|status| status.is_none())
    }
}

impl Drop for Guardian {
    fn drop(&mut self) {
        self.pipe.take();
        // The guard reaps llama-server before it exits, including after backend SIGKILL.
        let deadline = Instant::now() + Duration::from_secs(2);
        while matches!(self.child.try_wait(), Ok(None)) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Entry point used by the bundled backend's process guardian.
///
/// # Errors
/// Returns an error for malformed arguments or when the child process cannot be managed.
pub fn run_guard(mut args: impl Iterator<Item = OsString>) -> io::Result<()> {
    let invalid = || io::Error::other("invalid managed worker arguments");
    let mode = args.next().ok_or_else(invalid)?;
    let alias = args.next().ok_or_else(invalid)?;
    let profile = match (mode.to_str(), alias.to_str()) {
        (Some("embedding"), Some(PROFILE)) => Profile::Embedding,
        (Some("generation"), Some(alias)) => Profile::Generation(
            crate::generation::MODELS
                .into_iter()
                .find(|model| model.id == alias)
                .ok_or_else(invalid)?,
        ),
        _ => return Err(invalid()),
    };
    let binary = PathBuf::from(args.next().ok_or_else(invalid)?);
    let model = PathBuf::from(args.next().ok_or_else(invalid)?);
    let port: u16 = args
        .next()
        .and_then(|value| value.to_str()?.parse().ok())
        .filter(|value| *value > 0)
        .ok_or_else(invalid)?;
    if args.next().is_some() || !binary.is_absolute() || !model.is_absolute() {
        return Err(invalid());
    }
    let (credential, parent_closed) = parent_channel()?;
    let directory = tempfile::tempdir()?;
    let key = directory.path().join("session.key");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&key)?
        .write_all(credential.as_bytes())?;
    let mut command = Command::new(binary);
    command
        .env_clear()
        .args(["--offline", "--host", "127.0.0.1", "--port"])
        .arg(port.to_string())
        .arg("--api-key-file")
        .arg(&key)
        .arg("--model")
        .arg(model)
        .args([
            "--alias",
            profile.alias(),
            "--no-ui",
            "--log-disable",
            "--gpu-layers",
            "all",
            "--ctx-size",
            "4096",
            "--parallel",
            "1",
            "--threads",
            "4",
        ]);
    if matches!(profile, Profile::Embedding) {
        command.args([
            "--embedding",
            "--pooling",
            "last",
            "--embd-normalize",
            "2",
            "--batch-size",
            "4096",
            "--ubatch-size",
            "4096",
        ]);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    while matches!(
        parent_closed.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ) {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
    }
    let _ = child.kill();
    child.wait()?;
    Ok(())
}

fn parent_channel() -> io::Result<(String, mpsc::Receiver<()>)> {
    let (credential_tx, credential_rx) = mpsc::sync_channel(1);
    let (closed, parent_closed) = mpsc::channel();
    thread::Builder::new()
        .name("inference-parent".into())
        .spawn(move || {
            let mut input = io::stdin().lock();
            let mut secret = String::new();
            if input.by_ref().take(65).read_line(&mut secret).is_ok()
                && secret.len() == 49
                && secret.as_bytes()[..48].iter().all(u8::is_ascii_hexdigit)
                && credential_tx.send(secret).is_ok()
            {
                let _ = io::copy(&mut input, &mut io::sink());
            }
            let _ = closed.send(());
        })?;
    let credential = credential_rx
        .recv()
        .map_err(|_| io::Error::other("missing worker key"))?;
    Ok((credential, parent_closed))
}
