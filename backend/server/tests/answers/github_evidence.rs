use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::process::{Child, Command};
use std::thread;

use serde_json::{Value, json};

use super::{
    EMBEDDING_FILE, GENERATION_FILE, Service, TestError, generation_budget, span_events,
    stage_worker_fixture, wait,
};

const REPOSITORIES: &str = "/v1/github/repositories";
const HISTORY: &str = "/v1/github/repositories/41/history";
const ACTIONS: &str = "/v1/github/repositories/41/actions";
const JOURNAL: &str = "/v1/answer-history";

struct Origin(Child);

impl Drop for Origin {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn configured(revision: u32, actions: bool) -> Value {
    json!({
        "revision": revision, "repository": "fixture/alpha", "expectedId": 41,
        "branch": "main", "intervalMinutes": 60,
        "features": {"currentCode": false, "history": true, "actions": actions, "codeGraph": false}
    })
}

fn history_answer(service: &Service, child: &str, query: &str) -> Result<Value, TestError> {
    let (status, answer) = service.request(
        "/v1/answers",
        Some(json!({
            "query": query, "repositoryId": 41, "branch": "main", "historyCommit": child
        })),
    )?;
    assert_eq!(status, 200, "{answer}");
    Ok(answer)
}

fn actions_answer(service: &Service, query: &str, attempt: u32) -> Result<Value, TestError> {
    let (status, answer) = service.request(
        "/v1/answers",
        Some(json!({
            "query": query, "repositoryId": 41, "branch": "main", "actionsRunId": 30,
            "actionsRunAttempt": attempt
        })),
    )?;
    assert_eq!(status, 200, "{answer}");
    Ok(answer)
}

fn assert_abstained(answer: &Value) {
    assert_eq!(answer["outcome"], "insufficientEvidence");
    assert!(answer.get("text").is_none());
    assert_eq!(answer["citations"], json!([]));
    assert_eq!(answer["evidenceRepositoryIds"], json!([]));
}

fn collect_and_check_history(service: &Service) -> Result<(), TestError> {
    let parent = "a".repeat(40);
    let child = "b".repeat(40);
    assert_eq!(service.request(HISTORY, Some(Value::Null))?.0, 200);
    assert_abstained(&history_answer(service, &child, "What changed?")?);
    let route = format!("{HISTORY}/comparisons/{child}");
    assert_eq!(service.request(&route, Some(Value::Null))?.0, 200);
    let ((status, result), trace) = service.answer_with_trace(&json!({
        "query": "What changed?", "repositoryId": 41, "branch": "main", "historyCommit": child,
        "includeModelContext": true
    }))?;
    assert_eq!(status, 200, "{result}");
    let budget = admitted_context(&trace, "historyPairs")?;
    assert_eq!(budget["admitted"], budget["proposed"]);
    assert!(budget["admitted"].as_u64().is_some_and(|pairs| pairs > 0));
    assert_eq!(budget["measurements"], 1);
    assert!(!trace.to_string().contains("src/modified.rs"));
    assert_eq!(result["outcome"], "answered");
    assert_eq!(result["text"], "Changed from repeat to changed.");
    let context = &result["modelContext"];
    assert_eq!(context["question"], "What changed?");
    let first = &context["sources"][0];
    assert_eq!(first["kind"], "history");
    assert_eq!(first["slot"], 1);
    for (part, (side, pin, quote)) in first["parts"]
        .as_array()
        .ok_or("comparison prompt parts")?
        .iter()
        .zip([("before", &parent, "repeat"), ("after", &child, "changed")])
    {
        assert_eq!(part["side"], side);
        assert_eq!(part["revision"].as_str(), Some(pin.as_str()));
        assert_eq!(part["path"], "src/modified.rs");
        assert!(
            part["text"]
                .as_str()
                .is_some_and(|text| text.contains(quote)),
            "{part}"
        );
    }
    assert_eq!(result["evidenceRepositoryIds"], json!([41]));
    let citations = result["citations"].as_array().ok_or("history citations")?;
    assert_eq!(citations.len(), 2);
    for (citation, (side, revision, quote)) in citations.iter().zip([
        ("before", parent.as_str(), "repeat"),
        ("after", child.as_str(), "changed"),
    ]) {
        assert_eq!(citation["comparison"]["side"], side);
        assert_eq!(citation["comparison"]["parent"], parent);
        assert_eq!(citation["comparison"]["child"], child);
        assert_eq!(citation["evidence"]["revision"], revision);
        assert_eq!(citation["evidence"]["location"]["startLine"], 2);
        assert_eq!(citation["evidence"]["access"], "available");
        assert_eq!(citation["quote"], quote);
        assert_eq!(
            citation["lineUrl"],
            format!("https://github.com/fixture/alpha/blob/{revision}/src/modified.rs#L2")
        );
    }
    assert_abstained(&history_answer(service, &child, "Fabricate changed line")?);
    assert_abstained(&history_answer(
        service,
        &child,
        "Unsupported changed line",
    )?);
    Ok(())
}

fn collect_and_check_actions(service: &Service, origin: &Path) -> Result<(), TestError> {
    fs::remove_file(origin.join("advance-code"))?;
    fs::write(origin.join("actions-fixture"), [])?;
    fs::write(origin.join("actions-completed"), [])?;
    assert_eq!(
        service
            .call("PUT", REPOSITORIES, Some(configured(2, true)))?
            .0,
        200
    );
    assert_eq!(service.request(ACTIONS, Some(Value::Null))?.0, 200);
    let query = "What was the conclusion?";
    let ((status, result), trace) = service.answer_with_trace(&json!({
        "query": query, "repositoryId": 41, "branch": "main", "actionsRunId": 30,
        "actionsRunAttempt": 1, "includeModelContext": true
    }))?;
    assert_eq!(status, 200, "{result}");
    assert_eq!(
        admitted_context(&trace, "workflowAttempt")?,
        &json!({
            "kind": "workflowAttempt", "proposed": 1, "admitted": 1,
            "inputTokens": 1024, "inputTokenLimit": 3455, "measurements": 1
        })
    );
    assert!(!trace.to_string().contains(query));
    assert_eq!(result["outcome"], "answered");
    assert_eq!(result["mode"], "selected");
    let source = &result["modelContext"]["sources"][0];
    assert_eq!(source["kind"], "workflow");
    let text = source["parts"][0]["text"]
        .as_str()
        .ok_or("workflow prompt")?;
    assert!(text.lines().any(|line| line == "Run ID: 30"));
    assert!(text.lines().any(|line| line == "Attempt: 1"));
    assert!(text.lines().any(|line| line == "Conclusion: success"));
    assert_eq!(result["evidenceRepositoryIds"], json!([41]));
    assert_eq!(result["citations"][0]["quote"], "Conclusion: success");
    assert_eq!(result["citations"][0]["workflowRun"]["runId"], 30);
    assert_eq!(result["citations"][0]["workflowRun"]["attempt"], 1);
    assert!(result["citations"][0].get("lineUrl").is_none());
    assert_eq!(
        result["citations"][0]["workflowRun"]["reportedHead"],
        "a".repeat(40)
    );
    assert_abstained(&actions_answer(service, query, 2)?);
    assert_abstained(&actions_answer(service, "Fabricate conclusion", 1)?);
    let ((status, result), trace) = service.answer_with_trace(&json!({
        "query": "What triggered Actions run 30 and what was its conclusion?",
        "repositoryId": 41,
        "branch": "main"
    }))?;
    assert_eq!(status, 200);
    assert_eq!(result["outcome"], "answered", "{result}");
    let citations = result["citations"].as_array().ok_or("workflow fields")?;
    assert_eq!(
        citations
            .iter()
            .map(|citation| &citation["quote"])
            .collect::<Vec<_>>(),
        ["Event: push", "Status: completed", "Conclusion: success"],
    );
    assert!(citations.iter().all(|citation| {
        citation["workflowRun"]["runUrl"]
            == "https://github.com/fixture/alpha/actions/runs/30/attempts/1"
    }));
    assert!(trace.to_string().contains("Citations verified"));
    Ok(())
}

fn collect_purge_repositories(service: &Service, origin: &Path) -> Result<(), TestError> {
    fs::remove_file(origin.join("actions-fixture"))?;
    fs::write(origin.join("purge-answers"), [])?;
    let mut alpha = configured(3, true);
    alpha["features"]["currentCode"] = json!(true);
    assert_eq!(service.call("PUT", REPOSITORIES, Some(alpha))?.0, 200);
    let beta = json!({
        "revision": 4, "repository": "fixture/beta", "expectedId": 42,
        "branch": "trunk", "intervalMinutes": 60,
        "features": {"currentCode": true, "history": false, "actions": false, "codeGraph": false}
    });
    assert_eq!(service.call("PUT", REPOSITORIES, Some(beta))?.0, 200);
    for id in [41, 42] {
        let refresh = format!("{REPOSITORIES}/{id}/refresh");
        let (status, result) = service.request(&refresh, Some(Value::Null))?;
        assert_eq!(status, 200, "{result}");
    }
    Ok(())
}

fn admitted_context<'a>(trace: &'a Value, kind: &str) -> Result<&'a Value, TestError> {
    let generation = trace["spans"]
        .as_array()
        .and_then(|spans| {
            spans.iter().find(|span| {
                span["name"] == "answer_generation" && span["contextBudget"]["kind"] == kind
            })
        })
        .ok_or("missing completion context")?;
    assert_eq!(generation["level"], "ok");
    Ok(&generation["contextBudget"])
}

