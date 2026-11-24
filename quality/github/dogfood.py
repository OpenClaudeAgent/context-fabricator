"""Qualify the packaged helper and CLI on Context Fabricator's pinned Git snapshots."""

import json
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import time
import uuid
from contextlib import contextmanager
from pathlib import Path
from tempfile import TemporaryDirectory
from urllib.error import HTTPError
from urllib.parse import quote, urlencode, urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

from git_origin import FIXTURE_TOKEN, REPOSITORY, REPOSITORY_ID, GitHistory, origin
from retrieval_evaluation import evaluate

BASE = "9ece069abec91a537ec9a57fa65c140eaad36100"
TIP = "a3b844b3b59213156866cf913ba7aa228d7a30a8"
GENERATION_PROFILE = "qwen3-4b-instruct-2507-q4-k-m-3605803b-v1"
FEATURES = {"currentCode": True, "history": False, "actions": False, "codeGraph": False}
REDIRECT_SOURCE = "backend/server/src/adapters/outbound/github/request.rs"
RATE_SOURCE = "backend/server/src/domain/github.rs"
COOLDOWN_SOURCE = "backend/server/src/domain/sync.rs"
COOLDOWN_ANCHOR = "pub const UNCERTAIN_COOLDOWN_MS: i64 = 1_000;"
COOLDOWN_USAGE = "backend/server/src/runtime/outbound.rs"
COOLDOWN_USE = "let cooldown = gate.clock.now().saturating_add(UNCERTAIN_COOLDOWN_MS);"
COOLDOWN_FR = "Quel délai est appliqué après une requête dont la livraison n’a pas pu être confirmée ?"
COOLDOWN_FR_VALUE = r"\b(?:1[,_\s]?000\s*(?:ms|millisecondes?)|1\s*(?:s|secondes?))\b"
THROTTLE_FR = "Quel est le délai de repli après une réponse HTTP 429 sans en-tête Retry-After ?"
THROTTLE_SOURCE = "backend/server/src/adapters/outbound/http.rs"
THROTTLE_QUOTE = "retry_after(response.headers()).unwrap_or(Duration::from_secs(2))"
# This path belongs to the pinned Git snapshots used as the corpus.
HISTORY_QUESTION = ("In quality/m07/github_fixture.py, which class was used in the with statement "
                    "that starts the HTTP server before and after this commit?")


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


def wait_until(deadline_seconds, check):
    deadline = time.monotonic() + deadline_seconds
    while time.monotonic() < deadline:
        result = check()
        if result is not None:
            return result
        time.sleep(0.1)
    raise TimeoutError(f"Timed out polling {check.__name__}.")


def save(report, filename, value):
    (report / filename).write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n",
                                   encoding="utf-8")


def record_trace(api, identifier, stage, report, filename):
    def available():
        try:
            result, _ = api("GET", f"/v1/telemetry/traces/{identifier}")
        except HTTPError as error:
            if error.code != 404:
                raise
            error.close()
            return None
        return result if any(span["name"] == stage for span in result["spans"]) else None

    detail = wait_until(10, available)
    save(report, filename, detail)
    return detail


def prepare_answers(api, report, *, embeddings=False, expect_download=True, fixture_attempts=None):
    # The durable outbound counter survives helper restarts after fixture publication.
    def model_attempts():
        total = api("GET", "/v1/status")[0]["outboundAttempts"]
        return total if fixture_attempts is None else total - fixture_attempts()

    runtime, _ = api("GET", "/v1/runtime")
    assert runtime["state"] == "ready", runtime
    assert model_attempts() == 0, "Unexpected outbound traffic before model setup"
    route = "/v1/inference/setup"
    selection = {"runtimeSource": "hosted", "embedding": embeddings}
    plan, _ = api("POST", route + "/plan", selection)
    assert plan["canStart"] and not plan["runtimeRequired"], plan
    assert model_attempts() == 0, "Unexpected outbound traffic while planning model setup"
    operation = str(uuid.uuid4())
    started, _ = api("POST", route, {
        "operationId": operation, "fingerprint": plan["fingerprint"], "selection": plan["selection"],
    })
    assert started["operationId"] == operation

    def completed():
        state, _ = api("GET", route)
        assert state["operationId"] == operation and state["phase"] not in (
            "failed", "interrupted", "waitingForArchive",
        ), state
        return state if state["phase"] == "completed" else None

    state = wait_until(1800 if expect_download else 120, completed)
    assert state["answerReady"]
    assert api("GET", "/v1/generation/models")[0]["activeProfile"] == GENERATION_PROFILE
    attempts = model_attempts()
    assert (attempts > 0 if expect_download else attempts == 0), attempts
    save(report, "answer-setup.json", {"runtime": runtime, "plan": plan, "status": state,
                                      "modelDownloadAttempts": attempts})
    return attempts


