use std::error::Error;
use std::ffi::OsStr;
use std::os::unix::fs::{DirBuilderExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use ctxfab_inference::{generation::GENERATION_FILE, supervisor::EMBEDDING_FILE};
use ctxfab_server::domain::corpus::snapshot::Location;
use serde::Deserialize;
use serde_json::{Value, json};

#[path = "source_snapshot_cli/answers.rs"]
mod answers;

#[path = "source_snapshot_cli/retrieval.rs"]
mod retrieval;

#[path = "source_snapshot_cli/structure.rs"]
mod structure;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Reference {
    repository: String,
    repository_id: u64,
    commit: String,
    path: String,
    file: String,
    blob: String,
    line: u32,
    query: String,
    needle: String,
}

#[test]
#[ignore = "requires pinned source files and the CLI; run quality/m07/reference-corpora.sh"]
fn pinned_references_keep_exact_coordinates_through_the_cli() -> Result<(), Box<dyn Error>> {
    let fixture_dir =
        PathBuf::from(std::env::var_os("CTXFAB_REFERENCE_CORPUS").ok_or("fixture path")?);
    let cli = std::env::var_os("CTXFAB_REFERENCE_CLI").ok_or("CLI path")?;
    let references: Vec<Reference> =
        serde_json::from_str(include_str!("../../../quality/m07/reference-corpora.json"))?;
    let compare = std::env::var_os("CTXFAB_REFERENCE_STRUCTURAL_BOUNDARIES").map(PathBuf::from);
    run_reference(&fixture_dir, &cli, &references, compare.as_deref(), false)?;
    if let Some(manifest) = compare {
        run_reference(&fixture_dir, &cli, &references, Some(&manifest), true)?;
    }
    Ok(())
}

fn run_reference(
    fixture_dir: &Path,
    cli: &OsStr,
    references: &[Reference],
    comparison: Option<&Path>,
    use_structure: bool,
) -> Result<(), Box<dyn Error>> {
    let profile = if use_structure { "syntax" } else { "lines" };
    eprintln!("Code chunking profile {profile}");
    let work = tempfile::tempdir()?;
    let data = work.path().join("data");
    std::fs::DirBuilder::new().mode(0o700).create(&data)?;
    let embeddings = stage_model(&data, "CTXFAB_REFERENCE_EMBEDDING_ASSETS", EMBEDDING_FILE)?;
    let code_answers = stage_model(&data, "CTXFAB_REFERENCE_ANSWER_ASSETS", GENERATION_FILE)?;
    let origin = work.path().join("origin");
    std::fs::create_dir(&origin)?;
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../quality/m07/github_fixture.py");
    let _origin = Backend(Command::new("python3").arg(script).arg(&origin).spawn()?);
    wait_for(|| Ok(origin.join("port").is_file().then_some(())))?;
    let port = std::fs::read_to_string(origin.join("port"))?;
    let mut backend = Backend::start(&data, port.trim())?;
    let mut local = Local::open(&data)?;
    local.collect_references(references)?;
    let database = data.join("context.sqlite3");
    if let Some(manifest) = comparison {
        backend.stop()?;
        structure::prepare(&database, fixture_dir, manifest, references, use_structure)?;
        backend = Backend::start(&data, port.trim())?;
        local = Local::open(&data)?;
    }
    for reference in references {
        reference.verify_cli(
            &search(
                cli,
                &data,
                &reference.query,
                reference.repository_id,
                "fixture",
                "bm25",
            )?,
            fixture_dir,
        )?;
        assert_eq!(
            search(
                cli,
                &data,
                &reference.query,
                reference.repository_id,
                "unindexed",
                "bm25"
            )?,
            [] as [Value; 0]
        );
    }
    let local = if embeddings {
        let progress = retrieval::rebuild(&local)?;
        backend.stop()?;
        backend = Backend::start(&data, port.trim())?;
        let restarted = Local::open(&data)?;
        assert_eq!(restarted.read("/v1/retrieval")?, progress);
        retrieval::qualify(cli, &data, fixture_dir, references, !use_structure)?;
        restarted
    } else {
        local
    };
    if code_answers {
        answers::qualify(
            cli,
            &data,
            fixture_dir,
            references,
            embeddings,
            !use_structure,
        )?;
    }
    if comparison.is_some() {
        structure::report_size(&database)?;
    }
    let origin_hits = std::fs::read_to_string(origin.join("hits.log"))?;
    assert!(origin_hits.lines().all(|line| line.starts_with("GET ")));
    let counted = local.read("/v1/status")?;
    assert_eq!(counted["outboundAttempts"], origin_hits.lines().count());
    eprintln!(
        "Verified {} counted read-only GitHub hops",
        origin_hits.lines().count()
    );
    backend.stop()?;
    Ok(())
}

fn stage_model(data: &Path, variable: &str, weight: &str) -> Result<bool, Box<dyn Error>> {
    let Some(assets) = std::env::var_os(variable).map(PathBuf::from) else {
        return Ok(false);
    };
    let inference = data.join("inference");
    std::fs::create_dir_all(&inference)?;
    let worker = inference.join("llama-server");
    if !worker.exists() {
        symlink(std::fs::canonicalize(assets.join("llama-server"))?, worker)?;
    }
    symlink(
        std::fs::canonicalize(assets.join(weight))?,
        inference.join(weight),
    )?;
    Ok(true)
}

fn search(
    cli: &OsStr,
    data: &Path,
    query: &str,
    id: u64,
    branch: &str,
    mode: &str,
) -> Result<Vec<Value>, Box<dyn Error>> {
    let response = Command::new(cli)
        .args([
            "search",
            query,
            "--mode",
            mode,
            "--repository-id",
            &id.to_string(),
            "--branch",
            branch,
        ])
        .env("CTXFAB_DATA_DIRECTORY", data)
        .output()?;
    assert!(
        response.status.success(),
        "{}",
        String::from_utf8_lossy(&response.stderr)
    );
    Ok(serde_json::from_slice(&response.stdout)?)
}

impl Reference {
    fn url(&self) -> String {
        format!(
            "https://github.com/{}/blob/{}/{}",
            self.repository, self.commit, self.path
        )
    }

    fn verify_cli(&self, hits: &[Value], fixture_dir: &Path) -> Result<(), Box<dyn Error>> {
        let hit = hits
            .iter()
            .find(|hit| {
                hit["revision"] == self.commit
                    && hit["location"]["path"] == self.path
                    && hit["location"]["startLine"]
                        .as_u64()
                        .is_some_and(|line| line <= u64::from(self.line))
                    && hit["location"]["endLine"]
                        .as_u64()
                        .is_some_and(|line| line >= u64::from(self.line))
            })
            .ok_or("CLI search did not retrieve the pinned line")?;
        let source = std::fs::read_to_string(fixture_dir.join(&self.file))?;
        let (_, bytes) = self.verify_location(hit, &source)?;
        assert!(bytes.contains(&self.needle));
        eprintln!("Verified CLI citation {hit}");
        Ok(())
    }
    fn verify_location<'source>(
        &self,
        hit: &Value,
        source: &'source str,
    ) -> Result<(Location, &'source str), Box<dyn Error>> {
        let expected_source = format!("github:{}:{}", self.repository_id, self.path);
        assert_eq!(hit["source"], expected_source);
        assert_eq!(hit["kind"], "current");
        assert_eq!(hit["trackedRef"], "refs/heads/fixture");
        assert_eq!(hit["revision"], self.commit);
        let locator: Location = serde_json::from_value(hit["location"].clone())?;
        assert_eq!(locator.file_url, self.url());
        assert_eq!(locator.blob, self.blob);
        assert_eq!(locator.path, self.path);
        let start = usize::try_from(locator.start_byte)?;
        let end = usize::try_from(locator.end_byte)?;
        let original = source
            .get(start..end)
            .ok_or("invalid UTF-8 source coordinates")?;
        assert_ne!(original, "");
        let leading_lines = source[..start]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count();
        let covered_lines = original.bytes().filter(|byte| *byte == b'\n').count();
        assert_eq!(usize::try_from(locator.start_line)?, leading_lines + 1);
        assert_eq!(
            usize::try_from(locator.end_line)?,
            leading_lines + covered_lines + usize::from(!original.ends_with('\n'))
        );
        Ok((locator, original))
    }
}