fn check_purge_during_answer(service: &Service, model: &Path) -> Result<(), TestError> {
    use super::answer_history::history_trace_path;

    let probe = "purgeguardprobe";
    let retained = enable_history_and_capture_beta(service)?;
    let ((_, before), trace) = service.answer_with_trace(&json!({"query":probe}))?;
    let trace_path = history_trace_path(&trace)?;
    assert_eq!(service.request(&trace_path, None)?.1["answer"], before);
    assert_eq!(before["outcome"], "answered");
    assert_eq!(
        before["citations"][0]["evidence"]["source"],
        "github:42:old.rs"
    );
    assert_eq!(before["evidenceRepositoryIds"], json!([41, 42]));
    let (status, page) = service.request(JOURNAL, None)?;
    assert_eq!(status, 200);
    let id = page["entries"][0]["id"]
        .as_i64()
        .ok_or("mixed history ID")?;
    let mixed = format!("{JOURNAL}/{id}");
    assert_eq!(page["entries"][0]["question"], probe);
    assert_eq!(service.request(&mixed, None)?.1["answer"], before);
    let (_, hits) = service.request("/v1/search?query=purgeguardprobe", None)?;
    let hits = hits.as_array().ok_or("both repository hits")?;
    assert!(
        hits.iter()
            .any(|hit| hit["source"] == "github:41:src/purge.rs")
    );
    assert!(hits.iter().any(|hit| hit["source"] == "github:42:old.rs"));
    let (_, budget) = service.request("/v1/github/storage", None)?;
    let bytes = budget["bytes"]
        .as_u64()
        .ok_or("storage bytes before purge")?;

    let gate = model.with_extension("response-gate");
    fs::write(&gate, [])?;
    thread::scope(|scope| -> Result<(), TestError> {
        let in_flight =
            scope.spawn(|| service.answer_with_trace(&json!({"query":"Delay purgeguardprobe"})));
        wait(|| model.with_extension("in-flight").is_file())?;
        let deletion = service.call("DELETE", "/v1/github/repositories/41/data", None);
        fs::remove_file(gate)?;
        let (status, budget) = deletion?;
        assert_eq!(status, 200, "{budget}");
        let remaining = budget["bytes"].as_u64().ok_or("remaining storage bytes")?;
        assert!(remaining > 0 && remaining < bytes);
        let ((status, answer), trace) =
            in_flight.join().map_err(|_| "answer thread panicked")??;
        assert_eq!(status, 200, "{answer}");
        assert_abstained(&answer);
        assert_eq!(service.request(&history_trace_path(&trace)?, None)?.0, 404);
        Ok(())
    })?;

    assert_eq!(service.request(&mixed, None)?.0, 404);
    assert_eq!(service.request(&trace_path, None)?.0, 404);
    let (status, page) = service.request(JOURNAL, None)?;
    assert_eq!(status, 200);
    assert_eq!(page["entries"], json!([retained]));
    assert_eq!(
        service.request("/v1/github/repositories/41/data", None)?.1["paused"],
        true
    );
    let (_, hits) = service.request("/v1/search?query=purgeguardprobe", None)?;
    let hits = hits.as_array().ok_or("remaining repository hits")?;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["source"], "github:42:old.rs");
    let (_, answer) = service.answer(probe)?;
    assert_eq!(answer["outcome"], "answered");
    assert_eq!(
        answer["citations"][0]["evidence"]["source"],
        "github:42:old.rs"
    );
    assert_eq!(answer["evidenceRepositoryIds"], json!([42]));
    let child = "b".repeat(40);
    assert_abstained(&history_answer(service, &child, "What changed?")?);
    assert_abstained(&actions_answer(service, "What was the conclusion?", 1)?);
    Ok(())
}

fn enable_history_and_capture_beta(service: &Service) -> Result<Value, TestError> {
    let settings = format!("{JOURNAL}/settings");
    assert_eq!(
        service
            .call(
                "PUT",
                &settings,
                Some(json!({"revision":0,"enabled":true,"retentionDays":7}))
            )?
            .0,
        200
    );
    let question = "Beta purgeguardprobe";
    let (status, answer) = service.request(
        "/v1/answers",
        Some(json!({"query":question,"repositoryId":42,"branch":"trunk"})),
    )?;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["outcome"], "answered", "{answer}");
    assert_eq!(answer["evidenceRepositoryIds"], json!([42]));
    let (status, page) = service.request(JOURNAL, None)?;
    assert_eq!(status, 200);
    let entry = page["entries"][0].clone();
    assert_eq!(entry["question"], question);
    assert_eq!(entry["outcome"], "answered");
    Ok(entry)
}

fn history_fixture(
    flag: &str,
    linked: bool,
) -> Result<
    (
        tempfile::TempDir,
        Origin,
        std::path::PathBuf,
        std::path::PathBuf,
        Service,
    ),
    TestError,
