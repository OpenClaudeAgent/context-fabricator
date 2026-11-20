use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use serde::Deserialize;
use ureq::http::Uri;
mod command;

use command::{Command, ParseError, USAGE};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Endpoint {
    api_version: u8,
    url: String,
    token: String,
}

enum RequestFailure {
    Unavailable(&'static str),
    Incompatible,
    InvalidSession(&'static str),
    InvalidResponse,
    Rejected(String),
}

impl RequestFailure {
    fn report(&self) -> ExitCode {
        let (status, message) = match *self {
            Self::Unavailable(message) => (3, message),
            Self::Incompatible => (4, "the running backend uses an incompatible API version"),
            Self::InvalidSession(message) => (1, message),
            Self::InvalidResponse => (1, "the local backend returned an unexpected response"),
            Self::Rejected(ref message) => (1, message.as_str()),
        };
        eprintln!("ctxfab: {message}");
        ExitCode::from(status)
    }
}

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let words: Vec<_> = arguments.iter().map(String::as_str).collect();
    let operation = match Command::parse(&words) {
        Ok(Command::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(operation) => operation,
        Err(ParseError::RequestId) => return invalid_request_id(),
        Err(ParseError::Usage) => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match call(operation) {
        Ok(value) => match serde_json::to_writer_pretty(io::stdout().lock(), &value) {
            Ok(()) => {
                println!();
                ExitCode::SUCCESS
            }
            Err(_) => ExitCode::FAILURE,
        },
        Err(error) => error.report(),
    }
}

fn invalid_request_id() -> ExitCode {
    eprintln!("ctxfab: supply a UUID for the answer request");
    ExitCode::from(2)
}

fn discover_session() -> Result<Endpoint, RequestFailure> {
    let directory = std::env::var_os("CTXFAB_DATA_DIRECTORY")
        .map_or_else(
            || {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join("Library/Application Support/ContextFabricator"))
            },
            |directory| Some(PathBuf::from(directory)),
        )
        .ok_or(RequestFailure::Unavailable(
            "could not locate the local application data directory",
        ))?;
    let metadata = std::fs::read(directory.join("endpoint.json")).map_err(|_| {
        RequestFailure::Unavailable("launch Context Fabricator to start a local backend session")
    })?;
    let endpoint: Endpoint = serde_json::from_slice(&metadata).map_err(|_| {
        RequestFailure::InvalidSession("the local endpoint metadata could not be read")
    })?;
    if endpoint.api_version != 1 {
        return Err(RequestFailure::Incompatible);
    }
    let address: Uri = endpoint
        .url
        .parse()
        .map_err(|_| RequestFailure::InvalidSession("the session address is not valid"))?;
    if address.scheme_str() != Some("http")
        || address.host() != Some("127.0.0.1")
        || address.port_u16().is_none()
        || address.path() != "/"
        || address.query().is_some()
    {
        return Err(RequestFailure::InvalidSession(
            "the local endpoint is not a loopback session",
        ));
    }

    Ok(endpoint)
}

fn call(command: Command<'_>) -> Result<serde_json::Value, RequestFailure> {
    let endpoint = discover_session()?;
    let timeout = match &command {
        Command::Read(_) => 3,
        Command::Search(_, options) if matches!(options.mode, None | Some("bm25")) => 3,
        Command::Search(..) => 60,
        _ => 90,
    };
    let report_problem = !matches!(command, Command::Read(_));
    // Code results can carry twenty long paths in several fields, including
    // their percent-encoded permalinks.
    let response_limit = if matches!(command, Command::Search(..) | Command::Ask(..)) {
        512 * 1024
    } else {
        64 * 1024
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(timeout)))
        .build()
        .into();
    let bearer = format!("Bearer {}", endpoint.token);
    let mut response = match command {
        Command::Ask(query, options) => {
            let mut payload = serde_json::json!({"query":query});
            if let Some(id) = options.request_id {
                payload["requestId"] = serde_json::json!(id.to_string());
            }
            if let (Some(repository_id), Some(branch)) = (options.repository_id, options.branch) {
                payload["repositoryId"] = serde_json::json!(repository_id);
                payload["branch"] = serde_json::json!(branch);
            }
            agent
                .post(format!("{}/v1/answers", endpoint.url))
                .header("Authorization", &bearer)
                .send_json(payload)
        }
        Command::Select(profile) => agent
            .put(format!("{}/v1/generation/models", endpoint.url))
            .header("Authorization", &bearer)
            .send_json(serde_json::json!({"profile":profile})),
        Command::Install(profile) => agent
            .post(format!("{}/v1/model-downloads", endpoint.url))
            .header("Authorization", &bearer)
            .send_json(serde_json::json!({"profile":profile})),
        Command::Cancel(id) => agent
            .delete(format!("{}/v1/answers/{id}", endpoint.url))
            .header("Authorization", &bearer)
            .call(),
        Command::Read(path) => agent
            .get(format!("{}{path}", endpoint.url))
            .header("Authorization", &bearer)
            .call(),
        Command::Search(query, options) => {
            let mut request = agent
                .get(format!("{}/v1/search", endpoint.url))
                .header("Authorization", &bearer)
                .query("query", query);
            if let Some(mode) = options.mode {
                request = request.query("mode", mode);
            }
            if let (Some(repository_id), Some(branch)) = (options.repository_id, options.branch) {
                request = request
                    .query("repositoryId", repository_id.to_string())
                    .query("branch", branch);
            }
            request.call()
        }
        Command::Help => unreachable!("help does not call the local backend"),
    }
    .map_err(|_| RequestFailure::Unavailable("the local backend could not serve the request"))?;
    if !response.status().is_success() {
        if report_problem {
            let problem = response
                .body_mut()
                .with_config()
                .limit(4096)
                .read_json::<serde_json::Value>()
                .map_err(|_| RequestFailure::InvalidResponse)?;
            let message = problem["message"]
                .as_str()
                .ok_or(RequestFailure::InvalidResponse)?;
            return Err(RequestFailure::Rejected(message.to_owned()));
        }
        return Err(RequestFailure::InvalidResponse);
    }
    response
        .body_mut()
        .with_config()
        .limit(response_limit)
        .read_json()
        .map_err(|_| RequestFailure::InvalidResponse)
}
