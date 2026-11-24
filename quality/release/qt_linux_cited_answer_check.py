"""Qualify a cited answer from the installed Qt application's bundled CPU runtime."""

import argparse
import json
from pathlib import Path
import re
import sys
import time

from linux_runtime_check import adopt_weights
from runtime_fixture import GENERATION_PROFILE, request


ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "desktop/qt/tests"))

from linux_window_smoke import (
    INSTALLED_APP, INSTALLED_BACKEND, INSTALLED_CLI, check_isolation, verify_window,
)
from run_window_scenarios import seed_corpus


QUESTION = "What is the retry cooldown?"
QUOTE = "The retry cooldown is 47 seconds."


def read_trace(session, trace_id):
    assert re.fullmatch(r"[a-f0-9]{32}", trace_id), "Missing saved trace identifier."
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        status, trace = request(session, f"/v1/telemetry/traces/{trace_id}")
        if status == 200:
            spans = {span["name"]: span for span in trace["spans"]}
            if {"api_answer", "answer_generation", "answer_validation"} <= spans.keys():
                return spans
        time.sleep(0.1)
    raise AssertionError("The backend did not publish the answer, generation and validation spans.")


def verify_answer(session, profile, weights):
    adopt_weights(session, profile, weights)
    print("The packaged backend verified the catalog weights offline.", flush=True)
    status, models = request(session, "/v1/generation/models",
                             {"profile": GENERATION_PROFILE}, method="PUT", timeout=120)
    assert status == 200 and models.get("activeProfile") == GENERATION_PROFILE, (status, models)
    seed_corpus(profile / "context.sqlite3")
    status, settings = request(session, "/v1/answer-history/settings")
    assert status == 200
    status, saved = request(session, "/v1/answer-history/settings", {
        **settings, "enabled": True,
    }, method="PUT")
    assert status == 200 and saved["enabled"], saved

    start = time.monotonic()
    status, answer = request(session, "/v1/answers", {"query": QUESTION}, timeout=120)
    elapsed = round((time.monotonic() - start) * 1000)
    assert status == 200 and answer["outcome"] == "answered", (status, answer)
    assert answer["model"] == GENERATION_PROFILE and answer["mode"] == "bm25", answer
    assert "47 seconds" in answer["text"], answer
    assert any(
        citation["evidence"]["source"] == "src/outbound.rs"
        and citation["evidence"]["revision"] == "r-native"
        and citation["quote"] == QUOTE
        for citation in answer["citations"]
    ), answer["citations"]

    status, history = request(session, "/v1/answer-history")
    assert status == 200 and len(history["entries"]) == 1, history
    identifier = history["entries"][0]["id"]
    status, saved = request(session, f"/v1/answer-history/{identifier}")
    assert status == 200 and saved["question"] == QUESTION, saved
    for field in ("outcome", "model", "text", "citations"):
        assert saved["answer"][field] == answer[field], field
    spans = read_trace(session, saved["traceId"])
    assert all(span["traceId"] == saved["traceId"] for span in spans.values())
    assert spans["api_answer"]["modelProfile"] == GENERATION_PROFILE
    assert spans["answer_validation"]["spanOutcome"] == "succeeded"
    assert spans["answer_generation"]["inferenceTiming"]["generatedTokens"] > 0
    status, service = request(session, "/v1/status")
    assert status == 200 and service["outboundAttempts"] == 0, service
    print(json.dumps({
        "profile": GENERATION_PROFILE,
        "traceId": saved["traceId"],
        "elapsedMilliseconds": elapsed,
        "outboundAttempts": 0,
        "generatedTokens": spans["answer_generation"]["inferenceTiming"]["generatedTokens"],
        "citation": {"source": "src/outbound.rs", "revision": "r-native", "quote": QUOTE},
    }, indent=2), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("weights", type=Path, help="Local, catalog-pinned Qwen3 Q4_K_M weights")
    parser.add_argument("--build", required=True, help="Pinned llama.cpp build number")
    args = parser.parse_args()
    check_isolation()
    weights = args.weights.resolve(strict=True)
    verify_window(INSTALLED_APP, INSTALLED_BACKEND, INSTALLED_CLI, args.build,
                  inspect_session=lambda session, profile: verify_answer(session, profile, weights))


if __name__ == "__main__":
    main()