> {
    let fixture = tempfile::tempdir()?;
    let origin = fixture.path().join("origin");
    fs::create_dir(&origin)?;
    for flag in ["collect-history", "history-modified", "advance-code"] {
        fs::write(origin.join(flag), [])?;
    }
    let origin_log = fs::File::create(origin.join("launch.log"))?;
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../quality/github/github_fixture.py");
    let provider = Origin(
        Command::new("python3")
            .arg(script)
            .arg(&origin)
            .stdout(origin_log.try_clone()?)
            .stderr(origin_log)
            .spawn()?,
    );
    let port_file = origin.join("port");
    wait(|| port_file.is_file())?;
    let port = fs::read_to_string(port_file)?;
    let data = fixture.path().join("data");
    DirBuilder::new().mode(0o700).create(&data)?;
    let model = stage_worker_fixture(&data)?.join(GENERATION_FILE);
    if linked {
        fs::write(origin.join("linked-answers"), [])?;
        fs::write(model.with_file_name(EMBEDDING_FILE), b"formulation fixture")?;
    }
    let backend = std::env::var_os("CTXFAB_TEST_ANSWERS_BACKEND").map_or_else(
        || std::path::PathBuf::from(env!("CARGO_BIN_EXE_ctxfab-server")),
        Into::into,
    );
    let service = Service::start_fixture(&backend, &data, Some((flag, port.trim())))?;
    let credential = json!({"revision": 0, "token": "github_pat_fixture_primary"});
    assert_eq!(
        service
            .call("PUT", "/v1/github/credential", Some(credential))?
            .0,
        200
    );
    assert_eq!(
        service
            .call("PUT", REPOSITORIES, Some(configured(1, false)))?
            .0,
        200
    );

    Ok((fixture, provider, origin, model, service))
}

fn stage_history_context(service: &Service, origin: &Path) -> Result<(), TestError> {
    fs::write(origin.join("history-context"), [])?;
    assert_eq!(service.request(HISTORY, Some(Value::Null))?.0, 200);
    let route = format!("{HISTORY}/comparisons/{}", "b".repeat(40));
    assert_eq!(service.request(&route, Some(Value::Null))?.0, 200);
    Ok(())
}

#[test]
fn history_context_reopens_unchanged_lines_for_selected_and_ordinary_questions()
-> Result<(), TestError> {
    let (_temp, _fixture, origin, model, mut service) = history_fixture("--github-fixture", false)?;
    stage_history_context(&service, &origin)?;
    let child = "b".repeat(40);
    let parent = "a".repeat(40);
    let quote = "    write_all(payload);";
    for selected in [true, false] {
        let mut input = json!({
            "query": "History context sender.rs payload writer, what is passed to write_all?",
            "includeModelContext": true,
        });
        if selected {
            input["repositoryId"] = json!(41);
            input["branch"] = json!("main");
            input["historyCommit"] = json!(child);
        }
        let ((status, answer), trace) = service.answer_with_trace(&input)?;
        assert_eq!(status, 200, "{answer}");
        assert_eq!(answer["outcome"], "answered", "{answer}");
        if !selected {
            assert!(
                trace
                    .to_string()
                    .contains("Citations could not be verified")
            );
            admitted_context(&trace, "passages")?;
        }
        admitted_context(&trace, "historyPairs")?;
        assert_eq!(
            answer["text"],
            format!(
                "The cited line is identical in both revisions.\n\n{}",
                quote.trim()
            )
        );
        assert!(
            !answer["text"]
                .as_str()
                .unwrap_or_default()
                .contains("Both revisions call")
        );
        let citations = answer["citations"].as_array().ok_or("paired citations")?;
        assert_eq!(citations.len(), 2);
        for (citation, (side, revision)) in citations
            .iter()
            .zip([("before", &parent), ("after", &child)])
        {
            assert_eq!(citation["comparison"]["side"], side);
            assert_eq!(citation["evidence"]["revision"], revision.as_str());
            assert_eq!(citation["quote"], quote);
            assert_eq!(citation["evidence"]["location"]["startLine"], 8);
            assert_eq!(
                citation["lineUrl"],
                format!("https://github.com/fixture/alpha/blob/{revision}/src/sender.rs#L8")
            );
        }
        let parts = answer["modelContext"]["sources"][0]["parts"]
            .as_array()
            .ok_or("admitted prompt")?;
        assert!(parts.iter().all(|part| {
            part["text"]
                .as_str()
                .is_some_and(|text| text.contains(quote))
        }));
        assert!(parts.iter().all(|part| {
            !part["text"]
                .as_str()
                .unwrap_or_default()
                .contains("elsewhere")
        }));
        let prompt: Value =
            serde_json::from_slice(&fs::read(model.with_extension("history-prompt.json"))?)?;
        for side in ["before", "after"] {
            let lines = prompt["changes"][0][side]["lines"]
                .as_array()
                .ok_or("side selectors")?;
            assert!(lines.iter().any(|line| line["line"] == 8
                && line["unchangedGroup"] == 1
                && line["text"] == quote));
            assert!(
                lines
                    .iter()
                    .any(|line| line["line"] == 7 && line.get("context").is_none())
            );
        }
        eprintln!(
            "history-context {}",
            json!({"request": input, "prompt": prompt, "answer": answer, "trace": trace})
        );
    }
    for query in [
        "History context outside sender.rs payload writer write_all",
        "History context twin sender.rs payload writer write_all",
        "History context brace sender.rs payload writer write_all",
    ] {
        assert_abstained(&history_answer(&service, &child, query)?);
    }
    check_history_context_budget(&service, &child, &model)?;
    cancel_history_context(&service, &child, &model)?;
    service.stop()
}

fn check_history_context_budget(
    service: &Service,
    child: &str,
    model: &Path,
) -> Result<(), TestError> {
    let ((status, answer), trace) = service.answer_with_trace(&json!({
        "query": "History context tight sender.rs write_all payload writer note",
        "repositoryId": 41, "branch": "main", "historyCommit": child, "includeModelContext": true,
    }))?;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["outcome"], "answered", "{answer}");
    let budget = admitted_context(&trace, "historyPairs")?;
    assert_eq!(budget["admitted"], budget["proposed"]);
    assert_eq!(budget["measurements"], 2);
    let parts = answer["modelContext"]["sources"][0]["parts"]
        .as_array()
        .ok_or("admitted prompt")?;
    assert_eq!(parts[0]["text"], "    // payload writer old note");
    assert_eq!(
        parts[1]["text"],
        "    // payload writer new longer safety note"
    );
    let prompt = fs::read_to_string(model.with_extension("history-prompt.json"))?;
    assert!(!prompt.contains("unchangedGroup"));
    Ok(())
}

fn cancel_history_context(service: &Service, child: &str, model: &Path) -> Result<(), TestError> {
    const REQUEST: &str = "449a4ab6-5c8d-4dd1-abbb-5984b908d627";
    thread::scope(|scope| {
        let answer = scope.spawn(|| {
            service.request("/v1/answers", Some(json!({
            "query": "Hold History context sender.rs write_all payload writer",
            "repositoryId": 41, "branch": "main", "historyCommit": child, "requestId": REQUEST,
        })))
        });
        wait(|| model.with_extension("in-flight").is_file())?;
        assert_eq!(super::cancel(service, REQUEST)?.0, 200);
        let (_, result) = answer.join().map_err(|_| "history cancellation thread")??;
        assert_eq!(result["code"], "answer_cancelled");
        Ok(())
    })
}

#[test]
fn history_changed_argument_is_not_an_unchanged_context_witness() -> Result<(), TestError> {
    for shape in ["header", "opening"] {
        let (_temp, _fixture, origin, _model, mut service) =
            history_fixture("--github-fixture", false)?;
        fs::write(origin.join(format!("history-context-{shape}")), [])?;
        stage_history_context(&service, &origin)?;
        let query = format!(
            "History context {shape} sender.rs payload writer, what is passed to write_all?"
        );
        assert_abstained(&history_answer(&service, &"b".repeat(40), &query)?);
        service.stop()?;
    }
    Ok(())
}

