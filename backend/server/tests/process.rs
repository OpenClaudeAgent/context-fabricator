use std::error::Error;
use std::io;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for<T>(mut read: impl FnMut() -> io::Result<Option<T>>) -> io::Result<T> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(value) = read()? {
            return Ok(value);
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "backend process did not reach the expected state",
    ))
}

fn server(directory: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ctxfab-server"));
    command
        .arg(directory)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[test]
fn app_session_stops_when_owner_pipe_closes() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let data = directory.path().join("data");
    let endpoint = data.join("endpoint.json");
    let mut backend = Running(
        server(&data)
            .arg("--app-session")
            .stdin(Stdio::piped())
            .spawn()?,
    );
    wait_for(|| Ok(endpoint.exists().then_some(())))?;
    assert!(
        !server(&data).status()?.success(),
        "a second process acquired the same directory"
    );
    assert!(endpoint.exists());
    drop(backend.0.stdin.take());
    let exit: ExitStatus = wait_for(|| backend.0.try_wait())?;
    assert!(exit.success());
    assert!(
        !endpoint.exists(),
        "discovery was left behind after shutdown"
    );
    Ok(())
}

#[test]
fn invalid_launch_arguments_fail_without_a_session() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let result = server(directory.path()).arg("--invalid").status()?;
    assert!(!result.success());
    assert!(!directory.path().join("endpoint.json").exists());
    Ok(())
}

#[cfg(all(target_os = "macos", debug_assertions))]
#[test]
fn fixture_arguments_require_an_explicit_valid_correlation() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let result = server(directory.path())
        .args(["--fixture-store", "not-a-uuid"])
        .stderr(Stdio::piped())
        .output()?;
    assert!(!result.status.success());
    assert!(String::from_utf8(result.stderr)?.contains("invalid fixture correlation"));
    assert!(!directory.path().join("endpoint.json").exists());
    Ok(())
}

#[path = "fixtures/corpus.rs"]
mod corpus_fixtures;

#[test]
#[ignore = "requires the macOS app bundle; run make m04"]
fn packaged_helper_serves_fresh_and_migrated_corpus() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::DirBuilderExt;

    use ctxfab_server::adapters::outbound::sqlite::SqliteStore;
    use ctxfab_server::application::ports::CorpusStore;
    use ctxfab_server::domain::corpus::{Batch, CollectionAccess, Commit};
    use serde_json::{Value, json};

    let bundle = std::env::var_os("CTXFAB_TEST_APP_BUNDLE")
        .map(std::path::PathBuf::from)
        .ok_or("CTXFAB_TEST_APP_BUNDLE is required")?;
    let executables = bundle.join("Contents/MacOS");
    let helper = executables.join("ctxfab-server");
    let cli = executables.join("ctxfab");

    for migrated in [false, true] {
        let directory = tempfile::tempdir()?;
        let data = directory.path().join("data");
        let database = data.join("context.sqlite3");
        if migrated {
            std::fs::DirBuilder::new().mode(0o700).create(&data)?;
            corpus_fixtures::seed_v1(&database)?;
        }
        let mut backend = Running(
            Command::new(&helper)
                .arg(&data)
                .arg("--app-session")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .spawn()?,
        );
        wait_for(|| Ok(data.join("endpoint.json").exists().then_some(())))?;

        let call = |args: &[&str]| -> Result<Value, Box<dyn Error>> {
            let output = Command::new(&cli)
                .env("CTXFAB_DATA_DIRECTORY", &data)
                .args(args)
                .output()?;
            if !output.status.success() {
                return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
            }
            Ok(serde_json::from_slice(&output.stdout)?)
        };

        let legacy = call(&["search", "migrationtoken"])?;
        if migrated {
            assert_eq!(legacy[0]["revision"], "commit-old");
            assert!(legacy[0].get("trackedRef").is_none());
            assert_eq!(call(&["settings"])?["revision"], 7);
        } else {
            assert_eq!(legacy, json!([]));
        }
        let store = SqliteStore::open(&database)?;
        let documents = corpus_fixtures::documents();
        let batch = Batch {
            collection: "fixture",
            generation: 0,
            previous_cursor: None,
            next_cursor: "page-1",
            documents: &documents,
        };
        assert_eq!(store.commit_batch(&batch)?, Commit::Indexed);
        let status = call(&["status"])?;
        let total = 3 + u8::from(migrated);
        assert_eq!(status["collectedDocuments"], total);
        assert_eq!(status["indexedDocuments"], total);
        assert_eq!(status["connectedSources"], 0);
        assert_eq!(status["outboundAttempts"], 0);
        let hits = call(&["search", "fabricator"])?;
        let hits = hits.as_array().ok_or("expected corpus evidence")?;
        assert_eq!(hits.len(), 3);
        assert!(
            hits.iter()
                .any(|hit| hit["source"] == corpus_fixtures::SOURCE
                    && hit["revision"] == "commit-b"
                    && hit["trackedRef"] == corpus_fixtures::MAIN
                    && hit["kind"] == "current")
        );
        assert!(
            hits.iter()
                .any(|hit| hit["revision"] == "commit-a" && hit["kind"] == "history")
        );
        assert!(
            hits.iter()
                .any(|hit| hit["revision"] == "run-190-attempt-2" && hit["kind"] == "workflow")
        );

        assert!(store.set_collection_access("fixture", 0, CollectionAccess::Lost)?);
        let retained = call(&["search", "stablecurrent"])?;
        assert_eq!(retained[0]["access"], "retained");
        store.delete_collection("fixture")?;
        assert_eq!(call(&["search", "fabricator"])?, json!([]));

        drop(backend.0.stdin.take());
        assert!(wait_for(|| backend.0.try_wait())?.success());
        let mode = if migrated { "migrated" } else { "fresh" };
        println!("Verified {mode} packaged SQLite/FTS5 through the authenticated CLI.");
    }
    Ok(())
}

