use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use serde_json::json;

const CHILD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const TRACE: &str = "0123456789abcdef0123456789abcdef";

fn run(directory: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_ctxfab"))
        .env("CTXFAB_DATA_DIRECTORY", directory)
        .args(args)
        .output()
}

#[cfg(target_os = "linux")]
#[test]
fn cli_discovers_the_desktop_profile_using_xdg() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let home = directory.path().join("home");
    let xdg = directory.path().join("xdg");
    for (base, data_home) in [
        (xdg.clone(), xdg.clone()),
        (home.join(".local/share"), "relative".into()),
    ] {
        let profile = base.join("context-fabricator");
        std::fs::create_dir_all(&profile)?;
        std::fs::write(
            profile.join("endpoint.json"),
            json!({"apiVersion":2,"url":"http://127.0.0.1:1","token":"fixture"}).to_string(),
        )?;
        let output = Command::new(env!("CARGO_BIN_EXE_ctxfab"))
            .env_remove("CTXFAB_DATA_DIRECTORY")
            .env("HOME", &home)
            .env("XDG_DATA_HOME", data_home)
            .arg("status")
            .output()?;
        assert_eq!(output.status.code(), Some(4));
    }
    Ok(())
}

#[test]
fn cli_reports_bad_invocations_and_invalid_session_discovery() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    assert!(run(directory.path(), &["--help"])?.status.success());
    for args in [
        &["cancel", "not-a-uuid"][..],
        &[
            "github-graph",
            "41",
            "--branch",
            "main",
            "--root",
            "src/lib.rs",
        ][..],
        &["github-graph", "-1", "--branch", "main"][..],
        &["github-actions", "0"][..],
        &["github-actions", "abc"][..],
        &["pause-model", "profile/../v1"][..],
        &["ask", "x", "--request-id", "invalid"][..],
        &["search", "x", "--repository-id", "42"][..],
        &["search", "x", "--repository-id", "no", "--branch", "main"][..],
        &["ask", "x", "--branch", "main", "--branch", "trunk"][..],
        &["ask", "x", "--mode", "hybrid"][..],
        &["ask", "x", "--history-commit", "short"][..],
        &[
            "ask",
            "x",
            "--repository-id",
            "41",
            "--branch",
            "main",
            "--actions-run-id",
            "30",
        ][..],
        &[
            "ask",
            "x",
            "--actions-run-id",
            "30",
            "--actions-run-attempt",
            "1",
        ][..],
    ] {
        assert_eq!(run(directory.path(), args)?.status.code(), Some(2));
    }
    assert_eq!(run(directory.path(), &["unknown"])?.status.code(), Some(2));
    assert_eq!(run(directory.path(), &["status"])?.status.code(), Some(3));

    let endpoint = directory.path().join("endpoint.json");
    for (value, exit_code) in [
        (json!("bad endpoint"), 1),
        (
            json!({"apiVersion":2,"url":"http://127.0.0.1:1","token":"fixture"}),
            4,
        ),
        (
            json!({"apiVersion":1,"url":"not a url","token":"fixture"}),
            1,
        ),
        (
            json!({"apiVersion":1,"url":"http://192.0.2.1:1","token":"fixture"}),
            1,
        ),
    ] {
        std::fs::write(&endpoint, value.to_string())?;
        assert_eq!(
            run(directory.path(), &["status"])?.status.code(),
            Some(exit_code)
        );
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let closed_address = listener.local_addr()?;
    drop(listener);
    let stale = json!({"apiVersion":1,"url":format!("http://{closed_address}"),"token":"fixture"});
    std::fs::write(endpoint, stale.to_string())?;
    assert_eq!(run(directory.path(), &["status"])?.status.code(), Some(3));
    Ok(())
}

#[test]
fn model_download_controls_use_the_shared_routes() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    for (command, route) in [
        ("pause-model", "/v1/model-downloads/qwen3-4b/pause"),
        ("resume-model", "/v1/model-downloads/qwen3-4b/resume"),
    ] {
        assert_route(
            directory.path(),
            &[command, "qwen3-4b"],
            route,
            "202 Accepted",
            true,
        )?;
    }
    assert_route(
        directory.path(),
        &["discard-model-download", "qwen3-4b"],
        "/v1/model-downloads/qwen3-4b/partial",
        "204 No Content",
        true,
    )?;
    Ok(())
}