class BackendSession:
    def __init__(self, endpoint, discovered_ms):
        self.process_id = endpoint["processId"]
        self.discovered_ms = discovered_ms
        self._endpoint = endpoint

    def __call__(self, method, route, body=None):
        request = Request(
            self._endpoint["url"] + route, method=method,
            data=json.dumps(body).encode() if body is not None else None,
            headers={"Authorization": "Bearer " + self._endpoint["token"],
                     "Content-Type": "application/json"},
        )
        opener = build_opener(ProxyHandler({}), NoRedirect())
        with opener.open(request, timeout=150) as response:
            return json.load(response), response.headers.get("X-Context-Trace-Id")


def client_environment():
    return {key: os.environ[key] for key in ("HOME", "USER", "LOGNAME", "PATH", "TMPDIR", "LANG")
            if key in os.environ}


@contextmanager
def backend(bundle, data, port, report, *, hub_port=None, preview_runtime=True):
    helper = bundle / "Contents/MacOS/ctxfab-server"
    fixture_args = []
    if port is None:
        environment = client_environment()
    else:
        fixture_args = ["--github-scheduled-fixture", str(port)]
        environment = os.environ.copy()
        for key in ("CTXFAB_EMBEDDING_ADDRESS", "CTXFAB_EMBEDDING_KEY_FILE",
                    "CTXFAB_INFERENCE_DIRECTORY", "CTXFAB_TEST_HUGGING_FACE_PORT",
                    "CTXFAB_TEST_PREVIEW_RUNTIME"):
            environment.pop(key, None)
        if hub_port is not None:
            environment["CTXFAB_TEST_HUGGING_FACE_PORT"] = str(hub_port)
        if preview_runtime:
            environment["CTXFAB_TEST_PREVIEW_RUNTIME"] = "1"
    output = (report / "backend.log").open("w", encoding="utf-8")
    started = time.monotonic()

    with output, subprocess.Popen(
        [helper, data, "--app-session", *fixture_args],
        stdin=subprocess.PIPE, stdout=output, stderr=subprocess.STDOUT, env=environment,
    ) as process:
        def endpoint_ready():
            if process.poll() is not None:
                raise RuntimeError("Packaged backend failed to start. See backend.log.")
            path = data / "endpoint.json"
            return json.loads(path.read_text()) if path.is_file() else None

        try:
            endpoint = wait_until(10, endpoint_ready)
            expected = urlsplit(endpoint["url"])
            assert expected.scheme == "http" and expected.hostname == "127.0.0.1"
            assert endpoint["processId"] == process.pid
            yield BackendSession(endpoint, round((time.monotonic() - started) * 1000))
        finally:
            process.stdin.close()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
                raise
            assert process.returncode == 0


def wait_for_commit(api, commit, *, timeout=120):
    def published():
        state, _ = api("GET", "/v1/github")
        collection = state["repositories"][0]["codeCollection"]
        if collection["phase"] == "succeeded":
            assert collection["commit"] == commit
            return collection
        if collection["phase"] in ("failed", "credentials"):
            raise RuntimeError(f"Failed to publish GitHub snapshot: {collection}")
        return None

    return wait_until(timeout, published)


def read_manifest(data, git, commit):
    database = data / "context.sqlite3"
    with sqlite3.connect(database.as_uri() + "?mode=ro", uri=True) as store:
        rows = store.execute(
            """SELECT f.path, f.blob, s.revision
               FROM source_snapshot_files f
               JOIN source_snapshots s ON s.id = f.snapshot_id
               WHERE s.phase = 'published'"""
        ).fetchall()
    accepted, skipped = git.coverage(commit)
    assert {revision for _, _, revision in rows} == {commit}
    assert {path: blob for path, blob, _ in rows} == {
        path: entry["sha"] for path, entry in accepted.items()
    }
    return {"commit": commit, "trackedPaths": len(git.files[commit]),
            "publishedPaths": len(accepted),
            "publishedBytes": sum(entry["size"] for entry in accepted.values()),
            "skipped": skipped}


