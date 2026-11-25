#!/usr/bin/env python3
"""Small llama.cpp protocol double for process and grounding regression tests."""

import json
import os
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

args = sys.argv[1:]


def argument(name):
    return args[args.index(name) + 1]


model = Path(argument("--model"))
secret = "Bearer " + Path(argument("--api-key-file")).read_text().strip()
alias = argument("--alias")
if model.read_bytes().startswith(b"fixture model boot failure"):
    sys.exit(9)
model.with_suffix(".worker-pid").write_text(str(os.getpid()))
startup_gate = model.with_suffix(".startup-gate")
if startup_gate.is_file():
    deadline = time.monotonic() + 20
    while startup_gate.is_file():
        if time.monotonic() >= deadline:
            sys.exit("Fixture startup gate was not released")
        time.sleep(0.025)


class Worker(BaseHTTPRequestHandler):
    def reply(self, value, code=200):
        body = json.dumps(value).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def authenticated(self):
        if self.headers.get("Authorization") == secret:
            return True
        self.reply({}, 401)
        return False

    def do_GET(self):
        if self.authenticated():
            capacity = model.with_suffix(".context")
            context = int(capacity.read_text()) if capacity.exists() else 4_096
            self.reply({"data": [{"id": alias, "meta": {"n_ctx": context}}]})

    def do_POST(self):
        if not self.authenticated():
            return
        payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/v1/embeddings" and model.read_bytes() == b"formulation fixture":
            self.reply({"model": alias, "data": [{"index": 0, "embedding": [1.0] + [0.0] * 1023}]})
            return
        instructions = payload["messages"][0]["content"]
        if "Translate the engineering search question into natural English" in instructions:
            question = payload["messages"][1]["content"]
            if question.startswith("Attendez avant"):
                model.with_suffix(".formulation-in-flight").touch()
                time.sleep(30)
            query = ""
            if question.startswith("Reformulez la question"):
                query = "fixture retry cooldown"
            elif question.startswith("Quel délai de règlement"):
                query = "settlement delay"
            content = "" if question.startswith("Échouez à reformuler") else json.dumps({"query": query})
            self.reply({"model": alias, "choices": [{
                "finish_reason": "stop", "message": {"content": content},
            }]})
            return
        prompt = json.loads(payload["messages"][1]["content"])
        question = prompt["question"]
        if self.path == "/v1/chat/completions/input_tokens":
            size = 1_024
            if question.startswith("History context tight") and "changes" in prompt:
                size = 3456 if any("unchangedGroup" in line for pair in prompt["changes"]
                                   for line in pair["before"]["lines"]) else 3455
            if question.startswith("Pack "):
                paths = [p["source"]["path"].rsplit("/", 1)[-1] for p in prompt["passages"]]
                with model.with_suffix(".packing").open("a") as events:
                    events.write(",".join(paths) + "\n")
                exceeds = ("oversized" in question or ("unpressured" not in question and (
                    len(paths) == 4 or ("packing-detail.rs" in paths and (
                        len(paths) == 3 or "fallback" in question
                    ))
                )))
                size = 3456 if exceeds else 3455
                Worker.measured = payload
            if question.startswith("Budget"):
                slots = len(prompt["passages"])
                with model.with_suffix(".budget").open("a") as events:
                    events.write(f"measure:{slots}\n")
                size = 3_456 if slots > 1 or question.startswith("Budget none") else 3_455
                Worker.measured = payload
            self.reply({"object": "response.input_tokens", "input_tokens": size})
            return
        if question.startswith("Budget"):
            with model.with_suffix(".budget").open("a") as events:
                events.write(f"generate:{len(prompt['passages'])}\n")
            if getattr(Worker, "measured", payload) != payload:
                self.reply({}, 400)
                return
            model.with_suffix(".completion-fixture.json").write_text(
                payload["messages"][1]["content"], encoding="utf-8"
            )
        if question.startswith("Pack ") and getattr(Worker, "measured", None) != payload:
            self.reply({}, 400)
            return
        if "History context" in question and "changes" in prompt:
            model.with_suffix(".history-prompt.json").write_text(
                payload["messages"][1]["content"], encoding="utf-8"
            )
        if question.startswith("Crash"):
            os._exit(12)
        if question.startswith(("Delay", "Hold")):
            model.with_suffix(".in-flight").touch()
            gate = model.with_suffix(".response-gate")
            if gate.exists():
                deadline = time.monotonic() + 10
                while gate.exists():
                    if time.monotonic() >= deadline:
                        sys.exit("Fixture response gate was not released")
                    time.sleep(0.025)
            else:
                time.sleep(30 if question.startswith("Hold") else 0.8)
        if question.startswith("Quel délai de règlement"):
            draft = settlement_draft(prompt, "analysis field" in instructions)
        else:
            draft = draft_for(prompt)
        if question.startswith("Uncited"):
            draft["citations"] = []
        if question.startswith("Budget dropped"):
            draft["citations"][0]["passage"] = 2
        if question.startswith("Bad"):
            draft = {"answer": 1234, "citations": []}
        self.reply({"model": alias, "choices": [{
            "finish_reason": "stop", "message": {"content": json.dumps(draft)}
        }], "timings": {
            "cache_n": 128, "prompt_n": 256, "predicted_n": 18,
            "prompt_ms": 0.125, "predicted_ms": 0.25, "extra": "private-runtime-fixture",
        }})


