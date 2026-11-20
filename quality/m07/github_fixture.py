#!/usr/bin/env python3
"""Loopback GitHub read-only fixture used through the real backend HTTP adapter."""

import base64
import hashlib
import json
import os
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

ALPHA = {"id": 41, "full_name": "fixture/alpha", "default_branch": "main", "private": True}
BETA = {"id": 42, "full_name": "fixture/beta", "default_branch": "trunk", "private": True}
PREFIX = "/repos/fixture/alpha"
COMMIT_A, COMMIT_B = "a" * 40, "b" * 40
ROOT_A, ROOT_B, SRC_A, SRC_B = "1" * 40, "2" * 40, "3" * 40, "4" * 40
SHARED = b"fn paced_source_fixture() {}\n"
REMOVED = b"fn removed_source_fixture() {}\n"
NEW = b"fn newly_collected_fixture() {}\n"
BETA_SOURCE = b"fn paced_source_fixture() { assert!(true); }\n"


def blob_id(content):
    return hashlib.sha1(f"blob {len(content)}\0".encode() + content).hexdigest()


def blob_entry(path, content):
    return {"path": path, "type": "blob", "mode": "100644",
            "sha": blob_id(content), "size": len(content)}


def collect_route(path):
    if path in references:
        return references[path]
    if path == "/repos/fixture/beta/branches/trunk":
        return 200, {"name": "trunk", "commit": {"sha": "c" * 40,
                                                "commit": {"tree": {"sha": "7" * 40}}}}
    if path == "/repos/fixture/beta/git/trees/" + "7" * 40:
        return 200, {"sha": "7" * 40, "truncated": False, "tree": [blob_entry("old.rs", BETA_SOURCE)]}
    if path == "/repos/fixture/beta/git/blobs/" + blob_id(BETA_SOURCE):
        return 200, {"sha": blob_id(BETA_SOURCE), "size": len(BETA_SOURCE), "encoding": "base64",
                     "content": base64.encodebytes(BETA_SOURCE).decode()}
    if path == PREFIX + "/branches":
        return (403, {"message": "Resource not accessible"}) if (directory / "deny-contents").exists() else (200, [])
    if (directory / "deny-contents").exists() and (
        path.startswith(PREFIX + "/git/") or path.startswith(PREFIX + "/branches/")
    ):
        return 403, {"message": "Resource not accessible"}
    if path == PREFIX + "/branches/main":
        advanced = (directory / "advance-code").exists()
        if os.getenv("CTXFAB_GITHUB_UI_ADVANCE") == "1":
            # The UI first verifies access and then collects. The next branch
            # read models a change that became visible when the user refreshed.
            advanced = hits.read_text().splitlines().count(f"GET {path}") >= 3
        commit, root = (COMMIT_B, ROOT_B) if advanced else (COMMIT_A, ROOT_A)
        return 200, {"name": "main", "commit": {"sha": commit, "commit": {"tree": {"sha": root}}}}
    if path.startswith(PREFIX + "/git/trees/"):
        oid = path.rsplit("/", 1)[1]
        trees = {
            ROOT_A: [
                {"path": "src", "type": "tree", "mode": "040000", "sha": SRC_A},
                {"path": "tests", "type": "tree", "mode": "040000", "sha": "5" * 40},
                {"path": "vendor", "type": "tree", "mode": "040000", "sha": "6" * 40},
            ],
            ROOT_B: [{"path": "src", "type": "tree", "mode": "040000", "sha": SRC_B}],
            SRC_A: [blob_entry("old.rs", SHARED), blob_entry("removed.rs", REMOVED)],
            SRC_B: [blob_entry("renamed.rs", SHARED), blob_entry("z-new.rs", NEW)],
            "5" * 40: [],
            "6" * 40: [],
        }
        if oid in trees:
            return 200, {"sha": oid, "truncated": False, "tree": trees[oid]}
    for content in (SHARED, REMOVED, NEW):
        oid = blob_id(content)
        if path == PREFIX + "/git/blobs/" + oid:
            if content == NEW and (directory / "fail-code").exists():
                return 503, {}
            return 200, {"sha": oid, "size": len(content), "encoding": "base64",
                         "content": base64.encodebytes(content).decode()}
    return None


