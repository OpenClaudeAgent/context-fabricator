use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ctxfab_server::adapters::outbound::telemetry::Limits;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::io::Write as _;

const REFRESH: &str = "/v1/github/repositories/41/refresh";
const STATE: &str = "/v1/github";

struct GitOrigin {
    http: Origin,
    remote: std::path::PathBuf,
    work: tempfile::TempDir,
}

impl GitOrigin {
    async fn start() -> Result<Self, Box<dyn Error>> {
        let http = Origin::start().await?;
        fs::create_dir(http.directory.path().join("fixture"))?;
        let remote = http.directory.path().join("fixture/alpha.git");
        let work = tempfile::tempdir()?;
        git(work.path(), &["init", "-q", "-b", "main"])?;
        fs::write(
            work.path().join("README.md"),
            "The packfixture is searchable.\n",
        )?;
        // Stable but hard to compress so a full retransfer is visible in the fixture.
        let mut noise = Vec::new();
        for index in 0u32..1024 {
            writeln!(
                noise,
                "{}",
                STANDARD.encode(Sha256::digest(index.to_le_bytes()))
            )?;
        }
        fs::write(work.path().join("unchanged.txt"), noise)?;
        git(work.path(), &["add", "."])?;
        git(work.path(), &["commit", "-qm", "Initial snapshot"])?;
        git(
            work.path(),
            &[
                "clone",
                "-q",
                "--bare",
                ".",
                remote.to_str().ok_or("fixture git path")?,
            ],
        )?;
        git(
            &remote,
            &["config", "uploadpack.allowReachableSHA1InWant", "true"],
        )?;
        fs::write(http.directory.path().join("smart-git"), [])?;
        Ok(Self { http, remote, work })
    }

    async fn configured(&self) -> Result<(Router, Arc<EmbeddedTelemetry>), Box<dyn Error>> {
        let store = Arc::new(SqliteStore::open(Path::new(":memory:"))?);
        self.configured_with_store(&store).await
    }

    async fn configured_with_store(
        &self,
        store: &Arc<SqliteStore>,
    ) -> Result<(Router, Arc<EmbeddedTelemetry>), Box<dyn Error>> {
        let github = self
            .http
            .github(store, Some(Arc::new(memory::Credentials::default())))?;
        let telemetry = Arc::new(EmbeddedTelemetry::open(
            &self.http.directory.path().join("telemetry.sqlite3"),
            24,
            Limits::default(),
        )?);
        let app = traced_app(store, github, telemetry.clone());
        authorize(&app, 0, FIRST, 200).await?;
        call(
            &app,
            "PUT",
            "/v1/github/repositories",
            selection(1, "fixture/alpha", 41),
            200,
        )
        .await?;
        Ok((app, telemetry))
    }

    fn advance(&self) -> Result<(), Box<dyn Error>> {
        fs::write(
            self.work.path().join("README.md"),
            "The packfixture now has newcontents.\n",
        )?;
        git(self.work.path(), &["commit", "-qam", "Update README"])?;
        git(
            self.work.path(),
            &[
                "push",
                "-q",
                self.remote.to_str().ok_or("fixture remote")?,
                "main",
            ],
        )
    }

    fn negotiation(&self) -> Result<String, Box<dyn Error>> {
        Ok(fs::read_to_string(
            self.http.directory.path().join("git-negotiation.log"),
        )?)
    }

    fn reject_current_pin(&self) -> Result<(), Box<dyn Error>> {
        let branch = Command::new("git")
            .args(["rev-parse", "refs/heads/main"])
            .current_dir(&self.remote)
            .output()?;
        if !branch.status.success() {
            return Err("fixture branch unavailable".into());
        }
        fs::write(
            self.http.directory.path().join("reject-pack-pin"),
            branch.stdout.trim_ascii(),
        )?;
        Ok(())
    }
}