#[test]
fn runtime_commands_use_the_shared_routes_and_absolute_archive_path() -> Result<(), Box<dyn Error>>
{
    let directory = tempfile::tempdir()?;
    assert_route(
        directory.path(),
        &["download-runtime"],
        "/v1/runtime/download",
        "200 OK",
        true,
    )?;
    assert_route(
        directory.path(),
        &["runtime-status"],
        "/v1/runtime",
        "200 OK",
        true,
    )?;
    assert_route(
        directory.path(),
        &["storage-plan"],
        "/v1/inference/storage",
        "200 OK",
        true,
    )?;
    assert_route(
        directory.path(),
        &["install-runtime", "fixture runtime.zip"],
        "/v1/runtime/install",
        "200 OK",
        true,
    )
}

#[test]
fn inference_setup_uses_reviewed_plan_and_explicit_confirmation() -> Result<(), Box<dyn Error>> {
    const SETUP_ID: &str = "2dbc6441-c57b-4d1a-8806-9760966e8041";
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("plan.json");
    let plan = json!({
        "selection":{"answerProfile":"answer-q4","runtimeSource":"localArchive","embedding":true},
        "fingerprint":"a".repeat(64),
        "canStart":true
    });
    std::fs::write(&path, plan.to_string())?;
    let file = path.to_str().ok_or("invalid fixture path")?;
    assert_route(
        directory.path(),
        &["setup-plan", "localArchive", "hybrid", "answer-q4"],
        "/v1/inference/setup/plan",
        "200 OK",
        true,
    )?;
    assert_route(
        directory.path(),
        &[
            "setup-start",
            file,
            "2dbc6441-c57b-4d1a-8806-9760966e8041",
            "fixture runtime.zip",
        ],
        "/v1/inference/setup",
        "202 Accepted",
        true,
    )?;
    assert_route(
        directory.path(),
        &["setup-archive", SETUP_ID, "fixture runtime.zip"],
        "/v1/inference/setup/archive",
        "202 Accepted",
        true,
    )?;
    assert_route(
        directory.path(),
        &["setup-cancel-archive", SETUP_ID],
        &format!("/v1/inference/setup/archive/{SETUP_ID}"),
        "200 OK",
        true,
    )?;
    assert_route(
        directory.path(),
        &["setup-status"],
        "/v1/inference/setup",
        "200 OK",
        true,
    )?;
    assert_eq!(
        run(
            directory.path(),
            &[
                "setup-start",
                "missing-review.json",
                "2dbc6441-c57b-4d1a-8806-9760966e8041"
            ]
        )?
        .status
        .code(),
        Some(1)
    );
    Ok(())
}

#[test]
fn maintenance_commands_use_explicit_index_and_model_routes() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    for command in [
        "embedding-index",
        "enable-embedding-index",
        "disable-embedding-index",
        "delete-embedding-index",
    ] {
        assert_route(
            directory.path(),
            &[command],
            "/v1/retrieval/index",
            "200 OK",
            true,
        )?;
    }
    assert_route(
        directory.path(),
        &["model-removal-plan", "fixture"],
        "/v1/model-downloads/fixture/removal-plan",
        "200 OK",
        true,
    )?;
    assert_route(
        directory.path(),
        &["delete-model", "fixture", "reviewed identity"],
        "/v1/model-downloads/fixture",
        "204 No Content",
        true,
    )
}