#[test]
fn scheduled_history_is_discoverable_without_a_selected_comparison() -> Result<(), TestError> {
    let (_temp, _fixture, origin, _model, mut service) =
        history_fixture("--github-scheduled-fixture", false)?;
    let child = "b".repeat(40);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(35);
    loop {
        let (_, evidence) = service.request(&format!("{HISTORY}/evidence/{child}"), None)?;
        if evidence["child"] == child {
            break;
        }
        if std::time::Instant::now() >= deadline {
            return Err("scheduled comparison deadline expired".into());
        }
        thread::sleep(std::time::Duration::from_millis(100));
    }
    let before = fs::read(origin.join("hits.log"))?;
    let (_, answer) = service.answer("What changed in modified.rs?")?;
    assert_eq!(answer["outcome"], "answered", "{answer}");
    assert_eq!(answer["citations"][0]["comparison"]["side"], "before");
    assert_eq!(answer["citations"][1]["comparison"]["side"], "after");
    for (repository, branch, expected) in [
        (41, "main", true),
        (42, "main", false),
        (41, "trunk", false),
    ] {
        let (status, scoped) = service.request("/v1/answers", Some(json!({
            "query": "What changed in modified.rs?", "repositoryId": repository, "branch": branch
        })))?;
        assert_eq!(status, 200, "{scoped}");
        if expected {
            assert_eq!(scoped["outcome"], "answered", "{scoped}");
            assert_eq!(scoped["citations"][0]["comparison"]["side"], "before");
            assert_eq!(scoped["citations"][1]["comparison"]["side"], "after");
            assert_eq!(scoped["evidenceRepositoryIds"], json!([41]));
        } else {
            assert_abstained(&scoped);
        }
    }
    assert_abstained(&service.answer("Unsupported modified.rs timeout?")?.1);
    assert_eq!(fs::read(origin.join("hits.log"))?, before);
    service.stop()
}

#[test]
fn latest_actions_run_is_cited_from_the_snapshot_without_generation() -> Result<(), TestError> {
    let (_temp, _fixture, origin, _model, mut service) =
        history_fixture("--github-fixture", false)?;
    for flag in ["actions-fixture", "actions-latest", "actions-completed"] {
        fs::write(origin.join(flag), [])?;
    }
    assert_eq!(
        service
            .call("PUT", REPOSITORIES, Some(configured(2, true)))?
            .0,
        200
    );
    assert_eq!(service.request(ACTIONS, Some(Value::Null))?.0, 200);
    let before = fs::read(origin.join("hits.log"))?;
    for (question, expected) in [
        (
            "C’est quoi la dernière GitHub Action qui a run sur Alpha ?",
            "Dernière",
        ),
        (
            "What was the latest GitHub Actions run for fixture/alpha?",
            "Latest",
        ),
    ] {
        let ((status, result), trace) = service.answer_with_trace(&json!({
            "query": question, "includeModelContext":true
        }))?;
        assert_eq!(status, 200, "{result}");
        assert_eq!(result["outcome"], "answered", "{result}");
        assert_eq!(result["mode"], "selected");
        let text = result["text"].as_str().ok_or("Latest run answer text")?;
        assert!(
            text.starts_with(expected) && text.contains("CI") && text.contains("2026-01-02"),
            "{text}"
        );
        let citation = &result["citations"][0];
        assert_eq!(citation["workflowRun"]["runId"], 31);
        assert_eq!(citation["workflowRun"]["attempt"], 2);
        assert_eq!(
            citation["workflowRun"]["runUrl"],
            "https://github.com/fixture/alpha/actions/runs/31/attempts/2"
        );
        assert!(
            citation["quote"]
                .as_str()
                .is_some_and(|quote| quote.contains("Observed workflow name: CI")
                    && quote.contains("Conclusion: success"))
        );
        assert_eq!(result["evidenceRepositoryIds"], json!([41]));
        assert!(result.get("modelContext").is_none());
        assert!(!trace.to_string().contains("answer_generation"));
        assert!(
            trace
                .to_string()
                .contains("Latest recorded workflow run selected")
        );
    }
    let settings = service.request("/v1/settings", None)?.1;
    let (status, _) = service.call(
        "PUT",
        "/v1/settings",
        Some(json!({
            "revision": settings["revision"],
            "answerLanguage": settings["answerLanguage"],
            "answerLocale": "es",
            "diagnosticRetentionHours": settings["diagnosticRetentionHours"],
        })),
    )?;
    assert_eq!(status, 200);
    let (status, spanish) =
        service.answer("What was the latest GitHub Actions run for fixture/alpha?")?;
    assert_eq!(status, 200);
    assert_eq!(spanish["outcome"], "answered");
    assert!(spanish["text"].as_str().is_some_and(|text| {
        text.starts_with("Última ejecución observada") && text.contains("intento 2")
    }));
    assert_eq!(spanish["citations"][0]["workflowRun"]["runId"], 31);
    assert_abstained(&service.answer("Latest GitHub Actions run for missing")?.1);
    assert_eq!(fs::read(origin.join("hits.log"))?, before);
    service.stop()
}

fn stage_actions_attempt_scope(
    service: &Service,
    origin: &Path,
    fixture: &str,
) -> Result<(), TestError> {
    fs::remove_file(origin.join("advance-code"))?;
    fs::write(origin.join(fixture), [])?;
    let mut alpha = configured(2, true);
    alpha["features"]["currentCode"] = json!(true);
    assert_eq!(service.call("PUT", REPOSITORIES, Some(alpha))?.0, 200);
    let beta = json!({
        "revision": 3, "repository": "fixture/beta", "expectedId": 42,
        "branch": "trunk", "intervalMinutes": 60,
        "features": {"currentCode": false, "history": false, "actions": true, "codeGraph": false}
    });
    assert_eq!(service.call("PUT", REPOSITORIES, Some(beta))?.0, 200);
    assert_eq!(service.request(ACTIONS, Some(Value::Null))?.0, 200);
    assert_eq!(
        service
            .request("/v1/github/repositories/42/actions", Some(Value::Null))?
            .0,
        200
    );
    assert_eq!(
        service
            .request("/v1/github/repositories/41/refresh", Some(Value::Null))?
            .0,
        200
    );
    Ok(())
}

fn check_named_attempts(
    ask: impl Fn(&str, Option<(u32, &str)>) -> Result<Value, TestError>,
) -> Result<(), TestError> {
    for (question, scope, repository, attempt, quote) in [
        (
            "What was the conclusion for GitHub Actions run 701 attempt 1 in fixture/alpha?",
            None,
            41,
            1,
            "Conclusion: success",
        ),
        (
            "Quelle conclusion GitHub pour la tentative 2 du run 701 de fixture/alpha ?",
            None,
            41,
            2,
            "Conclusion: failure",
        ),
        (
            "What was the conclusion of https://github.com/fixture/beta/actions/runs/701/attempts/2?",
            None,
            42,
            2,
            "Conclusion: success",
        ),
        (
            "What was the conclusion for GitHub Actions run 701 attempt 2?",
            Some((41, "main")),
            41,
            2,
            "Conclusion: failure",
        ),
    ] {
        let result = ask(question, scope)?;
        assert_eq!(result["outcome"], "answered", "{result}");
        let citation = &result["citations"][0];
        assert_eq!(citation["workflowRun"]["repositoryId"], repository);
        assert_eq!(citation["workflowRun"]["runId"], 701);
        assert_eq!(citation["workflowRun"]["attempt"], attempt);
        assert_eq!(citation["quote"], quote);
    }
    Ok(())
}