#[tokio::test]
async fn branch_files_arrive_in_a_git_pack_and_publish_searchable_citations()
-> Result<(), Box<dyn Error>> {
    let origin = GitOrigin::start().await?;
    let (app, telemetry) = origin.configured().await?;
    let before = origin.http.hits()?.len();
    let attempts = attempts(&app).await?;
    let (publication, _, work_trace) = post_with_trace(&app, REFRESH, 200).await?;
    let hits = origin.http.hits()?;
    let hits = &hits[before..];
    assert!(hits.contains("GET /fixture/alpha.git/info/refs?service=git-upload-pack\n"));
    assert!(hits.contains("POST /fixture/alpha.git/git-upload-pack\n"));
    assert!(!hits.contains("/git/blobs/"));
    assert!(!hits.contains("/git/trees/"));
    assert!(hits.lines().count() <= 6, "{hits}");
    assert_eq!(hits.lines().count(), attempts_in(&app, attempts).await?);
    let trace = work_trace.ok_or("missing work trace")?;
    let commit = publication["commit"].as_str().ok_or("published commit")?;
    assert_citation(&app, "packfixture", commit).await?;
    let initial_negotiation = origin.negotiation()?;

    let before = origin.http.hits()?.len();
    let unchanged = call(&app, "POST", REFRESH, Value::Null, 200).await?;
    assert_eq!(unchanged["changed"], false);
    assert!(!origin.http.hits()?[before..].contains("git-upload-pack"));

    origin.advance()?;
    let next = call(&app, "POST", REFRESH, Value::Null, 200).await?;
    assert_eq!(next["changed"], true);
    let updated_commit = next["commit"].as_str().ok_or("updated commit")?;
    assert_ne!(updated_commit, commit);
    assert_citation(&app, "newcontents", updated_commit).await?;
    let negotiation = origin.negotiation()?;
    let incremental = &negotiation[initial_negotiation.len()..];
    assert!(incremental.contains(commit), "{negotiation}");
    assert!(
        pack_bytes(incremental)? * 10 < pack_bytes(&initial_negotiation)?,
        "{negotiation}"
    );
    telemetry.shutdown();
    verify_git_spans(&app, &trace, hits).await?;
    println!(
        "Fixture: {} HTTP exchanges for the first snapshot, {} Git response bytes initially, {} on update",
        hits.lines().count(),
        pack_bytes(&initial_negotiation)?,
        pack_bytes(incremental)?
    );
    Ok(())
}

#[tokio::test]
#[ignore = "requires the pinned uv Git objects; see quality/github/README.md"]
async fn external_reference_publishes_pinned_uv_files_without_per_file_requests()
-> Result<(), Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../quality/github/uv-reference.json");
    let reference: Value = serde_json::from_slice(&fs::read(path)?)?;
    let commit = reference["commit"].as_str().ok_or("pinned commit")?;
    let expected = &reference["expectedPublication"];
    let source = std::path::PathBuf::from(
        std::env::var_os("CTXFAB_UV_GIT_REFERENCE")
            .ok_or("set CTXFAB_UV_GIT_REFERENCE to the verified bare Git repository")?,
    )
    .canonicalize()?;
    let origin = GitOrigin::start().await?;
    let objects = source.join("objects");
    let objects = objects
        .to_str()
        .ok_or("the fixture needs a UTF-8 Git path")?;
    assert!(!objects.contains(['\n', '\r']));
    fs::write(
        origin.remote.join("objects/info/alternates"),
        format!("{objects}\n"),
    )?;
    let shallow = source.join("shallow");
    if shallow.is_file() {
        fs::copy(&shallow, origin.remote.join("shallow"))?;
    }
    git(&origin.remote, &["update-ref", "refs/heads/main", commit])?;

    let database = origin.http.directory.path().join("corpus.sqlite3");
    let store = Arc::new(SqliteStore::open(&database)?);
    let (app, telemetry) = origin.configured_with_store(&store).await?;
    let before = origin.http.hits()?.len();
    let previous = attempts(&app).await?;
    let started = std::time::Instant::now();
    let (publication, _, work_trace) = post_with_trace(&app, REFRESH, 200).await?;
    let elapsed = started.elapsed();
    assert_eq!(publication, json!({"commit": commit, "changed": true}));
    let hits = origin.http.hits()?;
    let hits = &hits[before..];
    assert!(hits.contains("GET /fixture/alpha.git/info/refs?service=git-upload-pack\n"));
    assert!(hits.contains("POST /fixture/alpha.git/git-upload-pack\n"));
    assert!(!hits.contains("/git/blobs/"));
    assert!(!hits.contains("/git/trees/"));
    assert!(hits.lines().count() <= 6, "{hits}");
    assert_eq!(attempts_in(&app, previous).await?, hits.lines().count());

    let response = call(&app, "GET", STATE, Value::Null, 200).await?;
    let collection = &response["repositories"][0]["codeCollection"];
    assert_eq!(collection["publishedFiles"], expected["paths"]);
    let before = origin.http.hits()?.len();
    let previous = attempts(&app).await?;
    assert_eq!(
        call(&app, "POST", REFRESH, Value::Null, 200).await?,
        json!({"commit": commit, "changed": false})
    );
    let refresh_hits = origin.http.hits()?;
    let refresh_hits = &refresh_hits[before..];
    assert!(!refresh_hits.contains(".git/"));
    assert_eq!(refresh_hits.lines().count(), 2);
    assert_eq!(
        attempts_in(&app, previous).await?,
        refresh_hits.lines().count()
    );
    telemetry.shutdown();
    verify_git_spans(&app, &work_trace.ok_or("missing work trace")?, hits).await?;
    drop(app);
    drop(store);

    let (count, digest) = published_manifest(&database, commit)?;
    assert_eq!(json!(count), expected["paths"]);
    assert_eq!(json!(digest), expected["manifestSha256"]);
    println!("uv {commit}: {count} published paths, manifest SHA-256 {digest}");
    println!(
        "{} first collection exchanges, {} unchanged refresh exchanges, {} Git response bytes, {:?} first collection",
        hits.lines().count(),
        refresh_hits.lines().count(),
        pack_bytes(&origin.negotiation()?)?,
        elapsed
    );
    Ok(())
}