#[cfg(debug_assertions)]
#[test]
fn scheduler_stops_with_its_owner_and_reconciles_a_killed_session() -> Result<(), Box<dyn Error>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .timeout_global(Some(Duration::from_secs(3)))
        .build()
        .into();
    observe_scheduler(
        Path::new(env!("CARGO_BIN_EXE_ctxfab-server")),
        |data, args| {
            let endpoint: serde_json::Value =
                serde_json::from_slice(&std::fs::read(data.join("endpoint.json"))?)?;
            let url = endpoint["url"].as_str().ok_or("fixture URL missing")?;
            let token = endpoint["token"].as_str().ok_or("fixture bearer missing")?;
            let route = match args {
                ["status"] => "/v1/status",
                ["search", "incrementalfixture"] => "/v1/search?query=incrementalfixture",
                _ => return Err("unknown fixture route".into()),
            };
            let mut response = agent
                .get(format!("{url}{route}"))
                .header("Authorization", format!("Bearer {token}"))
                .call()?;
            Ok(serde_json::from_slice(&response.body_mut().read_to_vec()?)?)
        },
    )
}

#[cfg(debug_assertions)]
#[test]
#[ignore = "requires the macOS app bundle; run make m05"]
fn packaged_scheduler_recovers_incrementally_and_accounts_for_crash() -> Result<(), Box<dyn Error>>
{
    let bundle = std::env::var_os("CTXFAB_TEST_APP_BUNDLE")
        .map(std::path::PathBuf::from)
        .ok_or("CTXFAB_TEST_APP_BUNDLE is required")?;
    let executables = bundle.join("Contents/MacOS");
    let cli = executables.join("ctxfab");
    observe_scheduler(&executables.join("ctxfab-server"), |data, args| {
        let output = Command::new(&cli)
            .env("CTXFAB_DATA_DIRECTORY", data)
            .args(args)
            .output()?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        Ok(serde_json::from_slice(&output.stdout)?)
    })
}