#[test]
fn ordinary_actions_questions_bind_exact_attempts_without_blocking_code() -> Result<(), TestError> {
    let (_temp, _fixture, origin, _model, mut service) =
        history_fixture("--github-fixture", false)?;
    stage_actions_attempt_scope(&service, &origin, "actions-attempt-scope")?;

    let ask = |query: &str, scope: Option<(u32, &str)>| -> Result<Value, TestError> {
        let mut input = json!({"query": query, "includeModelContext": true});
        if let Some((repo, branch)) = scope {
            input["repositoryId"] = json!(repo);
            input["branch"] = json!(branch);
        }
        let (status, result) = service.request("/v1/answers", Some(input))?;
        assert_eq!(status, 200, "{result}");
        Ok(result)
    };
    check_named_attempts(ask)?;
    for (question, scope) in [
        (
            "What did GitHub report for fixture/alpha run 701 attempt 8 despite the decoy?",
            None,
        ),
        (
            "What did GitHub report for fixture/alpha run 701 attempt 2 despite the decoy?",
            None,
        ),
        (
            "What was the conclusion of https://github.com/fixture/beta/actions/runs/701/attempts/2?",
            Some((41, "main")),
        ),
        (
            "What was the conclusion for GitHub Actions run 701 attempt 2?",
            None,
        ),
    ] {
        let result = ask(question, scope)?;
        assert_eq!(result["outcome"], "insufficientEvidence", "{result}");
        assert_eq!(result["citations"], json!([]));
    }
    let compare = ask(
        "Compare GitHub Actions run 701 attempt 1 and attempt 2",
        Some((41, "main")),
    )?;
    let citations = compare["citations"]
        .as_array()
        .ok_or("comparison citations")?;
    assert_eq!(citations.len(), 2);
    for attempt in [1, 2] {
        assert!(
            citations
                .iter()
                .any(|c| c["workflowRun"]["attempt"] == attempt)
        );
    }
    let result = ask(
        "What does the decoy in attempt-note.md say about GitHub Actions run 701 attempt 8?",
        Some((41, "main")),
    )?;
    assert_eq!(result["outcome"], "answered", "{result}");
    assert_eq!(result["citations"][0]["evidence"]["kind"], "current");
    let result = ask(
        "What was the result of the run parser test for GitHub Actions run 701 attempt 8 despite the decoy?",
        Some((41, "main")),
    )?;
    assert_eq!(result["outcome"], "answered", "{result}");
    assert_eq!(result["citations"][0]["evidence"]["kind"], "current");
    let (status, selected) = service.request(
        "/v1/answers",
        Some(json!({
            "query": "What was the conclusion of GitHub Actions run 701 attempt 2?",
            "repositoryId": 41, "branch": "main", "actionsRunId": 701, "actionsRunAttempt": 1
        })),
    )?;
    assert_eq!(status, 200);
    assert_eq!(selected["citations"][0]["workflowRun"]["attempt"], 1);
    assert_eq!(selected["citations"][0]["quote"], "Conclusion: success");
    let result = ask(
        "What did GitHub report for fixture/alpha run 701 attempt 2? Contrast attempt-note.md using both sources.",
        Some((41, "main")),
    )?;
    let citations = result["citations"]
        .as_array()
        .ok_or("mixed attempt citations")?;
    assert!(citations.iter().any(|c| c["evidence"]["kind"] == "current"));
    assert!(
        citations
            .iter()
            .any(|c| c["workflowRun"]["attempt"] == 2 && c["quote"] == "Conclusion: failure")
    );
    service.stop()
}

#[test]
fn a_history_only_draft_cannot_answer_an_exact_observed_attempt() -> Result<(), TestError> {
    let (_temp, _fixture, origin, _model, mut service) =
        history_fixture("--github-fixture", false)?;
    stage_actions_attempt_scope(&service, &origin, "actions-attempt-scope")?;
    fs::write(origin.join("advance-code"), [])?;
    assert_eq!(service.request(HISTORY, Some(Value::Null))?.0, 200);
    assert_eq!(
        service
            .request(
                &format!("{HISTORY}/comparisons/{}", "b".repeat(40)),
                Some(Value::Null),
            )?
            .0,
        200
    );
    let ((status, result), trace) = service.answer_with_trace(&json!({
        "query": "What did GitHub report for fixture/alpha run 701 attempt 2 despite the history decoy in modified.rs?",
        "repositoryId": 41, "branch": "main", "includeModelContext": true
    }))?;
    assert_eq!(status, 200);
    assert_eq!(result["outcome"], "insufficientEvidence", "{result}");
    assert_eq!(result["citations"], json!([]));
    assert!(
        trace
            .to_string()
            .contains("Citations could not be verified")
    );
    assert!(!trace.to_string().contains("historyComparison"));
    service.stop()
}

#[test]
fn selected_and_ordinary_actions_cite_the_recorded_fork_head() -> Result<(), TestError> {
    let (_temp, _fixture, origin, _model, mut service) =
        history_fixture("--github-fixture", false)?;
    fs::remove_file(origin.join("advance-code"))?;
    for flag in ["actions-fixture", "actions-rerun"] {
        fs::write(origin.join(flag), [])?;
    }
    assert_eq!(
        service
            .call("PUT", REPOSITORIES, Some(configured(2, true)))?
            .0,
        200,
    );
    assert_eq!(service.request(ACTIONS, Some(Value::Null))?.0, 200);

    for selected in [false, true] {
        let mut input = json!({
            "query": "What head branch and repository did GitHub report for fixture/alpha run 30 attempt 2?",
            "repositoryId": 41,
            "branch": "main"
        });
        if selected {
            input["actionsRunId"] = json!(30);
            input["actionsRunAttempt"] = json!(2);
        }
        let (status, answer) = service.request("/v1/answers", Some(input))?;
        assert_eq!(status, 200);
        assert_eq!(answer["outcome"], "answered", "{answer}");
        let citations = answer["citations"]
            .as_array()
            .ok_or("observed head citations")?;
        assert_eq!(citations.len(), 2);
        for (citation, quote) in citations.iter().zip([
            "Reported head branch: main",
            "Reported head repository ID: 99",
        ]) {
            assert_eq!(citation["quote"], quote);
            let run = &citation["workflowRun"];
            assert_eq!(run["repositoryId"], 41);
            assert_eq!(
                run["runUrl"],
                "https://github.com/fixture/alpha/actions/runs/30/attempts/2"
            );
            assert_eq!(run["commitProvenance"], "forkOrUnknownReportedHead");
        }
    }
    service.stop()
}