fn published_manifest(database: &Path, commit: &str) -> Result<(u64, String), Box<dyn Error>> {
    let read = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut manifest = read.prepare(
        "SELECT f.path, f.blob, s.revision FROM source_snapshot_files f
        JOIN source_snapshots s ON s.id=f.snapshot_id WHERE s.phase='published' ORDER BY f.path",
    )?;
    let mut rows = manifest.query([])?;
    let mut count = 0u64;
    let mut digest = Sha256::new();
    while let Some(row) = rows.next()? {
        let (path, blob, revision): (String, String, String) =
            (row.get(0)?, row.get(1)?, row.get(2)?);
        assert_eq!(revision, commit);
        digest.update(path);
        digest.update(b"\0");
        digest.update(blob);
        digest.update(b"\n");
        count += 1;
    }
    let mut hex = String::with_capacity(64);
    for byte in digest.finalize() {
        write!(&mut hex, "{byte:02x}")?;
    }
    Ok((count, hex))
}

#[tokio::test]
async fn throttled_git_exchange_stops_and_records_the_shared_provider_cooldown()
-> Result<(), Box<dyn Error>> {
    let origin = GitOrigin::start().await?;
    let (app, _) = origin.configured().await?;
    fs::write(origin.http.directory.path().join("throttle-pack"), [])?;
    let before = origin.http.hits()?.len();
    let start_attempts = attempts(&app).await?;
    call(&app, "POST", REFRESH, Value::Null, 429).await?;
    let hits = origin.http.hits()?;
    let hits = &hits[before..];
    assert_eq!(
        hits.matches("POST /fixture/alpha.git/git-upload-pack")
            .count(),
        1
    );
    assert_eq!(
        hits.lines().count(),
        attempts_in(&app, start_attempts).await?
    );
    assert!(!hits.contains("/git/blobs/"));
    let snapshot = call(&app, "GET", STATE, Value::Null, 200).await?;
    assert_eq!(
        snapshot["repositories"][0]["codeCollection"]["phase"],
        "throttled"
    );
    assert!(snapshot["traffic"]["cooldownUntilMs"].as_i64().is_some());
    let results = call(
        &app,
        "GET",
        "/v1/search?query=packfixture",
        Value::Null,
        200,
    )
    .await?;
    assert_eq!(results, json!([]));
    Ok(())
}

