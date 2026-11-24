use super::*;

#[test]
fn optional_formulation_falls_back_and_can_be_cancelled_without_stopping_embeddings()
-> Result<(), TestError> {
    let work = tempfile::tempdir()?;
    let directory = work.path().join("data");
    seed(&directory, &FIXTURE[..1])?;
    let assets = stage_worker_fixture(&directory)?;
    std::fs::write(assets.join(EMBEDDING_FILE), b"formulation fixture")?;
    let mut service = Service::start(Path::new(env!("CARGO_BIN_EXE_ctxfab-server")), &directory)?;
    assert_eq!(
        service
            .call(
                "PUT",
                "/v1/retrieval/index",
                Some(json!({"enabled": false}))
            )?
            .0,
        200
    );
    let (answer, trace) = service.answer_with_trace(&json!({"query":QUESTION}))?;
    accepted(&answer, 0, "47 seconds");
    assert_eq!(answer.1["mode"], "bm25");
    assert!(
        span_events(&trace, "answer_retrieval").any(|(_, name)| name == "Semantic index disabled")
    );
    assert_eq!(
        service
            .call("PUT", "/v1/retrieval/index", Some(json!({"enabled": true})))?
            .0,
        200
    );
    wait(|| {
        service
            .request("/v1/retrieval", None)
            .is_ok_and(|(status, state)| {
                status == 200 && state["state"] == "ready" && state["indexed"] == 1
            })
    })?;
    let translated =
        service.answer("Reformulez la question sur le délai que cette fixture décrit.")?;
    accepted(&translated, 0, "47 seconds");
    assert_eq!(translated.1["mode"], "hybrid");
    let original = only_generation_worker(service.pid)?;
    let malformed = service
        .answer("Échouez à reformuler la question sur le délai que cette fixture décrit.")?;
    accepted(&malformed, 0, "47 seconds");
    assert_eq!(malformed.1["mode"], "hybrid");
    assert_eq!(malformed.1["citations"], translated.1["citations"]);
    assert_eq!(only_generation_worker(service.pid)?.pid, original.pid);
    let embedding = workers(service.pid)
        .into_iter()
        .find(|worker| !worker.generation)
        .ok_or("missing embedding worker")?;

    thread::scope(|scope| -> Result<(), TestError> {
        let started = assets
            .join(GENERATION_FILE)
            .with_extension("formulation-in-flight");
        let in_flight = scope.spawn(|| {
            named_answer(
                &service,
                "Attendez avant de reformuler cette question sur le délai de la fixture.",
            )
        });
        wait(|| started.is_file())?;
        assert!(!in_flight.is_finished());
        assert_eq!(cancel(&service, REQUEST)?.0, 200);
        let (status, answer) = in_flight.join().map_err(|_| "answer test panicked")??;
        assert_eq!(status, 409);
        assert_eq!(answer["code"], "answer_cancelled");
        Ok(())
    })?;
    let (answer, detail) = service.answer_with_trace(&json!({"query":QUESTION}))?;
    accepted(&answer, 0, "47 seconds");
    check_fusion(&detail, "api_answer", "answer_retrieval")?;
    check_search_trace(&service)?;
    assert_replaced(&original, &only_generation_worker(service.pid)?);
    assert!(
        workers(service.pid)
            .iter()
            .any(|worker| worker.pid == embedding.pid)
    );
    service.stop()?;
    Ok(())
}

fn check_search_trace(service: &Service) -> Result<(), TestError> {
    let response = service
        .http
        .get(format!(
            "{}/v1/search?query=retry%20cooldown&mode=hybrid",
            service.address
        ))
        .header("Authorization", &service.token)
        .call()?;
    let id = response.headers()["x-context-trace-id"]
        .to_str()?
        .to_owned();
    let (status, hits) = decode_response(response)?;
    assert_eq!(status, 200);
    assert_eq!(hits[0]["source"], "src/outbound.rs");
    check_fusion(
        &service.trace_detail(&id, "api_query")?,
        "api_query",
        "vector_retrieval",
    )
}

fn check_fusion(detail: &Value, root_stage: &str, lookup_stage: &str) -> Result<(), TestError> {
    let spans = detail["spans"].as_array().ok_or("trace spans")?;
    let root = spans
        .iter()
        .find(|span| span["name"] == root_stage)
        .ok_or("answer root")?;
    let lookup = spans
        .iter()
        .find(|span| span["name"] == lookup_stage)
        .ok_or("retrieval span")?;
    assert_eq!(root["httpStatus"], 200);
    assert!(lookup.get("httpStatus").is_none());
    assert_eq!(lookup["parentSpanId"], root["spanId"]);
    assert_eq!(
        lookup["fusionRanking"],
        json!({
            "rankOffset":60,"denseWeight":1.0,"lexicalCandidates":1,"denseCandidates":1,
            "candidates":[{"lexicalRank":1,"denseRank":1,"score":2.0/61.0,
                "scoreRank":1,"finalRank":1,"lexicalPromoted":false}]
        })
    );
    assert!(!detail["logs"].to_string().contains("fusionRanking"));
    assert!(!detail.to_string().contains("src/outbound.rs"));
    assert!(!detail.to_string().contains("47 seconds"));
    Ok(())
}