def command(bundle, data, action, query, branch="main", history_commit=None, attempt=None, mode=None,
            *, repository_id=REPOSITORY_ID):
    cli = bundle / "Contents/MacOS/ctxfab"
    options = ["--repository-id", str(repository_id), "--branch", branch]
    if history_commit:
        options += ["--history-commit", history_commit]
    if attempt:
        options += ["--actions-run-id", str(attempt[0]), "--actions-run-attempt", str(attempt[1])]
    if mode:
        options += ["--mode", mode]
    result = subprocess.run(
        [cli, action, query, *options],
        env={**client_environment(), "CTXFAB_DATA_DIRECTORY": str(data)},
        timeout=150, check=True, capture_output=True,
    )
    return json.loads(result.stdout)


def verify_evidence(git, evidence, commit=TIP, *, repository=REPOSITORY, repository_id=REPOSITORY_ID, branch="main"):
    location = evidence["location"]
    path = location["path"]
    blob = git.files[commit][path]["sha"]
    if evidence["revision"] != commit or evidence["trackedRef"] != f"refs/heads/{branch}":
        raise AssertionError("Evidence does not match the pinned revision and branch")
    if evidence["source"] != f"github:{repository_id}:{path}" or location["blob"] != blob:
        raise AssertionError("Evidence does not match the pinned repository path and blob")
    if location["fileUrl"] != f"https://github.com/{repository}/blob/{commit}/{quote(path, safe='/')}":
        raise AssertionError("Evidence file URL does not match the pinned source")
    source = git.blob(blob)
    start, end = location["startByte"], location["endByte"]
    if not (type(start) is int and type(end) is int and 0 <= start < end <= len(source)):
        raise AssertionError("Evidence byte range is outside the pinned source")
    passage = source[start:end]
    start_line = source[:start].count(b"\n") + 1
    end_line = source[:end].count(b"\n") + (not passage.endswith(b"\n"))
    if location["startLine"] != start_line or location["endLine"] != end_line:
        raise AssertionError("Evidence line range does not match its byte range")
    return passage.decode()


def verify_quote_link(citation, excerpt):
    location = citation["evidence"]["location"]
    quote = citation["quote"]
    if not isinstance(quote, str) or not quote:
        raise AssertionError("Citation quote must be nonempty text")
    matches = list(re.finditer(f"(?={re.escape(quote)})", excerpt))
    if not matches:
        raise AssertionError("Citation quote is absent from the pinned excerpt")
    if len(matches) == 1 or "lineUrl" in citation:
        expected = []
        for match in matches:
            first = location["startLine"] + excerpt[:match.start()].count("\n")
            last = first + quote.removesuffix("\n").count("\n")
            fragment = f"L{first}" if first == last else f"L{first}-L{last}"
            expected.append(f"{location['fileUrl']}#{fragment}")
        if expected.count(citation.get("lineUrl")) != 1:
            raise AssertionError("Citation line URL does not resolve to exactly one quote occurrence")


def french_cooldown_language(answer):
    return bool(re.search(r"\b(?:délai|requête|livraison|attente|secondes?|millisecondes?)\b",
                          answer.get("text", ""), re.IGNORECASE))


def verify_linked_cooldown(answer, git):
    quoted = set()
    for citation in answer["citations"]:
        evidence = citation["evidence"]
        verify_quote_link(citation, verify_evidence(git, evidence))
        quoted.add((evidence["location"]["path"], citation["quote"].strip()))
    assert quoted == {(COOLDOWN_SOURCE, COOLDOWN_ANCHOR), (COOLDOWN_USAGE, COOLDOWN_USE)}


def verify_unscoped_french_cooldown(api, git, report):
    answer, trace = api("POST", "/v1/answers", {"query": COOLDOWN_FR})
    save(report, "unscoped-cooldown-fr.json", answer)
    assert answer["model"] == GENERATION_PROFILE and answer["mode"] == "hybrid"
    assert answer["outcome"] == "answered" and french_cooldown_language(answer)
    assert re.search(COOLDOWN_FR_VALUE, answer["text"], re.IGNORECASE)
    verify_linked_cooldown(answer, git)
    assert trace
    detail = record_trace(api, trace, "answer_validation", report, "unscoped-cooldown-fr-trace.json")
    events = {(span["name"], event["name"]) for span in detail["spans"]
              for event in span.get("events", [])}
    assert ("answer_retrieval", "Auxiliary search formulation ranked") in events
    assert ("answer_validation", "Declaration and usage verified") in events
    assert COOLDOWN_FR not in json.dumps(detail, ensure_ascii=False)