struct Local {
    agent: ureq::Agent,
    url: String,
    authorization: String,
}

impl Local {
    fn open(data: &Path) -> Result<Self, Box<dyn Error>> {
        let endpoint: Value = serde_json::from_slice(&std::fs::read(data.join("endpoint.json"))?)?;
        let url = endpoint["url"].as_str().ok_or("endpoint URL")?;
        assert!(url.starts_with("http://127.0.0.1:"));
        let token = endpoint["token"].as_str().ok_or("endpoint token")?;
        let agent = ureq::Agent::config_builder()
            .proxy(None)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(75)))
            .build()
            .into();
        Ok(Self {
            agent,
            url: url.to_owned(),
            authorization: format!("Bearer {token}"),
        })
    }

    fn collect_references(&self, references: &[Reference]) -> Result<(), Box<dyn Error>> {
        let authenticated = self.write(
            "PUT",
            "/v1/github/credential",
            &json!({"revision": 0, "token": "github_pat_fixture_primary"}),
        )?;
        let mut revision = authenticated["revision"]
            .as_u64()
            .ok_or("configuration revision")?;
        for reference in references {
            revision = self.collect(reference, revision)?;
        }
        Ok(())
    }

    fn collect(&self, reference: &Reference, revision: u64) -> Result<u64, Box<dyn Error>> {
        let saved = self.write("PUT", "/v1/github/repositories", &json!({
            "revision": revision, "repository": reference.repository, "expectedId": reference.repository_id,
            "intervalMinutes": 60, "features": {
                "currentCode": true, "history": false, "actions": false, "codeGraph": false,
            },
        }))?;
        let revision = saved["revision"].as_u64().ok_or("saved revision")?;
        let published = wait_for(|| {
            let state = self
                .read("/v1/github")
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let collection = state["repositories"]
                .as_array()
                .and_then(|repositories| {
                    repositories.iter().find(|repository| {
                        repository["repository"]["id"] == reference.repository_id
                    })
                })
                .and_then(|repository| repository.get("codeCollection"))
                .ok_or_else(|| std::io::Error::other("missing current-code collection"))?;
            match collection["phase"].as_str() {
                Some("succeeded") => Ok(Some(collection.clone())),
                Some("failed" | "credentials") => {
                    Err(std::io::Error::other(collection.to_string()))
                }
                _ => Ok(None),
            }
        })?;
        assert_eq!(published["commit"], reference.commit);
        assert!(published["publishedAtMs"].is_i64());
        Ok(revision)
    }

    fn write(&self, method: &str, path: &str, body: &Value) -> Result<Value, Box<dyn Error>> {
        let url = format!("{}{path}", self.url);
        let request = if method == "PUT" {
            self.agent.put(&url)
        } else {
            self.agent.post(&url)
        };
        let mut response = request
            .header("Authorization", &self.authorization)
            .header("Content-Type", "application/json")
            .send(body.to_string())?;
        read_json(response.body_mut())
    }

    fn read(&self, path: &str) -> Result<Value, Box<dyn Error>> {
        let mut response = self
            .agent
            .get(format!("{}{path}", self.url))
            .header("Authorization", &self.authorization)
            .call()?;
        read_json(response.body_mut())
    }
}

