use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use serde::Deserialize;
use ureq::http::Uri;
mod command;
mod setup;

use command::{Command, Maintenance, ParseError, QueryOptions, RuntimeCommand, USAGE};

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
    let arguments = words.strip_prefix(&["--trace"]);
    let operation = match Command::parse(arguments.unwrap_or(&words)) {
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
    match call(operation, arguments.is_some()) {
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

#[cfg(target_os = "linux")]
fn default_data_directory() -> Option<PathBuf> {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    data_home.map(|path| path.join("context-fabricator"))
}

#[cfg(not(target_os = "linux"))]
fn default_data_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Application Support/ContextFabricator"))
}

fn discover_session() -> Result<Endpoint, RequestFailure> {
    let directory = std::env::var_os("CTXFAB_DATA_DIRECTORY")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(default_data_directory)
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

fn call(command: Command<'_>, show_trace: bool) -> Result<serde_json::Value, RequestFailure> {
    let endpoint = discover_session()?;
    let agent = loopback_agent(match &command {
        Command::Read(_) => 3,
        Command::Search(_, options) if matches!(options.mode, None | Some("bm25")) => 3,
        Command::Search(..) => 60,
        _ => 90,
    });
    let report_problem = !matches!(command, Command::Read(_));
    let actions_summary = matches!(command, Command::GitHubActions(_));
    let response_limit = response_limit(&command);
    let acknowledgement = empty_acknowledgement(&command);
    let bearer = format!("Bearer {}", endpoint.token);
    let mut response = match command {
        Command::Ask(query, options) => agent
            .post(format!("{}/v1/answers", endpoint.url))
            .header("Authorization", &bearer)
            .send_json(answer_payload(query, &options)),
        Command::Select(profile) => agent
            .put(format!("{}/v1/generation/models", endpoint.url))
            .header("Authorization", &bearer)
            .send_json(serde_json::json!({"profile":profile})),
        Command::Install(profile) => agent
            .post(format!("{}/v1/model-downloads", endpoint.url))
            .header("Authorization", &bearer)
            .send_json(serde_json::json!({"profile":profile})),
        Command::Runtime(operation) => Ok(runtime_request(&agent, &endpoint, &bearer, &operation)?),
        Command::Setup(operation) => {
            Ok(setup::request(&agent, &endpoint.url, &bearer, &operation)?)
        }
        Command::Maintenance(operation) => maintain(&agent, &endpoint.url, &bearer, &operation),
        Command::ControlDownload(profile, action) => agent
            .post(format!(
                "{}/v1/model-downloads/{profile}/{action}",
                endpoint.url
            ))
            .header("Authorization", &bearer)
            .send_empty(),
        Command::DiscardDownload(profile) => agent
            .delete(format!(
                "{}/v1/model-downloads/{profile}/partial",
                endpoint.url
            ))
            .header("Authorization", &bearer)
            .call(),
        Command::Cancel(id) => agent
            .delete(format!("{}/v1/answers/{id}", endpoint.url))
            .header("Authorization", &bearer)
            .call(),
        Command::GitHubGraph { id, branch, lookup } => {
            let mut request = agent
                .get(format!(
                    "{}/v1/github/repositories/{id}/graph",
                    endpoint.url
                ))
                .header("Authorization", &bearer)
                .query("branch", branch);
            if let Some((root, symbol)) = lookup {
                request = request.query("root", root).query("symbol", symbol);
            }
            request.call()
        }
        Command::GitHubActions(id) => agent
            .get(format!(
                "{}/v1/github/repositories/{id}/actions",
                endpoint.url
            ))
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
    if show_trace {
        report_trace(response.headers());
    }
    read_response(
        &mut response,
        report_problem,
        response_limit,
        actions_summary,
        acknowledgement,
    )
}

fn report_trace(headers: &ureq::http::HeaderMap) {
    if let Some(id) = headers
        .get("x-context-trace-id")
        .and_then(|value| value.to_str().ok())
        && id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        eprintln!("ctxfab: trace {id}");
    }
}

fn runtime_request(
    agent: &ureq::Agent,
    endpoint: &Endpoint,
    bearer: &str,
    operation: &RuntimeCommand<'_>,
) -> Result<ureq::http::Response<ureq::Body>, RequestFailure> {
    match operation {
        RuntimeCommand::Download => agent
            .post(format!("{}/v1/runtime/download", endpoint.url))
            .header("Authorization", bearer)
            .send_empty(),
        RuntimeCommand::Install(path) => {
            let archive = std::path::absolute(path)
                .map_err(|_| RequestFailure::InvalidSession("could not resolve the ZIP path"))?;
            agent
                .post(format!("{}/v1/runtime/install", endpoint.url))
                .header("Authorization", bearer)
                .send_json(serde_json::json!({"archivePath":archive}))
        }
    }
    .map_err(|_| RequestFailure::Unavailable("the local backend could not serve the request"))
}

fn maintain(
    agent: &ureq::Agent,
    origin: &str,
    bearer: &str,
    operation: &Maintenance<'_>,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    match operation {
        Maintenance::SetIndex(enabled) => agent
            .put(format!("{origin}/v1/retrieval/index"))
            .header("Authorization", bearer)
            .send_json(serde_json::json!({"enabled":enabled})),
        Maintenance::DeleteIndex => agent
            .delete(format!("{origin}/v1/retrieval/index"))
            .header("Authorization", bearer)
            .call(),
        Maintenance::ModelPlan(profile) => agent
            .get(format!(
                "{origin}/v1/model-downloads/{profile}/removal-plan"
            ))
            .header("Authorization", bearer)
            .call(),
        Maintenance::DeleteModel(profile, identity) => agent
            .delete(format!("{origin}/v1/model-downloads/{profile}"))
            .header("Authorization", bearer)
            .force_send_body()
            .send_json(serde_json::json!({"identity":identity})),
    }
}

fn response_limit(command: &Command<'_>) -> u64 {
    // Code results may include long source paths and their percent-encoded permalinks.
    match command {
        Command::GitHubActions(_) | Command::GitHubGraph { .. } => 2 * 1024 * 1024,
        Command::Search(..) | Command::Ask(..) => 512 * 1024,
        _ => 64 * 1024,
    }
}

fn empty_acknowledgement(command: &Command<'_>) -> Option<(u16, &'static str)> {
    match command {
        Command::ControlDownload(..) => Some((202, "accepted")),
        Command::DiscardDownload(_) => Some((204, "discarded")),
        Command::Maintenance(Maintenance::DeleteModel(..)) => Some((204, "deleted")),
        _ => None,
    }
}

fn loopback_agent(timeout: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent("ctxfab")
        .proxy(None)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(timeout)))
        .build()
        .into()
}