#[tokio::test]
async fn deleting_repository_data_evicts_the_git_negotiation_baseline() -> Result<(), Box<dyn Error>>
{
    let origin = GitOrigin::start().await?;
    let (app, _) = origin.configured().await?;
    let publication = call(&app, "POST", REFRESH, Value::Null, 200).await?;
    let initial_negotiation = origin.negotiation()?;
    call(
        &app,
        "DELETE",
        "/v1/github/repositories/41/data",
        Value::Null,
        200,
    )
    .await?;
    let results = call(
        &app,
        "GET",
        "/v1/search?query=packfixture",
        Value::Null,
        200,
    )
    .await?;
    assert_eq!(results, json!([]));
    assert_eq!(
        call(&app, "POST", REFRESH, Value::Null, 200).await?,
        publication
    );
    let negotiation = origin.negotiation()?;
    let after_delete = &negotiation[initial_negotiation.len()..];
    assert!(
        after_delete
            .lines()
            .all(|line| line.split_whitespace().count() == 1)
    );
    assert!(pack_bytes(after_delete)? > pack_bytes(&initial_negotiation)? / 2);
    Ok(())
}

#[tokio::test]
async fn branch_movement_recovers_when_git_refuses_the_pending_pin() -> Result<(), Box<dyn Error>> {
    let origin = GitOrigin::start().await?;
    let (app, _) = origin.configured().await?;
    origin.reject_current_pin()?;
    call(&app, "POST", REFRESH, Value::Null, 503).await?;
    origin.advance()?;
    wait_out_retry().await;
    let expired = call(&app, "POST", REFRESH, Value::Null, 409).await?;
    assert_eq!(expired["code"], "github_expired_pin");
    wait_out_retry().await;
    let publication = call(&app, "POST", REFRESH, Value::Null, 200).await?;
    assert_citation(
        &app,
        "newcontents",
        publication["commit"].as_str().ok_or("updated commit")?,
    )
    .await
}

async fn wait_out_retry() {
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(3)).await;
    tokio::time::resume();
}

async fn verify_git_spans(app: &Router, trace: &str, hits: &str) -> Result<(), Box<dyn Error>> {
    let page = call(
        app,
        "GET",
        &format!("/v1/telemetry/traces/{trace}"),
        Value::Null,
        200,
    )
    .await?;
    let spans = page["spans"].as_array().ok_or("trace spans")?;
    let parent = spans
        .iter()
        .find(|span| span["name"] == "github_current_collection")
        .ok_or_else(|| format!("missing collection span in {page}"))?;
    for (method, route, wire) in [
        (
            "GET",
            "/{owner}/{repo}.git/info/refs",
            "GET /fixture/alpha.git/info/refs",
        ),
        (
            "POST",
            "/{owner}/{repo}.git/git-upload-pack",
            "POST /fixture/alpha.git/git-upload-pack",
        ),
    ] {
        let recorded = spans.iter().filter(|span| {
            span["httpMethod"] == method
                && span["httpRoute"] == route
                && span["httpStatus"] == 200
                && span["parentSpanId"] == parent["spanId"]
        });
        assert_eq!(
            recorded.count(),
            hits.lines().filter(|hit| hit.starts_with(wire)).count()
        );
    }
    assert!(!page.to_string().contains(FIRST));
    Ok(())
}

async fn assert_citation(app: &Router, word: &str, commit: &str) -> Result<(), Box<dyn Error>> {
    let results = call(
        app,
        "GET",
        &format!("/v1/search?query={word}"),
        Value::Null,
        200,
    )
    .await?;
    let result = results
        .as_array()
        .and_then(|hits| hits.first())
        .ok_or("indexed file")?;
    assert!(
        result
            .to_string()
            .contains(&format!("/blob/{commit}/README.md"))
    );
    Ok(())
}

async fn attempts(app: &Router) -> Result<i64, Box<dyn Error>> {
    call(app, "GET", STATE, Value::Null, 200).await?["traffic"]["attempts"]
        .as_i64()
        .ok_or_else(|| "missing traffic count".into())
}

async fn attempts_in(app: &Router, before: i64) -> Result<usize, Box<dyn Error>> {
    Ok(usize::try_from(attempts(app).await? - before)?)
}

fn pack_bytes(log: &str) -> Result<usize, Box<dyn Error>> {
    log.lines().try_fold(0, |total, line| {
        let bytes = line
            .split_once(' ')
            .ok_or("missing pack size")?
            .0
            .parse::<usize>()?;
        Ok(total + bytes)
    })
}

fn git(directory: &Path, args: &[&str]) -> Result<(), Box<dyn Error>> {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .current_dir(directory)
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    Ok(())
}