def verify_http_throttle_control(api, git, report):
    answer, trace = api("POST", "/v1/answers", {
        "query": THROTTLE_FR, "repositoryId": REPOSITORY_ID, "branch": "main",
    })
    save(report, "http-throttle-control.json", answer)
    assert answer["outcome"] == "answered" and re.search(r"\b2\s*(?:s|secondes?)\b", answer.get("text", ""))
    cited = False
    for citation in answer["citations"]:
        evidence = citation["evidence"]
        verify_quote_link(citation, verify_evidence(git, evidence))
        cited |= evidence["location"]["path"] == THROTTLE_SOURCE and THROTTLE_QUOTE in citation["quote"]
        assert evidence["location"]["path"] != COOLDOWN_SOURCE
    assert cited and trace
    detail = record_trace(api, trace, "answer_validation", report, "http-throttle-control-trace.json")
    assert all(event["name"] != "Declaration and usage verified"
               for span in detail["spans"] for event in span.get("events", []))


def judge_answers(bundle, data, git, report, mode="bm25"):
    cases = [
        ("fr", "Quel plafond de requêtes GitHub par minute fixe MAX_REQUESTS_PER_MINUTE ?",
         "MAX_REQUESTS_PER_MINUTE", RATE_SOURCE, "pub const MAX_REQUESTS_PER_MINUTE: u16 = 60;",
         r"\b60\s*(?:req/min|(?:requêtes?|requests?)\s+(?:(?:par|per)\s+minute|/\s*min))\b"),
        ("en", "In the GitHub REST request adapter, what maximum does MAX_REDIRECTS set?",
         "MAX_REDIRECTS", REDIRECT_SOURCE, "const MAX_REDIRECTS: usize = 3;", r"\b3\b"),
    ]
    if mode == "hybrid":
        cases.extend([
            ("cooldown-en", "What cooldown follows a request if its delivery could not be confirmed?",
             "UNCERTAIN_COOLDOWN_MS", COOLDOWN_SOURCE, COOLDOWN_ANCHOR,
             r"\b(?:1[,_\s]?000\s*(?:ms|milliseconds?)|1\s*(?:s|seconds?))\b"),
            ("cooldown-fr", COOLDOWN_FR, "UNCERTAIN_COOLDOWN_MS", COOLDOWN_SOURCE,
             COOLDOWN_ANCHOR, COOLDOWN_FR_VALUE),
            ("passage-limit-fr", "Quelle est la taille maximale d’un passage fixée par MAX_PASSAGE_BYTES ?",
             "MAX_PASSAGE_BYTES", "backend/inference/src/qwen3.rs",
             "pub const MAX_PASSAGE_BYTES: usize = 3072;",
             r"\b(?:3[,_\s]?072\s*(?:B|o|octets?|bytes?)|3\s*Kio)\b"),
        ])
    results = []
    rankings = {}
    for case, question, identifier, path, anchor, expected in cases:
        start = time.monotonic()
        hits = command(bundle, data, "search", identifier, mode=mode)
        exact = [hit for hit in hits if hit.get("location", {}).get("path") == path
                 and anchor in verify_evidence(git, hit)]
        ranked = command(bundle, data, "search", question, mode=mode)
        rankings[case] = [{"rank": rank, **hit["location"],
                           "anchor": hit["location"]["path"] == path
                           and anchor in verify_evidence(git, hit)}
                          for rank, hit in enumerate(ranked, start=1)]
        save(report, "lexical-rankings.json", rankings)
        answer = command(bundle, data, "ask", question)
        save(report, "answer-" + case + ".json", answer)
        if case == "cooldown-fr":
            verify_linked_cooldown(answer, git)
        quoted = []
        for entry in answer["citations"]:
            verify_quote_link(entry, verify_evidence(git, entry["evidence"]))
            if entry["evidence"]["location"]["path"] == path:
                quoted.append(entry["quote"])
        correct = answer["outcome"] == "answered" and bool(
            re.search(expected, answer.get("text", ""), re.IGNORECASE)
        )
        grounded = any(anchor in quote for quote in quoted)
        language_correct = case != "cooldown-fr" or french_cooldown_language(answer)
        results.append({"language": case.rsplit("-", 1)[-1], "case": case,
                        "question": question, "searchFoundAnchor": bool(exact),
                        "correctValue": correct, "correctLanguage": language_correct,
                        "quotedAnchor": grounded,
                        "questionAnchorRank": next((hit["rank"] for hit in rankings[case]
                                                    if hit["anchor"]), None),
                        "outcome": answer["outcome"], "mode": answer["mode"],
                        "wallTimeMs": round((time.monotonic() - start) * 1000)})
        assert answer["model"] == GENERATION_PROFILE
        assert answer["mode"] == mode
    for branch, query in [
        ("main", "zqphylowz qoghnomyk zyrwpjvo"),
        ("unindexed", cases[0][1]),
    ]:
        answer = command(bundle, data, "ask", query, branch=branch)
        save(report, "abstention-" + branch + ".json", answer)
        assert answer["outcome"] == "insufficientEvidence" and answer["citations"] == []
    save(report, "judgments.json", results)
    assert all(
        item["searchFoundAnchor"] and item["correctValue"] and item["correctLanguage"]
        and item["quotedAnchor"]
        and (item["case"] == "cooldown-fr" or (
            item["questionAnchorRank"] is not None and item["questionAnchorRank"] <= 5
        ))
        for item in results
    ), "See the recorded retrieval judgments."