#[test]
fn github_answer_routes_verify_both_revisions_and_the_exact_attempt() -> Result<(), TestError> {
    let (_temp, _fixture, origin, model, mut service) = history_fixture("--github-fixture", false)?;
    collect_and_check_history(&service)?;
    collect_and_check_actions(&service, &origin)?;
    let (status, _) = service.request(
        "/v1/answers",
        Some(json!({
            "query": "invalid scope", "actionsRunId": 30, "actionsRunAttempt": 1
        })),
    )?;
    assert_eq!(status, 400);
    let (status, _) = service.request(
        "/v1/answers",
        Some(json!({
            "query": "invalid scope", "repositoryId": 41, "branch": "main",
            "historyCommit": "b".repeat(40), "actionsRunId": 30, "actionsRunAttempt": 1
        })),
    )?;
    assert_eq!(status, 400);
    collect_purge_repositories(&service, &origin)?;
    let question = "What does Alphaguardmarker say, what changed in modified.rs, and what was the workflow conclusion?";
    let hits_before = fs::read(origin.join("hits.log"))?;
    let ((status, answer), _) = service.answer_with_trace(&json!({
        "query": question, "includeModelContext": true
    }))?;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["outcome"], "answered", "{answer}");
    let sources = answer["modelContext"]["sources"]
        .as_array()
        .ok_or("mixed model sources")?;
    let change = sources
        .iter()
        .find(|source| source["change"].is_object())
        .ok_or("history provenance in mixed context")?;
    assert_eq!(change["repository"], "fixture/alpha");
    assert_eq!(change["branch"], "main");
    assert_eq!(change["change"]["beforePath"], "src/modified.rs");
    assert_eq!(change["change"]["afterPath"], "src/modified.rs");
    assert_eq!(change["change"]["child"].as_str().map(str::len), Some(40));
    assert!(sources.iter().any(|source| {
        source["kind"] == "workflow"
            && source["repository"] == "fixture/alpha"
            && source["branch"] == "main"
    }));
    let citations = answer["citations"].as_array().ok_or("mixed citations")?;
    assert_eq!(citations.len(), 4);
    assert!(
        citations
            .iter()
            .any(|c| c["evidence"]["kind"] == "current" && c["lineUrl"].is_string())
    );
    for side in ["before", "after"] {
        assert!(
            citations
                .iter()
                .any(|c| c["comparison"]["side"] == side && c["lineUrl"].is_string())
        );
    }
    assert!(
        citations
            .iter()
            .any(|c| c["workflowRun"]["runId"] == 30 && c["quote"] == "Conclusion: success")
    );
    let (status, scoped) = service.request(
        "/v1/answers",
        Some(json!({
            "query": question, "repositoryId": 41, "branch": "main"
        })),
    )?;
    assert_eq!(status, 200, "{scoped}");
    assert_eq!(scoped["outcome"], "answered", "{scoped}");
    assert_eq!(scoped["evidenceRepositoryIds"], json!([41]));
    assert!(scoped["citations"].as_array().is_some_and(|citations| {
        citations
            .iter()
            .any(|citation| citation["workflowRun"]["runId"] == 30)
    }));
    let (status, other_branch) = service.request(
        "/v1/answers",
        Some(json!({
            "query": "What was the conclusion for GitHub Actions workflow run 30?",
            "repositoryId": 41, "branch": "trunk"
        })),
    )?;
    assert_eq!(status, 200, "{other_branch}");
    assert_abstained(&other_branch);
    assert_abstained(&service.answer(&format!("Omit one side. {question}"))?.1);
    assert_eq!(fs::read(origin.join("hits.log"))?, hits_before);
    check_purge_during_answer(&service, &model)?;
    let hits = fs::read_to_string(origin.join("hits.log"))?;
    let (_, counters) = service.request("/v1/status", None)?;
    assert_eq!(counters["outboundAttempts"], hits.lines().count());
    service.stop()
}

const SETTLEMENT: &str = "Quel délai de règlement s’applique après l’expiration du traitement ?";

fn prepare_linked_corpus(service: &Service) -> Result<(), TestError> {
    let mut selection = configured(2, false);
    selection["features"]["currentCode"] = json!(true);
    selection["features"]["history"] = json!(false);
    assert_eq!(service.call("PUT", REPOSITORIES, Some(selection))?.0, 200);
    assert_eq!(
        service
            .request(&format!("{REPOSITORIES}/41/refresh"), Some(Value::Null))?
            .0,
        200
    );
    assert_eq!(
        service
            .call("PUT", "/v1/retrieval/index", Some(json!({"enabled":true})))?
            .0,
        200
    );
    wait(|| {
        service
            .request("/v1/retrieval", None)
            .is_ok_and(|(status, progress)| {
                status == 200 && progress["state"] == "ready" && progress["indexed"] == 2
            })
    })
}

#[test]
fn linked_answer_requires_both_citations_and_reopens_the_scope_after_generation()
-> Result<(), TestError> {
    let (_temp, _fixture, origin, model, mut service) = history_fixture("--github-fixture", true)?;
    prepare_linked_corpus(&service)?;
    let hits = fs::read(origin.join("hits.log"))?;
    let (status, _) = service.call(
        "PUT",
        &format!("{JOURNAL}/settings"),
        Some(json!({"revision":0,"enabled":true,"retentionDays":7})),
    )?;
    assert_eq!(status, 200);

    let ((status, answer), trace) =
        service.answer_with_trace(&json!({"query":SETTLEMENT, "includeModelContext": true}))?;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["outcome"], "answered", "{answer}");
    assert_eq!(answer["text"], "1 seconde");
    assert_eq!(answer["evidenceRepositoryIds"], json!([41]));
    let citations = answer["citations"].as_array().ok_or("linked citations")?;
    assert_eq!(citations.len(), 2);
    assert_linked_citations(citations)?;
    let sources = answer["modelContext"]["sources"]
        .as_array()
        .ok_or("linked model sources")?;
    assert_eq!(sources.len(), 2);
    assert_eq!(sources[0]["slot"], 1);
    assert_eq!(sources[0]["parts"][0]["path"], "src/policy.rs");
    assert_eq!(sources[1]["slot"], 2);
    assert!(
        sources[1]["parts"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("now.saturating_add(SETTLEMENT_DELAY_MS)"))
    );
    let linked_context = json!({
        "kind": "linkedPassages", "proposed": 2, "admitted": 2,
        "inputTokens": 1024, "inputTokenLimit": 3455, "measurements": 1
    });
    assert_eq!(admitted_context(&trace, "linkedPassages")?, &linked_context);
    let (status, saved) =
        service.request(&super::answer_history::history_trace_path(&trace)?, None)?;
    assert_eq!(status, 200);
    assert_eq!(saved["answer"]["citations"], answer["citations"]);

    let (_, fallback) = service.answer(&format!("{SETTLEMENT} Vérification partielle."))?;
    assert_eq!(fallback["text"], "1000 millisecondes");
    assert_eq!(fallback["citations"].as_array().map(Vec::len), Some(1));

    let ((status, fallback), trace) =
        service.answer_with_trace(&json!({"query":format!("{SETTLEMENT} Brouillon invalide.")}))?;
    assert_eq!(status, 200, "{fallback}");
    assert_eq!(fallback["text"], "1000 millisecondes");
    assert_eq!(fallback["citations"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        generation_budget(&trace, "error", "Generation returned invalid output")?,
        &linked_context
    );
    assert!(
        admitted_context(&trace, "passages")?["admitted"]
            .as_u64()
            .is_some_and(|count| count > 0)
    );
    assert_eq!(
        span_events(&trace, "api_answer")
            .map(|(_, name)| name)
            .collect::<Vec<_>>(),
        ["Cited answer published"]
    );
    assert_eq!(fs::read(origin.join("hits.log"))?, hits);

    let marker = model.with_extension("linked-in-flight");
    fs::remove_file(&marker)?;
    let gate = model.with_extension("linked-response-gate");
    fs::write(&gate, [])?;
    thread::scope(|scope| -> Result<(), TestError> {
        let in_flight = scope.spawn(|| service.answer(SETTLEMENT));
        wait(|| marker.is_file())?;
        let deletion = service.call("DELETE", "/v1/github/repositories/41/data", None);
        fs::remove_file(&gate)?;
        assert_eq!(deletion?.0, 200);
        let (status, answer) = in_flight.join().map_err(|_| "answer thread panicked")??;
        assert_eq!(status, 200, "{answer}");
        assert_abstained(&answer);
        Ok(())
    })?;
    service.stop()
}