def pinned_routes(root):
    routes = {}
    if not root:
        return routes
    manifest = json.loads(Path(__file__).with_name("reference-corpora.json").read_text())
    for reference in manifest:
        content = (Path(root) / reference["file"]).read_bytes()
        oid = blob_id(content)
        if oid != reference["blob"]:
            raise ValueError("Reference bytes do not match their pinned Git blob")
        base = "/repos/" + reference["repository"]
        paths = reference["path"].split("/")
        tree_ids = [hashlib.sha1(f"{base}:{depth}".encode()).hexdigest()
                    for depth in range(len(paths))]
        repo = {"id": reference["repositoryId"], "full_name": reference["repository"],
                "default_branch": "fixture", "private": False}
        routes[base] = (200, repo)
        routes[base + "/branches/fixture"] = (200, {
            "name": "fixture", "commit": {"sha": reference["commit"],
                                            "commit": {"tree": {"sha": tree_ids[0]}}}})
        for depth, tree in enumerate(tree_ids):
            if depth == len(paths) - 1:
                entry = blob_entry(paths[depth], content)
            else:
                entry = {"path": paths[depth], "type": "tree", "mode": "040000",
                         "sha": tree_ids[depth + 1]}
            routes[base + "/git/trees/" + tree] = (200, {"sha": tree, "truncated": False, "tree": [entry]})
        routes[base + "/git/blobs/" + oid] = (200, {
            "sha": oid, "size": len(content), "encoding": "base64",
            "content": base64.encodebytes(content).decode()})
    return routes


class GitHub(BaseHTTPRequestHandler):
    def log_message(self, _format, *_args):
        return

    def respond(self, status, body=None, headers=None):
        encoded = json.dumps(body).encode() if body is not None else b""
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        for name, value in (headers or {}).items():
            self.send_header(name, value)
        self.end_headers()
        if encoded:
            try:
                self.wfile.write(encoded)
            except (BrokenPipeError, ConnectionResetError):
                pass

    def do_GET(self):
        route = urlsplit(self.path)
        with hits.open("a", encoding="utf-8") as log:
            log.write(f"GET {self.path}\n")
        if self.headers.get("X-GitHub-Api-Version") != "2026-03-10":
            self.respond(400)
            return
        token = self.headers.get("Authorization")
        replacements = {"Bearer github_pat_fixture_replacement", "Bearer github_pat_r"}
        valid = {"Bearer github_pat_fixture_primary", *replacements}
        if token not in valid or (token.endswith("_primary") and expired.exists()):
            self.respond(401)
            return
        if route.path == PREFIX + "/git/blobs/" + blob_id(NEW) and (directory / "throttle-code").exists():
            self.respond(429, {}, {"Retry-After": "1"})
            return
        hold = directory / "hold-code-tree"
        if route.path == PREFIX + "/git/trees/" + ROOT_B and hold.exists():
            (directory / "held-code-tree").touch()
            deadline = time.monotonic() + 5
            while hold.exists() and time.monotonic() < deadline:
                time.sleep(0.01)
        if (candidate := collect_route(route.path)) is not None:
            self.respond(*candidate)
            return
        routes = {
            "/user": (200, {"login": "fixture-user"}),
            "/repos/fixture/alpha": (200, ALPHA),
            "/repos/fixture/beta": (200, BETA),
            "/repositories/41": (200, ALPHA),
            "/repos/fixture/broken": (200, {"id": 0}),
            "/repos/fixture/upstream-unavailable": (503, {}),

            "/repos/fixture/alpha/branches/main": (200, {}),
            "/repos/fixture/beta/branches/trunk": (200, {}),
            "/repos/fixture/beta/branches/release%2Fv1": (200, {}),
            "/repos/fixture/alpha/actions/runs": (200, {"workflow_runs": []}),
            "/repos/fixture/beta/actions/runs": (
                (200, {"workflow_runs": []}) if token in replacements
                else (403, {"message": "Resource not accessible"})
            ),
        }
        if route.path == "/user/repos":
            page = parse_qs(route.query).get("page", ["1"])[0]
            if page == "1":
                self.respond(200, [ALPHA], {"Link": '<https://api.github.com/user/repos?page=2>; rel="next"'})
            elif page == "3":
                self.respond(403, {"message": "secondary rate limit"}, {"Retry-After": "1"})
            elif page == "4":
                self.respond(429, {}, {"Retry-After": "1"})
            elif page == "5":
                self.respond(200, [], {
                    "X-RateLimit-Remaining": "0", "X-RateLimit-Reset": str(int(time.time()) + 3),
                })
            else:
                self.respond(200, [BETA] if page == "2" else [])
        elif route.path == "/repos/fixture/redirect":
            self.respond(301, headers={"Location": "/repositories/41"})
        elif route.path == "/repos/fixture/escape":
            self.respond(302, headers={"Location": "https://example.invalid/repos/fixture/alpha"})
        else:
            self.respond(*routes.get(route.path, (404, {"message": "Not Found"})))


directory = Path(sys.argv[1])
hits = directory / "hits.log"
expired = directory / "expire"
references = pinned_routes(os.getenv("CTXFAB_REFERENCE_CORPUS"))
with ThreadingHTTPServer(("127.0.0.1", 0), GitHub) as server:
    port_file = directory / "port"
    pending = port_file.with_suffix(".pending")
    pending.write_text(str(server.server_port), encoding="ascii")
    pending.replace(port_file)
    server.serve_forever()