#[cfg(debug_assertions)]
fn observe_scheduler(
    executable: &Path,
    call: impl Fn(&Path, &[&str]) -> Result<serde_json::Value, Box<dyn Error>>,
) -> Result<(), Box<dyn Error>> {
    use ctxfab_server::adapters::outbound::sqlite::SqliteStore;
    use ctxfab_server::application::ports::Repository;

    let directory = tempfile::tempdir()?;
    let data = directory.path().join("data");
    let start = |fixture: bool| -> Result<Running, Box<dyn Error>> {
        let mut command = Command::new(executable);
        command
            .arg(&data)
            .arg("--app-session")
            .stdin(Stdio::piped())
            .stdout(Stdio::null());
        if fixture {
            command.arg("--sync-fixture");
        }
        let child = Running(command.spawn()?);
        wait_for(|| {
            let discovered = std::fs::read(data.join("endpoint.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|value| value["processId"].as_u64());
            Ok((discovered == Some(u64::from(child.0.id()))).then_some(()))
        })?;
        Ok(child)
    };
    let status = || call(&data, &["status"]);
    let search = || call(&data, &["search", "incrementalfixture"]);

    let mut backend = start(true)?;
    wait_for(|| {
        Ok(search()
            .ok()
            .filter(|value| value[0]["revision"] == "second"))
    })?;
    assert_eq!(status()?["outboundAttempts"], 3);
    assert_eq!(status()?["connectedSources"], 0);
    drop(backend.0.stdin.take());
    assert!(wait_for(|| backend.0.try_wait())?.success());
    assert!(!data.join("endpoint.json").exists());
    let stop = SqliteStore::open(&data.join("context.sqlite3"))?;
    assert_eq!(stop.status()?.outbound_attempts, 3);
    assert!(
        stop.diagnostics()?
            .iter()
            .any(|event| event.code == "backend_stopped")
    );
    drop(stop);

    let mut restarted = start(true)?;
    wait_for(|| Ok(status().ok().filter(|value| value["outboundPending"] == 1)))?;
    assert_eq!(status()?["outboundAttempts"], 4);
    restarted.0.kill()?;
    assert!(!restarted.0.wait()?.success());

    let mut recovered = start(false)?;
    let state = status()?;
    assert_eq!(state["outboundAttempts"], 4);
    assert_eq!(state["outboundPending"], 0);
    assert_eq!(state["outboundUncertain"], 1);
    assert_eq!(state["connectedSources"], 0);
    assert_eq!(search()?[0]["revision"], "second");
    drop(recovered.0.stdin.take());
    assert!(wait_for(|| recovered.0.try_wait())?.success());
    println!(
        "Owner EOF joined the scheduler; actual process kill preserved the cursor and one uncertain attempt."
    );
    Ok(())
}

#[cfg(debug_assertions)]
#[test]
fn embedded_telemetry_is_served_by_the_live_backend() -> Result<(), Box<dyn Error>> {
    observe_telemetry(Path::new(env!("CARGO_BIN_EXE_ctxfab-server")))
}

#[cfg(debug_assertions)]
#[test]
#[ignore = "requires the macOS app bundle; run make m06"]
fn packaged_backend_persists_correlated_signals() -> Result<(), Box<dyn Error>> {
    let bundle = std::env::var_os("CTXFAB_TEST_APP_BUNDLE")
        .map(std::path::PathBuf::from)
        .ok_or("CTXFAB_TEST_APP_BUNDLE is required")?;
    observe_telemetry(&bundle.join("Contents/MacOS/ctxfab-server"))
}

#[cfg(debug_assertions)]
fn observe_telemetry(executable: &Path) -> Result<(), Box<dyn Error>> {
    use serde_json::Value;

    const SENTINEL: &str = "m06secretfixtureforredaction";
    let directory = tempfile::tempdir()?;
    let data = directory.path().join("data");
    let mut backend = Running(
        Command::new(executable)
            .arg(&data)
            .args(["--app-session", "--telemetry-fixture"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()?,
    );
    let (agent, url, token) = telemetry_fixture_client(&data)?;
    let bearer = format!("Bearer {token}");
    let get = |path| {
        telemetry_json(
            agent
                .get(format!("{url}{path}"))
                .header("Authorization", &bearer)
                .call()?,
        )
    };
    let command = |command: &str| {
        telemetry_json(
            agent
                .put(format!("{url}/v1/telemetry"))
                .header("Authorization", &bearer)
                .header("Content-Type", "application/json")
                .send(format!("{{\"command\":\"{command}\"}}"))?,
        )
    };
    let import = agent
        .post(format!("{url}/v1/fixtures/import"))
        .header("Authorization", &bearer)
        .send_empty()?;
    assert_eq!(import.status(), 202);
    let import_id = trace_header(&import)?;
    wait_for(|| {
        Ok(get("/v1/status")
            .ok()
            .filter(|value| value["indexedDocuments"] == 1))
    })?;
    assert!(
        command("enableDebug")?["health"]["debugUntil"]
            .as_i64()
            .is_some_and(|date| date > 0)
    );
    let mut query = agent
        .get(format!(
            "{url}/v1/search?query=incrementalfixture+{SENTINEL}"
        ))
        .header("Authorization", &bearer)
        .call()?;
    assert_eq!(query.status(), 200);
    let query_id = trace_header(&query)?;
    let hits: Value = serde_json::from_slice(&query.body_mut().read_to_vec()?)?;
    assert_eq!(hits[0]["revision"], "second");
    let snapshot = wait_for(|| {
        let snapshot = get("/v1/telemetry").ok();
        Ok(snapshot.filter(|item| {
            item["metrics"].as_array().is_some_and(|metrics| {
                metrics
                    .iter()
                    .any(|metric| metric["name"] == "api_query" && metric["value"] == 1)
            })
        }))
    })?;
    check_trace(
        &snapshot,
        &import_id,
        &["api_import", "scheduler_collection", "index_commit"],
    )?;
    check_trace(&snapshot, &query_id, &["api_query", "lexical_lookup"])?;
    let logs = snapshot["logs"].as_array().ok_or("logs missing")?;
    assert!(
        logs.iter()
            .any(|record| record["traceId"] == query_id && record["level"] == "debug")
    );
    let exported = snapshot.to_string();
    assert!(!exported.contains(SENTINEL));
    assert!(!exported.contains(&token));
    assert_eq!(command("disableDebug")?["health"]["debugUntil"], 0);
    assert_eq!(command("purgeExpired")?["health"]["retentionHours"], 24);
    assert_eq!(get("/v1/status")?["connectedSources"], 0);
    assert_eq!(get("/v1/status")?["outboundAttempts"], 1);
    drop(backend.0.stdin.take());
    assert!(wait_for(|| backend.0.try_wait())?.success());

    confirm_telemetry_shutdown(&data)?;
    println!(
        "Live helper correlated import and query spans; published SDK metrics and redacted logs; drained on owner EOF."
    );
    Ok(())
}

#[cfg(debug_assertions)]
fn telemetry_fixture_client(data: &Path) -> Result<(ureq::Agent, String, String), Box<dyn Error>> {
    use serde_json::Value;
    let endpoint = data.join("endpoint.json");
    wait_for(|| Ok(endpoint.exists().then_some(())))?;
    let discovered: Value = serde_json::from_slice(&std::fs::read(endpoint)?)?;
    let url = discovered["url"]
        .as_str()
        .ok_or("missing fixture address")?
        .to_owned();
    let token = discovered["token"]
        .as_str()
        .ok_or("missing fixture authorization")?
        .to_owned();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(3)))
        .build()
        .into();
    Ok((agent, url, token))
}

#[cfg(debug_assertions)]
fn trace_header(response: &ureq::http::Response<ureq::Body>) -> Result<String, Box<dyn Error>> {
    Ok(response
        .headers()
        .get("x-context-trace-id")
        .ok_or("missing trace header")?
        .to_str()?
        .to_owned())
}

#[cfg(debug_assertions)]
fn telemetry_json(
    mut response: ureq::http::Response<ureq::Body>,
) -> Result<serde_json::Value, Box<dyn Error>> {
    assert_eq!(response.status(), 200);
    Ok(serde_json::from_slice(&response.body_mut().read_to_vec()?)?)
}

#[cfg(debug_assertions)]
fn confirm_telemetry_shutdown(data: &Path) -> Result<(), Box<dyn Error>> {
    let stored = rusqlite::Connection::open(data.join("telemetry.sqlite3"))?;
    let shutdown_logs: i64 = stored.query_row(
        "SELECT count(*) FROM signals WHERE signal = 'log' AND name = 'backend_shutdown'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(shutdown_logs, 1);
    Ok(())
}

#[cfg(debug_assertions)]
fn check_trace(
    snapshot: &serde_json::Value,
    trace_id: &str,
    names: &[&str],
) -> Result<(), Box<dyn Error>> {
    let spans = snapshot["traces"].as_array().ok_or("traces missing")?;
    let mut parent = None;
    for name in names {
        let span = spans
            .iter()
            .find(|span| span["name"] == *name && span["traceId"] == trace_id)
            .ok_or_else(|| format!("missing span {name} in trace"))?;
        assert_eq!(span["parentSpanId"].as_str(), parent);
        parent = span["spanId"].as_str();
        assert!(parent.is_some());
        assert!(span["value"].as_i64().is_some_and(|duration| duration >= 0));
    }
    Ok(())
}

#[cfg(debug_assertions)]
#[test]
fn unavailable_telemetry_keeps_the_backend_and_startup_diagnostics_available()
-> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::DirBuilderExt;

    let directory = tempfile::tempdir()?;
    let data = directory.path().join("data");
    std::fs::DirBuilder::new().mode(0o700).create(&data)?;
    std::fs::create_dir(data.join("telemetry.sqlite3"))?;
    let mut backend = Running(
        server(&data)
            .arg("--app-session")
            .stdin(Stdio::piped())
            .spawn()?,
    );
    let (agent, url, token) = telemetry_fixture_client(&data)?;
    let bearer = format!("Bearer {token}");
    let get = |path| {
        agent
            .get(format!("{url}{path}"))
            .header("Authorization", &bearer)
            .call()
    };
    let status = telemetry_json(get("/v1/status")?)?;
    assert_eq!(status["status"], "ready");
    let logs = telemetry_json(get("/v1/diagnostics")?)?;
    assert_eq!(logs[0]["code"], "backend_started");
    assert_eq!(get("/v1/telemetry")?.status(), 503);
    drop(backend.0.stdin.take());
    assert!(wait_for(|| backend.0.try_wait())?.success());
    Ok(())
}