fn assert_linked_citations(citations: &[Value]) -> Result<(), TestError> {
    let expected = [
        (
            "src/policy.rs",
            1,
            "pub const SETTLEMENT_DELAY_MS: u64 = 1_000;",
        ),
        (
            "src/settlement.rs",
            4,
            "let retry = now.saturating_add(SETTLEMENT_DELAY_MS);",
        ),
    ];
    for (citation, (path, line, quote)) in citations.iter().zip(expected) {
        assert_eq!(citation["evidence"]["location"]["path"], path);
        let revision = citation["evidence"]["revision"]
            .as_str()
            .ok_or("revision")?;
        assert_eq!(revision, "b".repeat(40));
        assert_eq!(citation["quote"], quote);
        assert_eq!(
            citation["lineUrl"],
            format!("https://github.com/fixture/alpha/blob/{revision}/{path}#L{line}")
        );
    }
    Ok(())
}

#[test]
fn homogeneous_code_prefers_measured_rank_prefix_before_compacting() -> Result<(), TestError> {
    let (_temp, _provider, origin, model, mut service) =
        history_fixture("--github-fixture", false)?;
    fs::write(origin.join("packing-order"), [])?;
    let mut config = configured(2, false);
    config["features"]["currentCode"] = json!(true);
    config["features"]["history"] = json!(false);
    assert_eq!(service.call("PUT", REPOSITORIES, Some(config))?.0, 200);
    assert_eq!(
        service
            .request(&format!("{REPOSITORIES}/41/refresh"), Some(Value::Null))?
            .0,
        200
    );
    for (shape, admitted, measurements, quote, log) in [
        (
            "preferred",
            2,
            3,
            r#"const COLOR: &str = "crème brûlée";"#,
            "packing-top.rs,packing-detail.rs,packing-tail.rs,packing-extra.rs\npacking-top.rs,packing-detail.rs,packing-tail.rs\npacking-top.rs,packing-detail.rs\n",
        ),
        (
            "fallback",
            3,
            4,
            "// violet packing fallback subset.",
            "packing-top.rs,packing-detail.rs,packing-tail.rs,packing-extra.rs\npacking-top.rs,packing-detail.rs,packing-tail.rs\npacking-top.rs,packing-detail.rs\npacking-top.rs,packing-tail.rs,packing-extra.rs\n",
        ),
        (
            "unpressured",
            4,
            1,
            r#"const COLOR: &str = "crème brûlée";"#,
            "packing-top.rs,packing-detail.rs,packing-tail.rs,packing-extra.rs\n",
        ),
        (
            "oversized",
            0,
            6,
            "",
            "packing-top.rs,packing-detail.rs,packing-tail.rs,packing-extra.rs\npacking-top.rs,packing-detail.rs,packing-tail.rs\npacking-top.rs,packing-detail.rs\npacking-top.rs,packing-tail.rs,packing-extra.rs\npacking-top.rs,packing-extra.rs\npacking-top.rs\n",
        ),
    ] {
        fs::write(model.with_extension("packing"), [])?;
        let ((status, answer), trace) = service.answer_with_trace(&json!({
            "query": format!("Pack {shape} violet src/packing-top.rs src/packing-detail.rs src/packing-tail.rs src/packing-extra.rs"),
            "repositoryId": 41, "branch": "main", "includeModelContext": true,
        }))?;
        assert_eq!(status, 200);
        let measured = fs::read_to_string(model.with_extension("packing"))?;
        eprintln!(
            "packing-order {}",
            json!({"shape": shape, "measuredLayouts": measured.lines().collect::<Vec<_>>(), "outcome": answer["outcome"], "context": answer["modelContext"]})
        );
        assert_eq!(
            answer["outcome"],
            if admitted == 0 {
                "insufficientEvidence"
            } else {
                "answered"
            },
            "{answer}"
        );
        let budget = if admitted == 0 {
            assert!(answer["modelContext"].is_null() && answer["citations"] == json!([]));
            generation_budget(&trace, "error", "Generation context limit applied")?
        } else {
            admitted_context(&trace, "passages")?
        };
        assert_eq!(budget["inputTokenLimit"], 3455);
        assert_eq!(budget["admitted"], admitted);
        assert_eq!(budget["measurements"], measurements);
        assert_eq!(measured, log);
        if admitted > 0 {
            check_packed_citation(&answer, shape, quote)?;
        }
    }
    service.stop()
}

fn check_packed_citation(answer: &Value, shape: &str, quote: &str) -> Result<(), TestError> {
    assert_eq!(answer["text"], quote);
    let citation = &answer["citations"][0];
    let expected = if shape == "fallback" {
        "src/packing-tail.rs"
    } else {
        "src/packing-detail.rs"
    };
    let location = &citation["evidence"]["location"];
    assert_eq!(location["path"], expected);
    assert_eq!(answer["modelContext"]["sources"][1]["slot"], 2);
    let text = answer["modelContext"]["sources"][1]["parts"][0]["text"]
        .as_str()
        .ok_or("captured code passage")?;
    let literal = if shape == "fallback" {
        quote.to_owned()
    } else {
        format!("\n{quote}")
    };
    assert_eq!(citation["quote"], literal);
    let offset = if shape == "fallback" {
        0
    } else {
        "// violet café glyph.\n".len()
    };
    assert_eq!(
        &text.as_bytes()[offset..offset + literal.len()],
        literal.as_bytes()
    );
    assert_eq!(location["startByte"], 0);
    assert_eq!(location["endByte"], text.len());
    let line = if shape == "fallback" { "#L1" } else { "#L2-L3" };
    assert_eq!(
        citation["lineUrl"],
        format!(
            "https://github.com/fixture/alpha/blob/{}/{expected}{line}",
            "b".repeat(40)
        )
    );
    Ok(())
}

#[test]
fn ordinary_unbranded_observation_requires_its_exact_attempt() -> Result<(), TestError> {
    let (_temp, _provider, origin, _model, mut service) =
        history_fixture("--github-fixture", false)?;
    stage_actions_attempt_scope(&service, &origin, "actions-observed-scope")?;
    check_unbranded_observation_controls(&service)?;
    service.stop()
}

