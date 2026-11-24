use super::*;

const HISTORY: &str = "/v1/answer-history";
const SETTINGS: &str = "/v1/answer-history/settings";
const QUERY: &str = "ArchivePrivacyMarker current outbound retry cooldown";
const UNSUPPORTED: &str = "Fabricate current outbound retry cooldown";

#[test]
fn history_is_opt_in_and_serves_the_saved_answer_after_restart() -> Result<(), TestError> {
    let work = tempfile::tempdir()?;
    let directory = work.path().join("data");
    seed(&directory, &FIXTURE[..1])?;
    stage_worker_fixture(&directory)?;
    let executable = Path::new(env!("CARGO_BIN_EXE_ctxfab-server"));
    let mut service = Service::start(executable, &directory)?;

    let initial = service.request(SETTINGS, None)?;
    assert_eq!(
        initial,
        (
            200,
            json!({"revision":0,"enabled":false,"retentionDays":30})
        )
    );
    let (unrecorded, trace) = service.answer_with_trace(&json!({"query":QUESTION}))?;
    accepted(&unrecorded, 0, "47 seconds");
    let unrecorded_path = history_trace_path(&trace)?;
    assert_eq!(
        service.request(HISTORY, None)?,
        (200, json!({"entries":[]}))
    );
    let updated = enable_fixture_history(&service)?;

    let input = json!({"query":QUERY, "includeModelContext": true});
    let (answer, trace) = service.answer_with_trace(&input)?;
    accepted(&answer, 0, "47 seconds");
    assert_eq!(answer.1["modelContext"]["question"], QUERY);
    let path = history_trace_path(&trace)?;
    let (repeated, trace) = service.answer_with_trace(&input)?;
    assert_eq!(repeated, answer);
    let repeated_path = history_trace_path(&trace)?;
    assert_ne!(path, repeated_path);
    let (withheld, withheld_trace) = service.answer_with_trace(&json!({
        "query": UNSUPPORTED, "includeModelContext": true
    }))?;
    insufficient(&withheld);
    assert_eq!(
        withheld.1["evidenceReview"]["reason"],
        "citationsUnverified"
    );
    let withheld_path = history_trace_path(&withheld_trace)?;
    let page = service.request(HISTORY, None)?;
    assert_eq!(page.0, 200);
    let rows = page.1["entries"].as_array().ok_or("history entries")?;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["question"], UNSUPPORTED);
    assert_eq!(rows[0]["outcome"], "insufficientEvidence");
    assert_eq!(rows[1]["question"], QUERY);
    assert_eq!(rows[1]["outcome"], "answered");
    let repeated_id = rows[1]["id"].as_i64().ok_or("repeated answer id")?;
    let id = rows[2]["id"].as_i64().ok_or("answer id")?;
    let detail = format!("{HISTORY}/{id}");
    service.stop()?;

    let mut reopened = Service::start(executable, &directory)?;
    assert_eq!(reopened.request(SETTINGS, None)?, updated);
    let mut archived_answer = answer.1;
    let object = archived_answer.as_object_mut().ok_or("answer object")?;
    assert!(object.remove("modelContext").is_some());
    assert_saved_answer(&reopened, &path, &detail, &archived_answer)?;
    assert_eq!(reopened.request(&repeated_path, None)?.1["id"], repeated_id);
    let (status, saved_withheld) = reopened.request(&withheld_path, None)?;
    assert_eq!(status, 200);
    assert_eq!(saved_withheld["question"], UNSUPPORTED);
    assert!(saved_withheld["answer"].get("evidenceReview").is_none());
    assert_eq!(reopened.request(&unrecorded_path, None)?.0, 404);
    assert_eq!(
        reopened
            .request(&format!("{HISTORY}/by-trace/INVALID"), None)?
            .0,
        400
    );
    let telemetry = reopened.request("/v1/telemetry", None)?;
    assert_eq!(telemetry.0, 200);
    assert!(!telemetry.1.to_string().contains("ArchivePrivacyMarker"));
    let cleared = reopened
        .http
        .delete(format!("{}{}", reopened.address, HISTORY))
        .header("Authorization", &reopened.token)
        .call()?;
    assert_eq!(cleared.status().as_u16(), 204);
    assert_eq!(reopened.request(&detail, None)?.0, 404);
    assert_eq!(reopened.request(&path, None)?.0, 404);
    assert_eq!(reopened.request(&repeated_path, None)?.0, 404);
    assert_eq!(
        reopened.request(HISTORY, None)?,
        (200, json!({"entries":[]}))
    );
    reopened.stop()?;
    assert!(
        !std::fs::read(directory.join("telemetry.sqlite3"))?
            .windows("ArchivePrivacyMarker".len())
            .any(|bytes| bytes == b"ArchivePrivacyMarker")
    );
    Ok(())
}

fn enable_fixture_history(service: &Service) -> Result<(u16, Value), TestError> {
    assert_eq!(
        service
            .call(
                "PUT",
                SETTINGS,
                Some(json!({"revision":0,"enabled":true,"retentionDays":0}))
            )?
            .0,
        400
    );
    let input = json!({"revision":0,"enabled":true,"retentionDays":7});
    let updated = service.call("PUT", SETTINGS, Some(input.clone()))?;
    assert_eq!(
        updated,
        (200, json!({"revision":1,"enabled":true,"retentionDays":7}))
    );
    assert_eq!(service.call("PUT", SETTINGS, Some(input))?.0, 409);
    Ok(updated)
}

fn assert_saved_answer(
    service: &Service,
    path: &str,
    detail: &str,
    answer: &Value,
) -> Result<(), TestError> {
    let url = format!("{}{path}", service.address);
    assert_eq!(service.http.get(&url).call()?.status().as_u16(), 401);
    let response = service
        .http
        .get(&url)
        .header("Authorization", &service.token)
        .call()?;
    assert_eq!(
        response
            .headers()
            .get("cache-control")
            .ok_or("cache-control")?,
        "no-store"
    );
    let by_trace = decode_response(response)?;
    assert_eq!(by_trace.0, 200);
    assert_eq!(by_trace, service.request(detail, None)?);
    let saved = by_trace.1;
    assert_eq!(saved["question"], QUERY);
    assert_eq!(saved["scope"], json!({}));
    assert_eq!(&saved["answer"], answer);
    assert_eq!(saved["traceId"], path.rsplit('/').next().ok_or("path id")?);
    assert_eq!(
        saved["expiresAtMs"].as_i64(),
        Some(saved["completedAtMs"].as_i64().ok_or("completion time")? + 7 * 86_400_000 + 1)
    );
    Ok(())
}

pub(super) fn history_trace_path(trace: &Value) -> Result<String, TestError> {
    let spans = trace["spans"].as_array().ok_or("trace detail")?;
    let id = spans
        .iter()
        .find(|span| span["name"] == "api_answer")
        .and_then(|span| span["traceId"].as_str())
        .ok_or("answer trace id")?;
    Ok(format!("{HISTORY}/by-trace/{id}"))
}
