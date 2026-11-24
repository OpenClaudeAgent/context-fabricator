"""Loopback wire fixture for the macOS client's generated inference setup operations."""

import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from socketserver import TCPServer

SETUP = "/v1/inference/setup"
FINGERPRINT = "a" * 64
PROFILE = "qwen3-4b-instruct-2507-q4-k-m-3605803b-v1"
IDENTIFIER = "e87f440f-05e4-46b9-899e-e01f47b08011"
SELECTION = {"answerProfile": PROFILE, "embedding": True, "runtimeSource": "localArchive"}


class SetupHandler(BaseHTTPRequestHandler):
    def respond(self, status, payload):
        result = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(result)))
        self.end_headers()
        self.wfile.write(result)

    def problem(self, status, code, message):
        self.respond(status, {"code": code, "message": message})

    def authorized(self):
        if self.headers.get("Authorization") == "Bearer swift-setup-fixture":
            return True
        self.problem(401, "unauthorized", "A local session credential is required.")
        return False

    def do_GET(self):
        if not self.authorized():
            return
        if self.path != SETUP or not self.server.completed:
            self.problem(404, "inference_setup_not_started", "No inference setup has been confirmed.")
            return
        self.respond(200, {"operationId": IDENTIFIER, "phase": "completed", "answerReady": True,
                           "selection": SELECTION})

    def do_POST(self):
        if not self.authorized():
            return
        content = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == SETUP + "/plan" and content == SELECTION:
            self.respond(200, {"selection": SELECTION, "fingerprint": FINGERPRINT,
                               "peakAdditionalBytes": 512, "maximumTransferBytes": 256,
                               "availableBytes": 1024, "canStart": True, "runtimeRequired": True,
                               "answerInstalled": False, "embeddingInstalled": False})
            return
        if self.path != SETUP or content.get("selection") != SELECTION:
            self.problem(400, "invalid_inference_setup", "Choose a supported curated answer model.")
        elif content.get("fingerprint") != FINGERPRINT:
            self.problem(409, "stale_inference_setup_plan", "Review an updated installation estimate.")
        elif content.get("operationId", "").lower() != IDENTIFIER or content.get("archivePath") != "/fixture/pinned.zip":
            self.problem(400, "invalid_inference_setup", "Choose the reviewed runtime ZIP.")
        else:
            self.server.completed = True
            self.respond(202, {"operationId": IDENTIFIER, "phase": "runtime", "answerReady": False,
                               "selection": SELECTION})


class LoopbackHTTPServer(HTTPServer):
    def server_bind(self):
        TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address


with LoopbackHTTPServer(("127.0.0.1", 0), SetupHandler) as server:
    server.completed = False
    port_file = Path(sys.argv[1])
    staged = port_file.with_suffix(".pending")
    staged.write_text(str(server.server_port), encoding="utf-8")
    staged.replace(port_file)
    server.serve_forever()