#[test]
fn cli_uses_the_shared_authenticated_loopback_routes() -> Result<(), Box<dyn Error>> {
    const REQUEST: &str = "a35cf9bc-1b6c-492f-9b14-fd8e3a2a991b";
    let directory = tempfile::tempdir()?;
    for (args, route, response, success) in [
        (&["status"][..], "/v1/status", "200 OK", true),
        (&["settings"][..], "/v1/settings", "200 OK", true),
        (&["diagnostics"][..], "/v1/diagnostics", "200 OK", true),
        (
            &["search", "rate limit"][..],
            "/v1/search?query=rate%20limit",
            "200 OK",
            true,
        ),
        (
            &["search", "rate limit", "--mode", "hybrid"][..],
            "/v1/search?query=rate%20limit&mode=hybrid",
            "200 OK",
            true,
        ),
        (
            &[
                "search",
                "rate limit",
                "--branch",
                "release/v1",
                "--repository-id",
                "42",
            ][..],
            "/v1/search?query=rate%20limit&repositoryId=42&branch=release%2Fv1",
            "200 OK",
            true,
        ),
        (&["retrieval"][..], "/v1/retrieval", "200 OK", true),
        (&["ask", "rate limit"][..], "/v1/answers", "200 OK", true),
        (
            &["ask", "rate limit"][..],
            "/v1/answers",
            "503 Service Unavailable",
            false,
        ),
        (
            &["ask", "rate limit", "--request-id", REQUEST][..],
            "/v1/answers",
            "200 OK",
            true,
        ),
        (
            &[
                "ask",
                "rate limit",
                "--repository-id",
                "42",
                "--request-id",
                REQUEST,
                "--branch",
                "release/v1",
            ][..],
            "/v1/answers",
            "200 OK",
            true,
        ),
        (&["models"][..], "/v1/generation/models", "200 OK", true),
        (
            &["github-actions", "41"][..],
            "/v1/github/repositories/41/actions",
            "200 OK",
            true,
        ),
        (
            &["model-downloads"][..],
            "/v1/model-downloads",
            "200 OK",
            true,
        ),
        (
            &["install-model", "fixture-model"][..],
            "/v1/model-downloads",
            "202 Accepted",
            true,
        ),
        (
            &["use-model", "q5"][..],
            "/v1/generation/models",
            "200 OK",
            true,
        ),
        (
            &["cancel", REQUEST][..],
            "/v1/answers/a35cf9bc-1b6c-492f-9b14-fd8e3a2a991b",
            "200 OK",
            true,
        ),
        (&["status"][..], "/v1/status", "401 Unauthorized", false),
        (&["status"][..], "/v1/status", "302 Found", false),
    ] {
        assert_route(directory.path(), args, route, response, success)?;
    }
    Ok(())
}

#[test]
fn graph_cli_uses_the_scoped_graph_route() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    assert_route(
        directory.path(),
        &["github-graph", "41", "--branch", "main"],
        "/v1/github/repositories/41/graph?branch=main",
        "200 OK",
        true,
    )?;
    assert_route(
        directory.path(),
        &[
            "github-graph",
            "41",
            "--branch",
            "main",
            "--root",
            "src/lib.rs",
            "--symbol",
            "crate::go",
        ],
        "/v1/github/repositories/41/graph?branch=main&root=src%2Flib.rs&symbol=crate%3A%3Ago",
        "200 OK",
        true,
    )
}

#[test]
fn ask_sends_the_selected_history_commit_or_actions_attempt() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    assert_route(
        directory.path(),
        &[
            "ask",
            "change",
            "--repository-id",
            "41",
            "--branch",
            "main",
            "--history-commit",
            CHILD,
        ],
        "/v1/answers",
        "200 OK",
        true,
    )?;
    assert_route(
        directory.path(),
        &[
            "ask",
            "run",
            "--branch",
            "main",
            "--repository-id",
            "41",
            "--actions-run-attempt",
            "2",
            "--actions-run-id",
            "30",
        ],
        "/v1/answers",
        "200 OK",
        true,
    )
}

fn assert_route(
    directory: &Path,
    args: &[&str],
    route: &str,
    response: &str,
    success: bool,
) -> Result<(), Box<dyn Error>> {
    let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let endpoint =
        json!({"apiVersion":1,"url":format!("http://{}",server.local_addr()?),"token":"fixture"});
    std::fs::write(directory.join("endpoint.json"), endpoint.to_string())?;

    let mut child = Command::new(env!("CARGO_BIN_EXE_ctxfab"))
        .env("CTXFAB_DATA_DIRECTORY", directory)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .args(args)
        .spawn()?;
    let command = args.strip_prefix(&["--trace"]).unwrap_or(args);
    let result = respond(&server, command, route, response);
    if result.is_err() {
        let _ = child.kill();
    }
    let output = child.wait_with_output()?;
    result?;
    assert_eq!(output.status.success(), success);
    assert_eq!(
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .any(|line| line == format!("ctxfab: trace {TRACE}")),
        args.first() == Some(&"--trace")
    );
    if success {
        let expected = if command.first() == Some(&"delete-model") {
            json!({"deleted": true})
        } else if command.first() == Some(&"discard-model-download") {
            json!({"discarded": true})
        } else if command
            .first()
            .is_some_and(|verb| matches!(*verb, "pause-model" | "resume-model"))
        {
            json!({"accepted": true})
        } else {
            json!({"ready": true})
        };
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout)?,
            expected
        );
    } else if route == "/v1/answers" {
        assert!(String::from_utf8_lossy(&output.stderr).contains("generation is unavailable"));
    }
    Ok(())
}

