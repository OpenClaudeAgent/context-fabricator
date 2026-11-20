use std::error::Error;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use serde_json::json;

fn run(directory: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_ctxfab"))
        .env("CTXFAB_DATA_DIRECTORY", directory)
        .args(args)
        .output()
}

#[test]
fn cli_reports_bad_invocations_and_invalid_session_discovery() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    assert!(run(directory.path(), &["--help"])?.status.success());
    for args in [
        &["cancel", "not-a-uuid"][..],
        &["ask", "x", "--request-id", "invalid"][..],
        &["search", "x", "--repository-id", "42"][..],
        &["search", "x", "--repository-id", "no", "--branch", "main"][..],
        &["ask", "x", "--branch", "main", "--branch", "trunk"][..],
        &["ask", "x", "--mode", "hybrid"][..],
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
    let result = respond(&server, args, route, response);
    if result.is_err() {
        let _ = child.kill();
    }
    let output = child.wait_with_output()?;
    result?;
    assert_eq!(output.status.success(), success);
    if success {
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&output.stdout)?,
            json!({"ready": true})
        );
    } else if route == "/v1/answers" {
        assert!(String::from_utf8_lossy(&output.stderr).contains("generation is unavailable"));
    }
    Ok(())
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
    let method = match args.first().copied() {
        Some("ask" | "install-model") => "POST",
        Some("use-model") => "PUT",
        Some("cancel") => "DELETE",
        _ => "GET",
    };
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
    if matches!(method, "POST" | "PUT") {
        let mut payload = vec![0; length];
        reader.read_exact(&mut payload)?;
        let expected = match args[0] {
            "use-model" | "install-model" => json!({"profile":args[1]}),
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
    let body = if method == "POST" && response.starts_with("503") {
        r#"{"code":"generation_not_configured","message":"Local generation is unavailable."}"#
    } else {
        r#"{"ready":true}"#
    };
    write!(
        incoming,
        "HTTP/1.1 {response}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
