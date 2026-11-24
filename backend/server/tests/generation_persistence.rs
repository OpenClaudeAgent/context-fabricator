use std::error::Error;
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use ctxfab_inference::generation::{
    ALTERNATE_FILE, ALTERNATE_PROFILE, ALTERNATE_SHA256, GENERATION_FILE, GENERATION_PROFILE,
};
use serde_json::{Value, json};

struct Session {
    child: Child,
    url: String,
    token: String,
    agent: ureq::Agent,
}
impl Session {
    fn start(data: &Path) -> Result<Self, Box<dyn Error>> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ctxfab-server"))
            .arg(data)
            .arg("--app-session")
            .env_remove("CTXFAB_INFERENCE_DIRECTORY")
            .env_remove("CTXFAB_EMBEDDING_ADDRESS")
            .env_remove("CTXFAB_EMBEDDING_KEY_FILE")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(8);
        let endpoint = loop {
            if let Ok(bytes) = std::fs::read(data.join("endpoint.json"))
                && let Ok(endpoint) = serde_json::from_slice::<Value>(&bytes)
            {
                break endpoint;
            }
            if child.try_wait()?.is_some() || Instant::now() >= deadline {
                return Err("backend startup failed".into());
            }
            thread::sleep(Duration::from_millis(20));
        };
        Ok(Self {
            child,
            url: endpoint["url"].as_str().ok_or("URL")?.into(),
            token: endpoint["token"].as_str().ok_or("token")?.into(),
            agent: ureq::Agent::config_builder()
                .proxy(None)
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(10)))
                .build()
                .into(),
        })
    }
    fn models(&self) -> Result<Value, Box<dyn Error>> {
        let bytes = self
            .agent
            .get(format!("{}/v1/generation/models", self.url))
            .header("Authorization", format!("Bearer {}", self.token))
            .call()?
            .body_mut()
            .read_to_vec()?;
        Ok(serde_json::from_slice(&bytes)?)
    }
    fn select(&self, profile: &str) -> Result<u16, Box<dyn Error>> {
        Ok(self
            .agent
            .put(format!("{}/v1/generation/models", self.url))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Type", "application/json")
            .send(json!({"profile":profile}).to_string().as_bytes())?
            .status()
            .as_u16())
    }
    fn stop(mut self) -> Result<(), Box<dyn Error>> {
        drop(self.child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err("backend shutdown failed".into())
                };
            }
            thread::sleep(Duration::from_millis(20));
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "shutdown timeout").into())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn fixture() -> Result<(tempfile::TempDir, PathBuf), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let data = directory.path().join("data");
    std::fs::DirBuilder::new().mode(0o700).create(&data)?;
    let assets = data.join("inference");
    std::fs::create_dir(&assets)?;
    let worker = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../quality/answers/mock_worker.py");
    std::fs::copy(worker, assets.join("llama-server"))?;
    std::fs::set_permissions(
        assets.join("llama-server"),
        std::fs::Permissions::from_mode(0o755),
    )?;
    std::fs::write(assets.join(GENERATION_FILE), b"fixture model")?;
    std::fs::write(assets.join(ALTERNATE_FILE), b"fixture model boot failure")?;
    Ok((directory, data))
}

#[test]
fn selection_persists_only_after_warmup_and_matching_catalog_fingerprint()
-> Result<(), Box<dyn Error>> {
    let (_temporary, data) = fixture()?;
    let assets = data.join("inference");
    let first = Session::start(&data)?;
    assert_eq!(first.models()?["activeProfile"], GENERATION_PROFILE);
    assert_eq!(first.select(ALTERNATE_PROFILE)?, 503);
    assert_eq!(first.models()?["activeProfile"], GENERATION_PROFILE);
    std::fs::write(assets.join(ALTERNATE_FILE), b"fixture model")?;
    let database = rusqlite::Connection::open(data.join("context.sqlite3"))?;
    database.execute_batch("CREATE TRIGGER reject_selection BEFORE INSERT ON generation_selection BEGIN SELECT RAISE(FAIL, 'fixture'); END;")?;
    assert_eq!(first.select(ALTERNATE_PROFILE)?, 503);
    assert_eq!(first.models()?["activeProfile"], GENERATION_PROFILE);
    database.execute_batch("DROP TRIGGER reject_selection;")?;
    assert_eq!(first.select(ALTERNATE_PROFILE)?, 200);
    assert_eq!(first.models()?["activeProfile"], ALTERNATE_PROFILE);
    let saved: (String, String) = database.query_row(
        "SELECT profile, fingerprint FROM generation_selection",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(saved, (ALTERNATE_PROFILE.into(), ALTERNATE_SHA256.into()));
    first.stop()?;
    let second = Session::start(&data)?;
    assert_eq!(second.models()?["activeProfile"], ALTERNATE_PROFILE);
    second.stop()?;
    database.execute(
        "UPDATE generation_selection SET fingerprint = ?",
        ["0".repeat(64)],
    )?;
    let mismatch = Session::start(&data)?;
    assert_eq!(mismatch.models()?["activeProfile"], GENERATION_PROFILE);
    mismatch.stop()?;
    database.execute(
        "UPDATE generation_selection SET fingerprint = ?",
        [ALTERNATE_SHA256],
    )?;
    std::fs::remove_file(assets.join(ALTERNATE_FILE))?;
    let missing = Session::start(&data)?;
    assert_eq!(missing.models()?["activeProfile"], GENERATION_PROFILE);
    missing.stop()?;
    Ok(())
}