def settlement_draft(prompt, linked):
    declaration = {"passage": 1, "quote": "pub const SETTLEMENT_DELAY_MS: u64 = 1_000;"}
    usage = {"passage": 2, "quote": "let retry = now.saturating_add(SETTLEMENT_DELAY_MS);"}
    if not linked:
        anchor = next((p for p in prompt["passages"] if declaration["quote"] in p["text"]), None)
        if anchor is None:
            return {"answer": "", "citations": []}
        return {"answer": "1000 ms", "citations": [{**declaration, "passage": anchor["id"]}]}
    assert len(prompt["passages"]) == 2
    assert declaration["quote"] in prompt["passages"][0]["text"]
    assert usage["quote"] in prompt["passages"][1]["text"]
    if "Brouillon invalide" in prompt["question"]:
        return {"answer": 1234, "citations": []}
    marker = model.with_suffix(".linked-in-flight")
    marker.touch()
    gate = model.with_suffix(".linked-response-gate")
    deadline = time.monotonic() + 10
    while gate.exists() and time.monotonic() < deadline:
        time.sleep(0.025)
    refs = [declaration] if "partielle" in prompt["question"] else [declaration, usage]
    return {"analysis": "The expired path uses the declared interval.", "answer": "1 seconde", "citations": refs}