def judge_history(bundle, data, git, api, report, current_code=True, *, timeout=30):
    state, _ = api("GET", "/v1/github")
    api("PUT", "/v1/github/repositories", {
        "revision": state["revision"], "repository": REPOSITORY, "expectedId": REPOSITORY_ID,
        "branch": "main", "intervalMinutes": 60,
        "features": {**FEATURES, "currentCode": current_code, "history": True}})
    prefix = f"/v1/github/repositories/{REPOSITORY_ID}/history"
    def published_history():
        state, _ = api("GET", prefix)
        return state.get("snapshot")
    history = wait_until(timeout, published_history)
    assert history["tip"] == TIP and history["commits"][0]["parents"][0] == BASE
    def prepared_history():
        return api("GET", f"{prefix}/evidence/{TIP}")[0]
    evidence = wait_until(timeout, prepared_history)
    assert evidence["child"] == TIP and evidence["parent"] == BASE
    save(report, "history-evidence.json", evidence)
    question = HISTORY_QUESTION
    before_attempts, _ = api("GET", "/v1/status")
    answer, trace = api("POST", "/v1/answers", {
        "query": question, "repositoryId": REPOSITORY_ID, "branch": "main",
        "historyCommit": TIP})
    record_trace(api, trace, "api_answer", report, "history-trace.json")
    after_attempts, _ = api("GET", "/v1/status")
    assert before_attempts["outboundAttempts"] == after_attempts["outboundAttempts"]
    save(report, "history-answer.json", answer)
    assert answer["outcome"] == "answered"
    assert command(bundle, data, "ask", question, history_commit=TIP) == answer
    assert re.search(
        r"\bbefore\b.*?\bThreadingHTTPServer\b.*?\bafter\b.*?\bLoopbackHTTPServer\b"
        r"|\bfrom\s+`?ThreadingHTTPServer`?\s+to\s+`?LoopbackHTTPServer\b",
        answer["text"], re.IGNORECASE | re.DOTALL,
    )
    assert answer["mode"] == "bm25" and answer["model"] == GENERATION_PROFILE
    citations = answer["citations"]
    assert len(citations) == 2
    for citation, (side, commit, server) in zip(citations, [
        ("before", BASE, "ThreadingHTTPServer"), ("after", TIP, "LoopbackHTTPServer"),
    ]):
        item = citation["evidence"]
        locator = item["location"]
        comparison = citation["comparison"]
        assert item["kind"] == "history" and item["revision"] == commit
        assert comparison["side"] == side and comparison["child"] == TIP and comparison["parent"] == BASE
        assert locator["path"] == "quality/m07/github_fixture.py"
        assert locator["fileUrl"] == f"https://github.com/{REPOSITORY}/blob/{commit}/{locator['path']}"
        blob = git.files[commit][locator["path"]]["sha"]
        assert locator["blob"] == blob
        source = git.blob(blob)
        start, end = locator["startByte"], locator["endByte"]
        assert source[start:end] == citation["quote"].encode() and server in citation["quote"]
        assert locator["startLine"] == locator["endLine"] == source[:start].count(b"\n") + 1
        verify_quote_link(citation, source[start:end].decode())
    assert [(citation["comparison"]["changeIndex"], citation["comparison"]["hunkIndex"])
            for citation in citations] == [(0, 2), (0, 2)]
    natural, _ = api("POST", "/v1/answers", {
        "query": "How did the HTTP server class in quality/m07/github_fixture.py change?",
    })
    assert natural["outcome"] == "answered"
    assert all(name in natural["text"] for name in ("ThreadingHTTPServer", "LoopbackHTTPServer"))
    assert natural["citations"] == citations
    assert api("GET", "/v1/status")[0]["outboundAttempts"] == after_attempts["outboundAttempts"]
    save(report, "unscoped-history.json", natural)
    absent = command(bundle, data, "ask", question, branch="unindexed", history_commit=TIP)
    assert absent["outcome"] == "insufficientEvidence" and absent["citations"] == []
    unsupported = command(
        bundle, data, "ask", "What database engine did the GitHub fixture HTTP server change?",
        history_commit=TIP,
    )
    save(report, "history-abstention.json", unsupported)
    assert unsupported["outcome"] == "insufficientEvidence" and unsupported["citations"] == []


