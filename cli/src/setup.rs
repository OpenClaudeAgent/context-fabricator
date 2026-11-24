use std::io::Read;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{RequestFailure, command::SetupCommand};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewedPlan {
    selection: serde_json::Value,
    fingerprint: String,
    can_start: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestSelection<'a> {
    runtime_source: &'a str,
    embedding: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    answer_profile: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Confirmation {
    selection: serde_json::Value,
    fingerprint: String,
    operation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    archive_path: Option<PathBuf>,
}

pub(super) fn request(
    agent: &ureq::Agent,
    origin: &str,
    bearer: &str,
    operation: &SetupCommand<'_>,
) -> Result<ureq::http::Response<ureq::Body>, RequestFailure> {
    let route = format!("{origin}/v1/inference/setup");
    match *operation {
        SetupCommand::Status => agent.get(route).header("Authorization", bearer).call(),
        SetupCommand::Archive { id, path } => agent.post(format!("{route}/archive"))
            .header("Authorization", bearer)
            .send_json(serde_json::json!({"operationId": id.to_string(), "archivePath": absolute_archive(path)?})),
        SetupCommand::CancelArchive(id) => agent.delete(format!("{route}/archive/{id}"))
            .header("Authorization", bearer).call(),
        SetupCommand::Plan {
            source,
            embedding,
            profile,
        } => agent
            .post(format!("{route}/plan"))
            .header("Authorization", bearer)
            .send_json(RequestSelection {
                runtime_source: source,
                embedding,
                answer_profile: profile,
            }),
        SetupCommand::Start { plan, id, archive } => agent
            .post(route)
            .header("Authorization", bearer)
            .send_json(confirm(plan, id, archive)?),
    }
    .map_err(|_| RequestFailure::Unavailable("the local backend could not serve the request"))
}

fn confirm(plan: &str, id: Uuid, archive: Option<&str>) -> Result<Confirmation, RequestFailure> {
    let file = std::fs::File::open(plan)
        .map_err(|_| RequestFailure::InvalidSession("could not open the reviewed setup plan"))?;
    let reviewed: ReviewedPlan = serde_json::from_reader(file.take(64 * 1024))
        .map_err(|_| RequestFailure::InvalidSession("the reviewed setup plan could not be read"))?;
    if !reviewed.can_start {
        return Err(RequestFailure::InvalidSession(
            "the reviewed plan cannot start",
        ));
    }
    let archive_path = archive.map(absolute_archive).transpose()?;
    Ok(Confirmation {
        selection: reviewed.selection,
        fingerprint: reviewed.fingerprint,
        operation_id: id.to_string(),
        archive_path,
    })
}

fn absolute_archive(path: &str) -> Result<PathBuf, RequestFailure> {
    std::path::absolute(path)
        .map_err(|_| RequestFailure::InvalidSession("could not resolve the ZIP path"))
}