def draft_for(prompt):
    question = prompt["question"]
    if question.startswith("Pack "):
        quote = ('const COLOR: &str = "crème brûlée";' if "fallback" not in question
                 else "// violet packing fallback subset.")
        anchor = next((source for source in prompt["passages"] if quote in source["text"]), None)
        if anchor is None:
            return {"answer": "", "citations": []}
        return {"answer": quote, "citations": [{"passage": anchor["id"], "quote": quote}]}
    if "changes" in prompt:
        pair = prompt["changes"][0]
        if question.startswith("Unsupported"):
            return {"supported": False, "pair": 0, "beforeLine": 0, "afterLine": 0, "answer": ""}
        if "History context" in question:
            has_context = any("unchangedGroup" in line for line in pair["before"]["lines"])
            if "tight" in question:
                before, after, answer = 7, 7, "The payload writer note changed."
                kind = "changed"
            else:
                before = 1 if "outside" in question else 8
                after = 9 if "twin" in question else before
                if "header" in question:
                    before = after = 9
                if "brace" in question:
                    before = after = 10
                answer = "Both revisions call write_all(header) in sender."
                kind = "unchanged" if "brace" in question else "changed"
            result = {"supported": True, "pair": pair["pair"], "beforeLine": before,
                      "afterLine": after, "answer": answer}
            if has_context or "header" in question or "opening" in question:
                result["evidenceKind"] = kind
            return result
        old, new = (next(line for line in pair[side]["lines"] if not line.get("context"))
                    for side in ("before", "after"))
        before = old["line"] if not question.startswith("Fabricate") else old["line"] + 9
        return {"supported": True, "pair": pair["pair"], "beforeLine": before,
                "afterLine": new["line"], "answer": f"Changed from {old['text']} to {new['text']}."}
    if question in ("What changed in modified.rs?", "Unsupported modified.rs timeout?"):
        history = next(item for item in prompt["passages"] if item["kind"] == "history")
        # The general quote schema may provide an unsupported or imprecise comparison.
        # Only the independent before/after line selection can recover this draft.
        return {"answer": "The line changed.", "citations": [
            {"passage": history["id"], "quote": "not an actual line"}]}
    if "History context" in question:
        history = next(item for item in prompt["passages"] if item["kind"] == "history")
        return {"answer": "Both revisions call write_all(payload) in sender.",
                "citations": [{"passage": history["id"], "quote": "write_all(payload);"}]}
    if "Alphaguardmarker" in question:
        passages = prompt["passages"]
        code = next(item for item in passages if "Alphaguardmarker" in item["text"])
        history = next(item for item in passages if item["kind"] == "history")
        run = next(item for item in passages if item["kind"] == "workflow")
        citations = [{"passage": code["id"], "quote": "Alphaguardmarker purgeguardprobe is private."},
                     {"passage": history["id"], "quote": "repeat"},
                     {"passage": history["id"], "quote": "changed"},
                     {"passage": run["id"], "quote": "Conclusion: success"}]
        if question.startswith("Omit"):
            del citations[1]
        return {"answer": "Alphaguardmarker is private. The line changed from repeat to changed. The attempt succeeded.",
                "citations": citations}
    if question == "graph_example":
        quote = "fn call(){crate::target::graph_example();}"
        source = next(item for item in prompt["passages"] if quote in item["text"])
        return {"answer": "The call is in src/lib.rs.",
                "citations": [{"passage": source["id"], "quote": quote}]}
    if question in ("Run 30 status", "What triggered Actions run 30 and what was its conclusion?"):
        source = next(item for item in prompt["passages"]
                      if item["kind"] == "workflow" and "Run ID: 30\n" in item["text"])
        if question != "Run 30 status":
            return {"answer": "A push triggered the run and it succeeded.",
                    "citations": [{"passage": source["id"],
                                   "quote": "Event: push\nStatus: completed\nConclusion: success"}]}
        return {"answer": "Run 30 was queued.",
                "citations": [{"passage": source["id"], "quote": "Status: queued"}]}
    if question == "What head branch and repository did GitHub report for fixture/alpha run 30 attempt 2?":
        source = next(item for item in prompt["passages"] if item["kind"] == "workflow")
        return {"answer": "The reported branch was main and the head repository ID was 99.",
                "citations": [{"passage": source["id"],
                               "quote": "Reported head branch: main\nReported head repository ID: 99"}]}
    if any(marker in question for marker in ("5827", "dock-sign", "seal-log")):
        if "dock-report.md" in question:
            quote = "Recorded dock-sign job outcome: success."
        elif "dock.yml" in question:
            quote = "run: echo dock-sign succeeded"
        else:
            quote = "Conclusion: failure"
        source = next((item for item in prompt["passages"] if quote in item["text"]), None)
        if source is None:
            return {"answer": "", "citations": []}
        return {"answer": quote, "citations": [{"passage": source["id"], "quote": quote}]}
    if "9431" in question:
        passages = prompt["passages"]
        exact_run = [p for p in passages if p["kind"] == "workflow"
                     and "Run ID: 9431\n" in p["text"]]
        parser_case = "parser" in question or "attempt 7" in question or "tentative 7" in question
        fallback = [p for p in passages if p["kind"] == "current"
                    and "The parcel parser fixture writes" in p["text"]]
        if parser_case and fallback:
            chosen = fallback[:1]
        elif question.startswith("Compare"):
            chosen = exact_run[:2]
        else:
            chosen = exact_run[:1]
        citations = [
            {"passage": p["id"], "quote": next(
                (line for line in p["text"].splitlines() if line.startswith("Conclusion: ")),
                "The parcel parser fixture writes run 9431 attempt 7 passed.")}
            for p in chosen
        ]
        return {"answer": ", ".join(c["quote"] for c in citations), "citations": citations}
    if "run 701" in question.lower() or "/actions/runs/701/attempts/" in question:
        passages = prompt["passages"]
        if "Compare" in question:
            chosen = [p for p in passages if p["kind"] == "workflow"
                      and "Run ID: 701\n" in p["text"]]
        elif "history decoy" in question:
            history = next(p for p in passages if p["kind"] == "history")
            return {"answer": "The file changed.",
                    "citations": [{"passage": history["id"], "quote": line}
                                  for line in ("repeat", "changed")]}
        elif "both sources" in question:
            chosen = [
                next(p for p in passages if p["kind"] == "workflow"),
                next(p for p in passages if p["kind"] == "current"
                     and "GitHub Actions run 701 attempt 8 concluded" in p["text"]),
            ]
        elif "decoy" in question:
            chosen = [p for p in passages if "GitHub Actions run 701 attempt 8 concluded" in p["text"]][:1]
        else:
            chosen = [p for p in passages if p["kind"] == "workflow"][:1]
        citations = [
            {"passage": p["id"], "quote": next(
                (line for line in p["text"].splitlines() if line.startswith("Conclusion: ")),
                "GitHub Actions run 701 attempt 8 concluded with success.")}
            for p in chosen
        ]
        return {"answer": ", ".join(c["quote"] for c in citations), "citations": citations}
    excerpt = prompt["passages"][0]
    if question == "Driftmarker conflicting revision citation":
        quote = "Driftmarker waits 205 ms."
        return {"answer": quote, "citations": [{"passage": excerpt["id"], "quote": quote}]}
    if "purgeguardprobe" in question:
        excerpt = next(item for item in prompt["passages"] if "Betaguardmarker" in item["text"])
    if prompt.get("evidenceFamily") == "githubActions":
        quote = next(line for line in excerpt["text"].splitlines() if line.startswith("Conclusion: "))
    else:
        quote = excerpt["text"].split(". ")[0].rstrip(".") + "."
    answer = "The configured wait is 47 seconds." if question == "What is the retry delay?" else quote
    return {"answer": answer, "citations": [{"passage": excerpt["id"],
             "quote": "This citation was invented." if question.startswith("Fabricate") else quote}]}


with HTTPServer(("127.0.0.1", int(argument("--port"))), Worker) as server:
    server.serve_forever()