fn read_json(body: &mut ureq::Body) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_slice(
        &body.with_config().limit(128 * 1024).read_to_vec()?,
    )?)
}

struct Backend(Child);

impl Backend {
    fn start(data: &Path, port: &str) -> Result<Self, Box<dyn Error>> {
        let mut backend = Self(
            Command::new(env!("CARGO_BIN_EXE_ctxfab-server"))
                .arg(data)
                .args(["--app-session", "--github-scheduled-fixture", port])
                .env_remove("CTXFAB_INFERENCE_DIRECTORY")
                .env_remove("CTXFAB_EMBEDDING_ADDRESS")
                .env_remove("CTXFAB_EMBEDDING_KEY_FILE")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()?,
        );
        wait_for(|| {
            if let Some(status) = backend.0.try_wait()? {
                return Err(std::io::Error::other(format!(
                    "backend exited with {status}"
                )));
            }
            Ok(data.join("endpoint.json").is_file().then_some(()))
        })?;
        Ok(backend)
    }

    fn stop(mut self) -> Result<(), Box<dyn Error>> {
        drop(self.0.stdin.take());
        assert!(wait_for(|| self.0.try_wait())?.success());
        Ok(())
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn wait_for<T>(mut poll: impl FnMut() -> std::io::Result<Option<T>>) -> Result<T, Box<dyn Error>> {
    for _ in 0..100 {
        if let Some(result) = poll()? {
            return Ok(result);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err("timed out waiting for the test backend".into())
}