#[test]
fn trace_flag_correlates_success_and_rejection_without_changing_stdout()
-> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    assert_route(
        directory.path(),
        &["--trace", "status"],
        "/v1/status",
        "200 OK",
        true,
    )?;
    assert_route(
        directory.path(),
        &["--trace", "ask", "rate limit"],
        "/v1/answers",
        "503 Service Unavailable",
        false,
    )
}

fn request_method(verb: Option<&str>) -> &'static str {
    match verb {
        Some(
            "ask" | "install-model" | "pause-model" | "resume-model" | "install-runtime"
            | "download-runtime" | "setup-plan" | "setup-start" | "setup-archive",
        ) => "POST",
        Some("use-model" | "enable-embedding-index" | "disable-embedding-index") => "PUT",
        Some(
            "cancel"
            | "discard-model-download"
            | "delete-model"
            | "delete-embedding-index"
            | "setup-cancel-archive",
        ) => "DELETE",
        _ => "GET",
    }
}

fn respond(
    server: &TcpListener,
    args: &[&str],
    route: &str,
    response: &str,
) -> std::io::Result<()> {
    server.set_nonblocking(true)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut incoming = loop {
        match server.accept() {
            Ok((stream, _)) => break stream,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    };
    incoming.set_nonblocking(false)?;
    incoming.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut reader = BufReader::new(&incoming);
    let mut first_line = String::new();
    reader.read_line(&mut first_line)?;
    let method = request_method(args.first().copied());
    assert_eq!(first_line.trim_end(), format!("{method} {route} HTTP/1.1"));
    let mut authorized = false;
    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header.trim_end().is_empty() {
            break;
        }
        let header = header.trim_end().to_ascii_lowercase();
        authorized |= header == "authorization: bearer fixture";
        if let Some(value) = header.strip_prefix("content-length: ") {
            length = value.parse().map_err(std::io::Error::other)?;
        }
    }
    assert!(authorized);
    if matches!(args[0], "pause-model" | "resume-model" | "download-runtime") {
        assert_eq!(length, 0);
    } else if matches!(method, "POST" | "PUT") || args[0] == "delete-model" {
        let mut payload = vec![0; length];
        reader.read_exact(&mut payload)?;
        let expected = match args[0] {
            "use-model" | "install-model" => json!({"profile":args[1]}),
            "install-runtime" => json!({"archivePath":std::path::absolute(args[1])?}),
            "setup-plan" => {
                json!({"runtimeSource":"localArchive","embedding":true,"answerProfile":"answer-q4"})
            }
            "setup-archive" => {
                json!({"operationId":args[1],"archivePath":std::path::absolute(args[2])?})
            }
            "setup-start" => json!({
                "selection":{"answerProfile":"answer-q4","runtimeSource":"localArchive","embedding":true},
                "fingerprint":"a".repeat(64),
                "operationId":"2dbc6441-c57b-4d1a-8806-9760966e8041",
                "archivePath":std::path::absolute(args[3])?
            }),
            "enable-embedding-index" => json!({"enabled":true}),
            "disable-embedding-index" => json!({"enabled":false}),
            "delete-model" => json!({"identity":args[2]}),
            "ask" if args[1] == "change" => json!({
                "query":"change","repositoryId":41,"branch":"main","historyCommit":CHILD
            }),
            "ask" if args[1] == "run" => json!({
                "query":"run","repositoryId":41,"branch":"main","actionsRunId":30,"actionsRunAttempt":2
            }),
            "ask" if args.contains(&"--repository-id") => json!({
                "query":"rate limit","requestId":"a35cf9bc-1b6c-492f-9b14-fd8e3a2a991b",
                "repositoryId":42,"branch":"release/v1"
            }),
            _ => args.get(3).map_or_else(
                || json!({"query":"rate limit"}),
                |id| json!({"query":"rate limit","requestId":id}),
            ),
        };
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&payload)?,
            expected
        );
    }
    let body = if matches!(
        args[0],
        "pause-model" | "resume-model" | "discard-model-download" | "delete-model"
    ) {
        ""
    } else if method == "POST" && response.starts_with("503") {
        r#"{"code":"generation_not_configured","message":"Local generation is unavailable."}"#
    } else {
        r#"{"ready":true}"#
    };
    write!(
        incoming,
        "HTTP/1.1 {response}\r\nContent-Type: application/json\r\nX-Context-Trace-Id: {TRACE}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