def judge_actions(bundle, data, git, api, report, current_code=True, *, timeout=30):
    state, _ = api("GET", "/v1/github")
    api("PUT", "/v1/github/repositories", {
        "revision": state["revision"], "repository": REPOSITORY, "expectedId": REPOSITORY_ID,
        "branch": "main", "intervalMinutes": 60,
        "features": {**FEATURES, "currentCode": current_code,
                     "history": current_code, "actions": True}})
    route = f"/v1/github/repositories/{REPOSITORY_ID}/actions"
    def published_actions():
        response, _ = api("GET", route)
        if response.get("snapshot") and response["availability"] == "ready":
            return response["snapshot"]
        if response["availability"] != "ready" or response.get("schedule", {}).get("lastError"):
            save(report, "actions-error.json", response)
            raise RuntimeError("Expected configured Actions publication. See actions-error.json.")
        return None
    snapshot = wait_until(timeout, published_actions)
    save(report, "actions-snapshot.json", snapshot)
    assert snapshot["definitionCommit"] == TIP and snapshot["executedDefinition"] == "unknown"
    [workflow] = snapshot["workflows"]
    entry = git.files[TIP][workflow["path"]]
    assert workflow["definitionBlob"] == entry["sha"]
    assert workflow["definition"].encode() == git.blob(entry["sha"])
    run = next(run for run in snapshot["runs"] if run["headSha"] == TIP)
    assert run["commitProvenance"] == "verifiedRepositoryCommit"
    assert (run["id"], run["attempt"], run["conclusion"]) == (44723025441, 1, "cancelled")
    question = "What was the conclusion of the selected GitHub Actions attempt?"
    attempt = (run["id"], run["attempt"])
    answer = command(bundle, data, "ask", question, attempt=attempt)
    save(report, "actions-answer.json", answer)
    assert answer["outcome"] == "answered" and answer["mode"] == "selected"
    assert "cancelled" in answer["text"].lower()
    [citation] = answer["citations"]
    assert citation["quote"] == "Conclusion: cancelled"
    assert citation["workflowRun"] == {
        "repositoryId": REPOSITORY_ID, "workflowId": workflow["id"],
        "runId": attempt[0], "attempt": attempt[1], "reportedHead": TIP,
        "commitProvenance": "verifiedRepositoryCommit",
        "runUrl": f"https://github.com/{REPOSITORY}/actions/runs/{attempt[0]}/attempts/{attempt[1]}",
    }
    before = api("GET", "/v1/status")[0]["outboundAttempts"]
    natural, _ = api("POST", "/v1/answers", {
        "query": f"How did the Quality workflow run {attempt[0]} conclude?",
    })
    assert natural["outcome"] == "answered" and "cancelled" in natural["text"].lower()
    assert any(item == citation for item in natural["citations"])
    assert api("GET", "/v1/status")[0]["outboundAttempts"] == before
    save(report, "unscoped-actions.json", natural)
    absent = command(bundle, data, "ask", question, attempt=(attempt[0], 5))
    assert absent["outcome"] == "insufficientEvidence" and absent["citations"] == []
    unsupported = command(
        bundle, data, "ask", "What runner operating system executed this attempt?", attempt=attempt,
    )
    save(report, "actions-abstention.json", unsupported)
    assert unsupported["outcome"] == "insufficientEvidence" and unsupported["citations"] == []