fn answer_payload(query: &str, options: &QueryOptions<'_>) -> serde_json::Value {
    let mut payload = serde_json::json!({"query":query});
    if let Some(id) = options.request_id {
        payload["requestId"] = serde_json::json!(id.to_string());
    }
    if let (Some(repository_id), Some(branch)) = (options.repository_id, options.branch) {
        payload["repositoryId"] = serde_json::json!(repository_id);
        payload["branch"] = serde_json::json!(branch);
    }
    if let Some(commit) = options.history_commit {
        payload["historyCommit"] = serde_json::json!(commit);
    }
    if let (Some(run), Some(attempt)) = (options.actions_run_id, options.actions_run_attempt) {
        payload["actionsRunId"] = serde_json::json!(run);
        payload["actionsRunAttempt"] = serde_json::json!(attempt);
    }
    payload
}

fn read_response(
    response: &mut ureq::http::Response<ureq::Body>,
    report_problem: bool,
    response_limit: u64,
    actions_summary: bool,
    acknowledgement: Option<(u16, &'static str)>,
) -> Result<serde_json::Value, RequestFailure> {
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
    if let Some((status, key)) = acknowledgement
        && response.status() == status
    {
        return Ok(serde_json::json!({key: true}));
    }
    let mut result: serde_json::Value = response
        .body_mut()
        .with_config()
        .limit(response_limit)
        .read_json()
        .map_err(|_| RequestFailure::InvalidResponse)?;
    if actions_summary {
        summarize_actions(&mut result);
    }
    Ok(result)
}

fn summarize_actions(result: &mut serde_json::Value) {
    if let Some(workflows) = result
        .get_mut("snapshot")
        .and_then(|s| s.get_mut("workflows"))
        .and_then(serde_json::Value::as_array_mut)
    {
        for workflow in workflows {
            if let Some(object) = workflow.as_object_mut() {
                let present = object
                    .remove("definition")
                    .is_some_and(|value| value.is_string());
                object.insert("hasDefinition".into(), present.into());
            }
        }
    }
}

#[cfg(test)]
mod actions_tests {
    #[test]
    fn summary_hides_definition_bytes_without_hiding_provenance() {
        let mut value = serde_json::json!({"snapshot":{"workflows":[
            {"name":"CI", "definition":"sensitive source text", "definitionBlob":"abc"},
            {"name":"unavailable", "definition":null, "definitionBlob":null}
        ], "runs":[{"id":42,"attempt":2}]}});
        super::summarize_actions(&mut value);
        assert_eq!(
            value["snapshot"]["workflows"][0],
            serde_json::json!({"name":"CI", "definitionBlob":"abc", "hasDefinition":true})
        );
        assert_eq!(
            value["snapshot"]["workflows"][1],
            serde_json::json!({"name":"unavailable", "definitionBlob":null, "hasDefinition":false})
        );
        assert_eq!(value["snapshot"]["runs"][0]["attempt"], 2);
    }
}