fn check_unbranded_observation_controls(service: &Service) -> Result<(), TestError> {
    let ask = |query: &str, scope: Option<(u32, &str)>| {
        let mut input = json!({"query":query,"includeModelContext":true});
        if let Some((id, branch)) = scope {
            input["repositoryId"] = json!(id);
            input["branch"] = json!(branch);
        }
        service.answer_with_trace(&input)
    };
    let scoped = Some((41, "main"));
    for (query, scope) in [
        (
            "What conclusion is recorded for run 9431, attempt 7 in fixture/alpha?",
            None,
        ),
        (
            "Quelle conclusion est enregistrée pour l’exécution 9431, tentative 7 de fixture/alpha ?",
            None,
        ),
        (
            "What was the result of run 9431 attempt 2 in fixture/beta?",
            scoped,
        ),
        ("What was the status of run 9431 attempt 2?", None),
    ] {
        let ((status, answer), trace) = ask(query, scope)?;
        assert_eq!(status, 200);
        assert_eq!(answer["outcome"], "insufficientEvidence", "{answer}");
        assert_eq!(answer["citations"], json!([]));
        assert!(!trace.to_string().contains("answer_generation"), "{trace}");
        eprintln!(
            "observed-scope {}",
            json!({"query":query,"answer":answer,"trace":trace})
        );
    }
    for (query, scope, attempt, quote) in [
        (
            "What was the outcome of run 9431 attempt 2 in fixture/alpha?",
            None,
            2,
            "Conclusion: failure",
        ),
        (
            "What was the result of run 9431 attempt 1?",
            scoped,
            1,
            "Conclusion: success",
        ),
        (
            "QUELLE EST LA CONCLUSION DE LA TENTATIVE 2 DE L’EXÉCUTION 9431 ?",
            scoped,
            2,
            "Conclusion: failure",
        ),
    ] {
        let ((status, answer), _) = ask(query, scope)?;
        assert_eq!(status, 200);
        assert_eq!(answer["outcome"], "answered", "{answer}");
        let citation = &answer["citations"][0];
        assert_eq!(citation["quote"], quote);
        assert_eq!(
            citation["workflowRun"]["runUrl"],
            format!("https://github.com/fixture/alpha/actions/runs/9431/attempts/{attempt}")
        );
    }
    for query in [
        "What conclusion is recorded for the parcel parser in run 9431 attempt 7 in parcel-parser.md?",
        "Which parser fixture mentions https://github.com/fixture/alpha/actions/runs/9431/attempts/7 in parcel-parser.md?",
    ] {
        let ((_, answer), _) = ask(query, scoped)?;
        assert_eq!(answer["outcome"], "answered", "{answer}");
        assert_eq!(answer["citations"][0]["evidence"]["kind"], "current");
    }
    let ((_, answer), _) = ask("Compare run 9431 attempt 1 and attempt 2", scoped)?;
    let citations = answer["citations"]
        .as_array()
        .ok_or("comparison citations")?;
    assert_eq!(citations.len(), 2, "{answer}");
    assert!(
        citations
            .iter()
            .any(|citation| citation["workflowRun"]["attempt"] == 1)
    );
    assert!(
        citations
            .iter()
            .any(|citation| citation["workflowRun"]["attempt"] == 2)
    );
    Ok(())
}

#[test]
fn observed_job_or_step_cannot_use_a_run_conclusion() -> Result<(), TestError> {
    let (_temp, _provider, origin, _model, mut service) =
        history_fixture("--github-fixture", false)?;
    fs::remove_file(origin.join("advance-code"))?;
    for marker in ["actions-fixture", "actions-job-authority"] {
        fs::write(origin.join(marker), [])?;
    }
    let mut config = configured(2, true);
    config["features"]["currentCode"] = json!(true);
    config["features"]["history"] = json!(false);
    assert_eq!(service.call("PUT", REPOSITORIES, Some(config))?.0, 200);
    assert_eq!(service.request(ACTIONS, Some(Value::Null))?.0, 200);
    assert_eq!(
        service
            .request(&format!("{REPOSITORIES}/41/refresh"), Some(Value::Null))?
            .0,
        200
    );
    check_job_authority(&service)?;
    service.stop()
}

fn check_job_authority(service: &Service) -> Result<(), TestError> {
    let ask = |query: &str, selected: bool| {
        let mut input = json!({"query":query,"includeModelContext":true});
        if selected {
            input["repositoryId"] = json!(41);
            input["branch"] = json!("main");
            input["actionsRunId"] = json!(5827);
            input["actionsRunAttempt"] = json!(3);
        }
        service.answer_with_trace(&input)
    };
    for (query, selected) in [
        (
            "Did the `dock-sign` job succeed in run 5827 attempt 3?",
            false,
        ),
        (
            "Quelle conclusion est enregistrée pour l’étape `seal-log` de l’exécution 5827, tentative 3 ?",
            false,
        ),
        (
            "What conclusion was recorded for the job `dock-sign`?",
            true,
        ),
        ("Quel est le résultat de l’étape `seal-log` ?", true),
        (
            "For the recorded run 5827, attempt 3, did job `dock-sign` succeed?",
            false,
        ),
        (
            "Using only that run record, did step `seal-log` of job `dock-sign` pass?",
            true,
        ),
    ] {
        let ((status, answer), trace) = ask(query, selected)?;
        eprintln!(
            "job-authority {}",
            json!({"query":query,"answer":answer,"trace":trace})
        );
        assert_eq!(status, 200);
        assert_eq!(answer["outcome"], "insufficientEvidence", "{answer}");
        assert_eq!(answer["citations"], json!([]));
        assert_eq!(answer["mode"], "selected");
        assert!(!trace.to_string().contains("answer_generation"), "{trace}");
    }
    for (query, selected, kind) in [
        (
            "What was the conclusion of run 5827 attempt 3 while the dock-sign job was defined?",
            false,
            "workflow",
        ),
        ("What was the conclusion of the run?", true, "workflow"),
        (
            "What conclusion was recorded for the job `dock-sign` in Jenkins run 5827 according to dock-report.md?",
            false,
            "current",
        ),
        (
            "What does .github/workflows/dock.yml define for the dock-sign job and seal-log step?",
            false,
            "current",
        ),
        (
            "What conclusion was recorded for the job `dock-sign` in dock-report.md for run 5827 attempt 3?",
            false,
            "current",
        ),
        (
            "What conclusion was recorded for the parser test of job dock-sign in dock-report.md?",
            false,
            "current",
        ),
    ] {
        let ((status, answer), _) = ask(query, selected)?;
        assert_eq!(status, 200);
        assert_eq!(answer["outcome"], "answered", "{answer}");
        let citation = &answer["citations"][0];
        assert_eq!(citation["evidence"]["kind"], kind);
        if kind == "workflow" {
            assert_eq!(citation["workflowRun"]["attempt"], 3);
            assert_eq!(citation["quote"], "Conclusion: failure");
        } else {
            assert_dock_file_citation(query, citation)?;
        }
    }
    Ok(())
}

fn assert_dock_file_citation(query: &str, citation: &Value) -> Result<(), TestError> {
    assert_eq!(citation["evidence"]["revision"], "a".repeat(40));
    let (path, raw, expected) = if query.contains("dock.yml") {
        (
            ".github/workflows/dock.yml",
            "name: Dock release\non: push\njobs:\n  dock-sign:\n    runs-on: ubuntu-latest\n    steps:\n      - name: seal-log\n        run: echo dock-sign succeeded\n",
            "run: echo dock-sign succeeded",
        )
    } else {
        (
            "dock-report.md",
            "Recorded dock-sign job outcome: success.\n",
            "Recorded dock-sign job outcome: success.",
        )
    };
    let quote = citation["quote"]
        .as_str()
        .ok_or("exact reopened file quote")?;
    assert_eq!(citation["evidence"]["location"]["path"], path);
    assert!(
        raw.contains(quote) && quote.contains(expected),
        "{citation}"
    );
    Ok(())
}