def judge_graph(bundle, data, git, api, report, *, timeout=30):
    state, _ = api("GET", "/v1/github")
    api("PUT", "/v1/github/repositories", {
        "revision": state["revision"], "repository": REPOSITORY, "expectedId": REPOSITORY_ID,
        "branch": "main", "intervalMinutes": 60,
        "features": {**FEATURES, "history": True, "actions": True, "codeGraph": True}})
    root, symbol = "backend/server/src/lib.rs", "valid_branch"
    query = urlencode({"branch": "main", "root": root, "symbol": symbol})
    route = f"/v1/github/repositories/{REPOSITORY_ID}/graph?{query}"
    def publication():
        state, _ = api("GET", route)
        return state if state.get("snapshot") else None
    state = wait_until(timeout, publication)
    save(report, "graph-lookup.json", state)
    assert state["access"] == "available" and state["snapshot"]["revision"] == TIP
    assert state["snapshot"]["coverage"] == "published"
    hit = next(hit for hit in state["snapshot"]["hits"]
               if hit["role"] == "call" and hit["site"]["path"] == "backend/server/src/application/mod.rs")
    assert hit["resolution"] == "resolved"
    assert hit["target"]["path"] == RATE_SOURCE
    for locator in (hit["site"], hit["target"]):
        entry = git.files[TIP][locator["path"]]
        assert locator["fileUrl"] == f"https://github.com/{REPOSITORY}/blob/{TIP}/{locator['path']}"
        assert locator["blob"] == entry["sha"]
        source = git.blob(entry["sha"])
        start, end = locator["startByte"], locator["endByte"]
        assert source[start:end].decode() == symbol
        assert locator["startLine"] == locator["endLine"] == source[:start].count(b"\n") + 1
    cli = bundle / "Contents/MacOS/ctxfab"
    result = subprocess.check_output(
        [cli, "github-graph", str(REPOSITORY_ID), "--branch", "main", "--root", root, "--symbol", symbol],
        env={**client_environment(), "CTXFAB_DATA_DIRECTORY": str(data)}, timeout=10,
    )
    assert json.loads(result) == state


def verify_retained_evidence(bundle, data, api, report):
    state, _ = api("GET", "/v1/github")
    api("DELETE", f"/v1/github/repositories/{REPOSITORY_ID}?revision={state['revision']}")
    answer = command(
        bundle, data, "ask", "What was the conclusion of the selected GitHub Actions attempt?",
        attempt=(44723025441, 1),
    )
    save(report, "retained-actions-answer.json", answer)
    assert answer["outcome"] == "answered"
    [citation] = answer["citations"]
    assert citation["evidence"]["access"] == "retained" and citation["quote"] == "Conclusion: cancelled"
    history = command(bundle, data, "ask", HISTORY_QUESTION, history_commit=TIP)
    save(report, "retained-history-answer.json", history)
    assert history["outcome"] == "answered"
    assert len(history["citations"]) == 2
    assert all(citation["evidence"]["access"] == "retained" for citation in history["citations"])
    published_history = json.loads((report / "history-answer.json").read_text())
    assert [citation["lineUrl"] for citation in history["citations"]] == [
        citation["lineUrl"] for citation in published_history["citations"]
    ]


