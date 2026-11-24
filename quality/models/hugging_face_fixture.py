#!/usr/bin/env python3
"""Loopback Hugging Face Hub used by the native catalog scenario."""

import hashlib
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from socketserver import TCPServer
from threading import Event
from urllib.parse import parse_qs, urlsplit

REPOSITORY = "fixture/catalog-qwen"
REVISION = "a" * 40
FILENAME = "Qwen3-4B-Instruct-2507-Q4_K_M.gguf"
WEIGHTS = b"GGUF loopback model catalog fixture\n"
Q8_FILENAME = "Qwen3-4B-Instruct-2507-Q8_0.gguf"
Q8_WEIGHTS = b"GGUF loopback Q8 retry and progress fixture\n"
q8_retry_allowed = Event()
q8_released = Event()
Q5_BYTES = 2_889_514_080
Q5_CHUNK = 1024 * 1024
Q5_ETAG = '"context-fabricator-q5-ui-fixture"'
q5_released = Event()
MODEL = {
    "id": REPOSITORY,
    "sha": REVISION,
    "private": False,
    "gated": False,
    "pipeline_tag": "text-generation",
    "gguf": {"architecture": "qwen3"},
    "baseModels": {
        "relation": "quantized",
        "models": [{"id": "Qwen/Qwen3-4B-Instruct-2507"}],
    },
}
TREE = [{
    "type": "file",
    "path": FILENAME,
    "size": len(WEIGHTS),
    "lfs": {"oid": hashlib.sha256(WEIGHTS).hexdigest(), "size": len(WEIGHTS)},
}]


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        if setup_q5 and self.path == "/__ui/release-q5":
            q5_released.set()
            self.send_response(204)
            self.end_headers()
        elif retry_q8 and self.path == "/__test/allow-q8-retry":
            q8_retry_allowed.set()
            self.send_response(204)
            self.end_headers()
        elif retry_q8 and self.path == "/__test/release-q8":
            q8_released.set()
            self.send_response(204)
            self.end_headers()
        else:
            self.send_error(404)

    def do_GET(self):
        request = urlsplit(self.path)
        with (directory / "hits.log").open("a", encoding="utf-8") as hits:
            hits.write(f"GET {request.path}\n")
        path = request.path
        if path == "/api/models":
            term = parse_qs(request.query).get("search", [""])[0]
            self.send_json(200, [MODEL] if term == "catalog" else [])
        elif path == f"/api/models/{REPOSITORY}/revision/{REVISION}":
            self.send_json(200, MODEL)
        elif path == f"/api/models/{REPOSITORY}/tree/{REVISION}":
            extra = [{
                "type": "file",
                "path": Q8_FILENAME,
                "size": len(Q8_WEIGHTS),
                "lfs": {
                    "oid": hashlib.sha256(Q8_WEIGHTS).hexdigest(),
                    "size": len(Q8_WEIGHTS),
                },
            }] if retry_q8 else []
            self.send_json(200, TREE + extra)
        elif setup_q5 and path == (
            "/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/"
            "a06e946bb6b655725eafa393f4a9745d460374c9/"
            "Qwen3-4B-Instruct-2507-Q5_K_M.gguf"
        ):
            self.send_partial_q5()
        elif path == f"/{REPOSITORY}/resolve/{REVISION}/{FILENAME}":
            self.send_payload(200, "application/octet-stream", WEIGHTS)
        elif retry_q8 and path == f"/{REPOSITORY}/resolve/{REVISION}/{Q8_FILENAME}":
            self.send_q8()
        else:
            self.send_json(404, {"error": "Unknown fixture request"})

    def send_json(self, status, payload):
        self.send_payload(status, "application/json", json.dumps(payload).encode())

    def send_payload(self, status, content_type, payload):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def send_q8(self):
        if not q8_retry_allowed.is_set():
            self.send_json(403, {"error": "Fixture transfer refused"})
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(Q8_WEIGHTS)))
        self.end_headers()
        self.wfile.write(Q8_WEIGHTS[:5])
        self.wfile.flush()
        if q8_released.wait(timeout=20):
            self.wfile.write(Q8_WEIGHTS[5:])

    def send_partial_q5(self):
        resume = self.headers.get("Range") == f"bytes={Q5_CHUNK}-"
        if resume and self.headers.get("If-Range") != Q5_ETAG:
            self.send_json(412, {"error": "Missing resume validator"})
            return
        offset = Q5_CHUNK if resume else 0
        self.send_response(206 if resume else 200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("ETag", Q5_ETAG)
        self.send_header("Content-Length", str(Q5_BYTES - offset))
        if resume:
            self.send_header("Content-Range", f"bytes {offset}-{Q5_BYTES - 1}/{Q5_BYTES}")
        self.end_headers()
        if not resume and not q5_released.wait(timeout=30):
            return
        self.wfile.write(b"\0" * Q5_CHUNK)
        self.wfile.flush()
        # Keep the transfer open for the native pause or discard control.
        time.sleep(90)


class LoopbackHTTPServer(ThreadingHTTPServer):
    def server_bind(self):
        TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address


directory = Path(sys.argv[1])
setup_q5 = "--setup-q5" in sys.argv[2:]
retry_q8 = "--download-retry" in sys.argv[2:]
with LoopbackHTTPServer(("127.0.0.1", 0), Handler) as server:
    port_file = directory / "port"
    pending = port_file.with_suffix(".pending")
    pending.write_text(str(server.server_port), encoding="ascii")
    pending.replace(port_file)
    server.serve_forever()
