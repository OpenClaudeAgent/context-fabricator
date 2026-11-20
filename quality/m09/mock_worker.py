#!/usr/bin/env python3
"""Small llama.cpp protocol double for process and grounding regression tests."""

import json
import os
from pathlib import Path
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

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
            self.reply({"data": [{"id": alias}]})

    def do_POST(self):
        if not self.authenticated():
            return
        payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        prompt = json.loads(payload["messages"][1]["content"])
        question = prompt["question"]
        excerpt = prompt["passages"][0]
        if question.startswith("Crash"):
            os._exit(12)
        if question.startswith(("Delay", "Hold")):
            model.with_suffix(".in-flight").touch()
            time.sleep(30 if question.startswith("Hold") else 0.8)
        quote = excerpt["text"].split(". ")[0].rstrip(".") + "."
        draft = {"answer": quote, "citations": [{"passage": excerpt["id"], "quote": quote}]}
        if question.startswith("Fabricate"):
            draft["citations"][0]["quote"] = "This citation was invented."
        if question.startswith("Bad"):
            draft = {"answer": 1234, "citations": []}
        self.reply({"model": alias, "choices": [{
            "finish_reason": "stop", "message": {"content": json.dumps(draft)}
        }]})


with HTTPServer(("127.0.0.1", int(argument("--port"))), Worker) as server:
    server.serve_forever()