def run(bundle, repo, assets, report):
    evidence_only = os.getenv("CTXFAB_M10_EVIDENCE_ONLY", "")
    if evidence_only not in ("", "history", "actions", "hybrid"):
        raise ValueError(f"Unknown GitHub evidence selection: {evidence_only!r}")
    lineage = subprocess.check_output([
        "git", "-C", repo, "rev-list", "--first-parent", "--max-count=16", TIP,
    ]).decode().split()
    git = GitHistory(repo, list(dict.fromkeys([BASE, *lineage])))
    hybrid = evidence_only == "hybrid"
    with TemporaryDirectory() as directory, origin(git) as (port, requests):
        data = Path(directory) / "data"
        data.mkdir(mode=0o700)
        inference = data / "inference"
        inference.mkdir()
        model_files = ["Qwen3-4B-Instruct-2507-Q4_K_M.gguf"]
        if hybrid:
            model_files.append("Qwen3-Embedding-0.6B-Q8_0.gguf")
        for name in model_files:
            shutil.copyfile(assets / name, inference / name)
        # Keep qualification on loopback even if a staged weight is invalid.
        with backend(bundle, data, port, report, hub_port=port, preview_runtime=False) as api:
            prepare_answers(api, report, embeddings=hybrid, expect_download=False)
            assert not requests
            api("PUT", "/v1/github/credential", {"revision": 0, "token": FIXTURE_TOKEN})
            if evidence_only in ("history", "actions"):
                git.tip = TIP
                api("PUT", "/v1/github/repositories", {
                    "revision": 1, "repository": REPOSITORY, "expectedId": REPOSITORY_ID,
                    "branch": "main", "intervalMinutes": 60,
                    "features": {**FEATURES, "currentCode": False}})
                if evidence_only == "history":
                    judge_history(bundle, data, git, api, report, current_code=False)
                else:
                    judge_actions(bundle, data, git, api, report, current_code=False)
                assert api("GET", "/v1/status")[0]["outboundAttempts"] == len(requests)
                (report / "origin.log").write_text("\n".join(requests) + "\n", encoding="utf-8")
                return
            api("PUT", "/v1/github/repositories", {
                "revision": 1, "repository": REPOSITORY, "expectedId": REPOSITORY_ID,
                "branch": "main", "intervalMinutes": 60, "features": FEATURES})
            wait_for_commit(api, BASE)
            base = read_manifest(data, git, BASE)
            first_requests = list(requests)
            git.tip = TIP
            result, trace = api("POST", f"/v1/github/repositories/{REPOSITORY_ID}/refresh")
            assert result == {"commit": TIP, "changed": True}
            wait_for_commit(api, TIP)
            tip = read_manifest(data, git, TIP)
            second_requests = requests[len(first_requests):]
            base_blobs = {entry["sha"] for path, entry in git.files[BASE].items()
                          if path not in base["skipped"]}
            second_blobs = {path.rsplit("/", 1)[-1] for path in second_requests if "/git/blobs/" in path}
            (report / "origin.log").write_text("\n".join(requests) + "\n", encoding="utf-8")
            assert second_blobs and not second_blobs.intersection(base_blobs), (
                "Incremental refresh fetched an already retained Git blob.",
                sorted(second_blobs.intersection(base_blobs)),
            )
            assert api("GET", "/v1/status")[0]["outboundAttempts"] == len(requests)
            assert trace
            record_trace(api, trace, "github_current_code", report, "refresh-trace.json")
            save(report, "snapshots.json", {"repository": REPOSITORY, "repositoryId": REPOSITORY_ID,
                                            "base": base, "tip": tip,
                                            "initialRequests": len(first_requests),
                                            "incrementalRequests": len(second_requests),
                                            "incrementalBlobReads": len(second_blobs)})
            if hybrid:
                started = time.monotonic()
                def index_ready():
                    result, _ = api("GET", "/v1/retrieval")
                    return result if result["state"] == "ready" and 0 < result["indexed"] == result["total"] else None
                progress = wait_until(300, index_ready)
                save(report, "embedding-index.json", {
                    **progress, "waitMilliseconds": round((time.monotonic() - started) * 1000),
                })
                attempts_before_queries = len(requests)
                rankings = evaluate(
                    lambda question, mode, branch: command(
                        bundle, data, "search", question, branch=branch, mode=mode,
                    ),
                    lambda evidence: verify_evidence(git, evidence),
                    lambda path: git.blob(git.files[TIP][path]["sha"]),
                )
                save(report, "retrieval-evaluation.json", rankings)
                judge_answers(bundle, data, git, report, mode="hybrid")
                verify_unscoped_french_cooldown(api, git, report)
                verify_http_throttle_control(api, git, report)
                assert api("GET", "/v1/status")[0]["outboundAttempts"] == len(requests) == attempts_before_queries
                return
            judge_answers(bundle, data, git, report)
            judge_history(bundle, data, git, api, report)
            judge_actions(bundle, data, git, api, report)
            judge_graph(bundle, data, git, api, report)
            verify_retained_evidence(bundle, data, api, report)
            assert api("GET", "/v1/status")[0]["outboundAttempts"] == len(requests)
            (report / "origin.log").write_text("\n".join(requests) + "\n", encoding="utf-8")


if __name__ == "__main__":
    run(*map(Path, sys.argv[1:]))
