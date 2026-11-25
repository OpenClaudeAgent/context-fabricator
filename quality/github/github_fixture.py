#!/usr/bin/env python3
"""Loopback GitHub read-only fixture used through the real backend HTTP adapter."""

import os
import sys

STARTUP_DIAGNOSTICS = os.getenv("CTXFAB_GITHUB_FIXTURE_STARTUP_DIAGNOSTICS") == "1"
if STARTUP_DIAGNOSTICS:
    print("GitHub fixture: Python script started", file=sys.stderr, flush=True)

import base64
import hashlib
import json
import re
import subprocess
import tempfile
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from socketserver import TCPServer
from urllib.parse import parse_qs, urlsplit

ALPHA = {"id": 41, "full_name": "fixture/alpha", "default_branch": "main", "private": True}
BETA = {"id": 42, "full_name": "fixture/beta", "default_branch": "trunk", "private": True}
ORGANIZATION = {"id": 43, "full_name": "fixture-org/private", "default_branch": "main", "private": True}
PREFIX = "/repos/fixture/alpha"
COMMIT_A, COMMIT_B = "a" * 40, "b" * 40
ROOT_A, ROOT_B, SRC_A, SRC_B = "1" * 40, "2" * 40, "3" * 40, "4" * 40
SHARED = b"fn paced_source_fixture() {}\n"
REMOVED = b"fn removed_source_fixture() {}\n"
NEW = b"fn newly_collected_fixture() {}\n"
HISTORY_OLD = b"line one\r\nrepeat\r\nrepeat\r\n"
HISTORY_NEW = b"line one\r\nchanged\r\nrepeat\r\n"
HISTORY_CONTEXT_OLD = (
    b"fn elsewhere() { write_all(payload); }\n\n"
    b"const A: u8 = 1;\nconst B: u8 = 2;\nconst C: u8 = 3;\n"
    b"fn sender() {\n    // payload writer old note\n"
    b"    write_all(payload);\n    store(payload);\n}\n"
)
HISTORY_CONTEXT_NEW = HISTORY_CONTEXT_OLD.replace(b"old note", b"new longer safety note")
HISTORY_CONTEXT_CHANGED = HISTORY_CONTEXT_NEW.replace(
    b"    write_all(payload);", b"    write_all(header);", 1
).replace(b"    store(payload);", b"    write_all(payload);")
HISTORY_CONTEXT_OPEN_OLD = HISTORY_CONTEXT_OLD.replace(
    b"    write_all(payload);", b"    write_all(\n        payload\n    );", 1
)
HISTORY_CONTEXT_OPEN_NEW = HISTORY_CONTEXT_OPEN_OLD.replace(
    b"old note", b"new longer safety note"
).replace(b"        payload", b"        header")
ATTEMPT_DECOY = b"GitHub Actions run 701 attempt 8 concluded with success.\n"
OBSERVED_SCOPE_DECOY = b"The parcel parser fixture writes run 9431 attempt 7 passed.\n"
BETA_SOURCE = b"fn paced_source_fixture() { assert!(true); }\n"
ALPHA_PURGE_SOURCE = b"// Alphaguardmarker purgeguardprobe is private. Scope fixture remainder.\n"
BETA_PURGE_SOURCE = b"// Betaguardmarker purgeguardprobe is available. Scope fixture remainder.\n"
LINKED_DECLARATION = b"pub const SETTLEMENT_DELAY_MS: u64 = 1_000;\n"
LINKED_USAGE = (
    b"use crate::policy::SETTLEMENT_DELAY_MS;\n"
    b"fn settle(now: u64, expired: bool) -> Option<u64> {\n"
    b"    if expired {\n"
    b"        let retry = now.saturating_add(SETTLEMENT_DELAY_MS);\n"
    b"        return Some(retry);\n"
    b"    }\n"
    b"    None\n"
    b"}\n"
)
PACKING_FILLER = b"// 0|1=2;3#4^5>6+7*8%9." * 5 + b"\n"
PACKING_SOURCES = {
    "packing-top.rs": b"// violet packing reference.\n\n" + PACKING_FILLER * 10,
    "packing-detail.rs": (
        "// violet café glyph.\n\nconst COLOR: &str = \"crème brûlée\";\n\n"
    ).encode("utf-8") + PACKING_FILLER * 12,
    "packing-tail.rs": b"// violet packing fallback subset.\n\n" + PACKING_FILLER * 10,
    "packing-extra.rs": b"// violet packing extra passage.\n\n" + PACKING_FILLER * 10,
}
WORKFLOW_DEFINITION = b"name: CI\n"
DOCK_WORKFLOW = (
    b"name: Dock release\non: push\njobs:\n  dock-sign:\n    runs-on: ubuntu-latest\n"
    b"    steps:\n      - name: seal-log\n        run: echo dock-sign succeeded\n"
)
DOCK_REPORT = b"Recorded dock-sign job outcome: success.\n"

UI_MARKERS = {
    "/__ui/actions": ("actions-fixture", "actions-dynamic-workflow", "collect-history"),
    "/__ui/graph": ("collect-graph",),
    "/__ui/history": ("collect-history",),
    "/__ui/advance-code": ("advance-code",),
    "/__ui/hold-code-tree": ("hold-code-tree",),
    "/__ui/hold-branches": ("hold-branches",),
}


GRAPH_ROOT = b"pub mod target;\r\nfn call(){crate::target::graph_example();}\r\n"
GRAPH_TARGET = b"pub fn graph_example() {}\n"
GRAPH_MAIN = b"fn main() {}\nfn local_only() {}\n"

def blob_id(content):
    return hashlib.sha1(f"blob {len(content)}\0".encode() + content).hexdigest()


def blob_entry(path, content):
    return {"path": path, "type": "blob", "mode": "100644",
            "sha": blob_id(content), "size": len(content)}


def collect_route(path):
    observed_scope = (directory / "actions-observed-scope").exists()
    if observed_scope or (directory / "actions-attempt-scope").exists():
        runs = (
            [(9431, 1, "success"), (9431, 2, "failure"), (9432, 2, "cancelled")],
            [(9431, 2, "success"), (9431, 7, "cancelled")],
        ) if observed_scope else (
            [(701, 1, "success"), (701, 2, "failure"), (702, 2, "cancelled")],
            [(701, 2, "success"), (701, 8, "cancelled")],
        )
        for repo, branch, repo_id, observations in (
            (PREFIX, "main", 41, runs[0]),
            ("/repos/fixture/beta", "trunk", 42, runs[1]),
        ):
            if path == repo + "/actions/workflows":
                return 200, {"total_count": 1, "workflows": [
                    {"id": 20, "name": "CI", "path": "dynamic/ci/managed", "state": "active"}]}
            if path == repo + "/actions/runs":
                runs = [
                    {"id": run, "workflow_id": 20, "run_attempt": attempt,
                     "event": "push", "head_sha": COMMIT_A, "head_branch": branch,
                     "head_repository": {"id": repo_id}, "status": "completed",
                     "conclusion": result, "created_at": "2026-01-01T00:00:00Z",
                     "updated_at": f"2026-01-01T00:00:{attempt:02d}Z"}
                    for run, attempt, result in observations
                ]
                return 200, {"total_count": len(runs), "workflow_runs": runs}
    if path == PREFIX + "/branches/main" and (directory / "smart-git").exists():
        commit = git_fixture("rev-parse", "refs/heads/main").strip()
        root = git_fixture("rev-parse", "refs/heads/main^{tree}").strip()
        return 200, {"name": "main", "commit": {"sha": commit, "commit": {"tree": {"sha": root}}}}
    if (directory / "actions-fixture").exists():
        dock = (directory / "actions-job-authority").exists()
        gh_tree, workflows_tree = "a1" * 20, "b2" * 20
        if path == PREFIX + "/actions/workflows":
            workflows = [{"id": 20, "name": "Dock release" if dock else "CI",
                          "path": ".github/workflows/dock.yml" if dock else ".github/workflows/ci.yml",
                          "state": "active"}]
            if (directory / "actions-dynamic-workflow").exists():
                workflows.append({
                    "id": 21, "name": "Dependabot Updates",
                    "path": "dynamic/dependabot/dependabot-updates", "state": "active",
                })
            return 200, {"total_count": len(workflows), "workflows": workflows}
        if path == PREFIX + "/actions/runs":
            rerun = (directory / "actions-rerun").exists()
            completed = rerun or (directory / "actions-completed").exists()
            stale = (directory / "actions-stale").exists()
            run = {"id": 30, "workflow_id": 20,
                "run_attempt": 2 if rerun else 1, "event": "push", "head_sha": COMMIT_A,
                "head_branch": "other" if (directory / "actions-other-branch").exists() else "main", "head_repository": {"id": 99 if rerun else 41},
                "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:03Z" if rerun or stale else "2026-01-01T00:00:02Z" if completed else "2026-01-01T00:00:01Z",
                "status": "queued" if stale or not completed else "completed", "conclusion": None if stale or not completed else "success"}
            if dock:
                return 200, {"total_count": 1, "workflow_runs": [
                    dict(run, id=5827, run_attempt=3, status="completed", conclusion="failure")]}
            if (directory / "actions-large-page").exists():
                metadata_size = 45_000 if (directory / "actions-over-limit").exists() else 15_000
                runs = [dict(run, id=30 + index, head_commit={"message": "x" * metadata_size})
                        for index in range(100)]
                return 200, {"total_count": 332, "workflow_runs": runs}
            if (directory / "actions-latest").exists():
                older = dict(run, id=29, updated_at="2026-01-03T00:00:00Z")
                latest = dict(run, id=31, created_at="2026-01-02T00:00:00Z",
                              updated_at="2026-01-02T00:00:00Z")
                latest_attempt = dict(latest, run_attempt=2,
                                      updated_at="2026-01-02T01:00:00Z")
                return 200, {"total_count": 3, "workflow_runs": [older, latest, latest_attempt]}
            return 200, {"total_count": 1, "workflow_runs": [run]}
        if path == PREFIX + "/git/trees/" + ROOT_A:
            tree = [{"path": ".github", "type": "tree", "mode": "040000", "sha": gh_tree}]
            if dock:
                tree.append(blob_entry("dock-report.md", DOCK_REPORT))
            return 200, {"sha": ROOT_A, "truncated": False, "tree": tree}
        if path == PREFIX + "/git/trees/" + gh_tree:
            return 200, {"sha": gh_tree, "truncated": False, "tree": [{"path": "workflows", "type": "tree", "mode": "040000", "sha": workflows_tree}]}
        if path == PREFIX + "/git/trees/" + workflows_tree:
            entry = blob_entry("dock.yml", DOCK_WORKFLOW) if dock else blob_entry("ci.yml", WORKFLOW_DEFINITION)
            return 200, {"sha": workflows_tree, "truncated": False, "tree": [entry]}
        if dock:
            for content in (DOCK_WORKFLOW, DOCK_REPORT):
                oid = blob_id(content)
                if path == PREFIX + "/git/blobs/" + oid:
                    return 200, {"sha": oid, "size": len(content), "encoding": "base64",
                                 "content": base64.encodebytes(content).decode()}
        if path == PREFIX + "/git/blobs/" + blob_id(WORKFLOW_DEFINITION):
            return 200, {"sha": blob_id(WORKFLOW_DEFINITION), "size": len(WORKFLOW_DEFINITION), "encoding": "base64", "content": base64.encodebytes(b"name: XX\n" if (directory / "actions-bad-blob").exists() else WORKFLOW_DEFINITION).decode()}
    if path in references:
        return references[path]
    if path == "/repos/fixture/beta/branches/trunk":
        return 200, {"name": "trunk", "commit": {"sha": "c" * 40,
                                                "commit": {"tree": {"sha": "7" * 40}}}}
    if path == "/repos/fixture/beta/git/trees/" + "7" * 40:
        content = BETA_PURGE_SOURCE if (directory / "purge-answers").exists() else BETA_SOURCE
        return 200, {"sha": "7" * 40, "truncated": False, "tree": [blob_entry("old.rs", content)]}
    for content in (BETA_SOURCE, BETA_PURGE_SOURCE):
        if path == "/repos/fixture/beta/git/blobs/" + blob_id(content):
            return 200, {"sha": blob_id(content), "size": len(content), "encoding": "base64",
                         "content": base64.encodebytes(content).decode()}
    if path == PREFIX + "/branches":
        return (403, {"message": "Resource not accessible"}) if (directory / "deny-contents").exists() else (200, [])
    if (directory / "deny-contents").exists() and (
        path.startswith(PREFIX + "/git/") or path.startswith(PREFIX + "/branches/")
    ):
        return 403, {"message": "Resource not accessible"}
    if path == PREFIX + "/branches/main":
        advanced = (directory / "advance-code").exists()
        commit, root = (COMMIT_B, ROOT_B) if advanced else (COMMIT_A, ROOT_A)
        if (directory / "long-history").exists():
            commit, root = f"{1:040x}", ROOT_A
        return 200, {"name": "main", "commit": {"sha": commit, "commit": {"tree": {"sha": root}}}}
    if path.startswith(PREFIX + "/git/commits/") and (directory / "collect-history").exists():
        oid = path.rsplit("/", 1)[1]
        lineage = {
            COMMIT_A: (ROOT_A, ["d" * 40, "e" * 40], "Merge tracked change"),
            COMMIT_B: (ROOT_B, [COMMIT_A], "Add a follow-up"),
            "d" * 40: (ROOT_A, [], "Start tracked history"),
        }
        if oid in lineage:
            tree, parents, message = lineage[oid]
            return 200, {"sha": oid, "tree": {"sha": tree}, "message": message,
                         "parents": [{"sha": parent} for parent in parents]}
        for position in range(1, 18):
            if oid == f"{position:040x}" and (directory / "long-history").exists():
                if position == 5 and (directory / "fail-history").exists():
                    return 503, {}
                parents = [{"sha": f"{position + 1:040x}"}] if position < 17 else []
                return 200, {"sha": oid, "tree": {"sha": ROOT_A}, "message": "Changed history",
                             "parents": parents}
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
        if (directory / "actions-attempt-scope").exists():
            trees[ROOT_A].append(blob_entry("attempt-note.md", ATTEMPT_DECOY))
        if (directory / "actions-observed-scope").exists():
            trees[ROOT_A].append(blob_entry("parcel-parser.md", OBSERVED_SCOPE_DECOY))
        if (directory / "history-two-crates").exists():
            trees = {}
            for root, offset, fast_id, executable in (
                (ROOT_A, 2000, HISTORY_OLD, SHARED),
                (ROOT_B, 3000, HISTORY_NEW, NEW),
            ):
                crates, fast_id_crate, fs_crate, fast_id_src, fs_src = (
                    f"{offset + index:040x}" for index in range(5)
                )

                def tree(path, oid):
                    return {"path": path, "type": "tree", "mode": "040000", "sha": oid}

                trees[root] = [tree("crates", crates), tree("unchanged", "6" * 40)]
                trees[crates] = [tree("fast-id", fast_id_crate), tree("fs", fs_crate)]
                trees[fast_id_crate] = [tree("src", fast_id_src)]
                trees[fs_crate] = [tree("src", fs_src)]
                trees[fast_id_src] = [blob_entry("lib.rs", fast_id)]
                trees[fs_src] = [blob_entry("which.rs", executable)]
            trees["6" * 40] = []
        if (directory / "history-wide").exists():
            for position in range(40):
                subtree = f"{1000 + position:040x}"
                trees[ROOT_B].append({"path": f"extra{position}", "type": "tree",
                                      "mode": "040000", "sha": subtree})
                trees[subtree] = []
        if oid == SRC_B and (directory / "history-partial").exists():
            trees[oid].append({"path": "large.bin", "type": "blob", "mode": "100644", "sha": "9" * 40, "size": 300000})
            trees[oid].append({"path": "link", "type": "blob", "mode": "120000", "sha": "8" * 40, "size": 3})
        if oid in trees and (directory / "history-modified").exists():
            if oid == SRC_A:
                trees[oid].append(blob_entry("modified.rs", HISTORY_OLD))
            if oid == SRC_B:
                trees[oid].append(blob_entry("modified.rs", HISTORY_NEW))
        if (directory / "history-context").exists():
            if oid == SRC_A:
                contents = HISTORY_CONTEXT_OPEN_OLD if (directory / "history-context-opening").exists() else HISTORY_CONTEXT_OLD
                trees[oid] = [blob_entry("sender.rs", contents)]
            if oid == SRC_B:
                contents = HISTORY_CONTEXT_CHANGED if (directory / "history-context-header").exists() else HISTORY_CONTEXT_NEW
                if (directory / "history-context-opening").exists():
                    contents = HISTORY_CONTEXT_OPEN_NEW
                trees[oid] = [blob_entry("sender.rs", contents)]
        if oid == SRC_A and (directory / "purge-answers").exists():
            trees[oid].append(blob_entry("purge.rs", ALPHA_PURGE_SOURCE))
        if (directory / "collect-graph").exists() and oid in (SRC_A, SRC_B):
            trees[oid] = [blob_entry("lib.rs", GRAPH_ROOT)]
            if (directory / "second-graph-root").exists():
                trees[oid].append(blob_entry("main.rs", GRAPH_MAIN))
            if oid == SRC_A or not (directory / "delete-graph-target").exists():
                trees[oid].append(blob_entry(
                    "target.rs" if oid == SRC_A else "moved.rs", GRAPH_TARGET))
        if (directory / "packing-order").exists() and oid in (SRC_A, SRC_B):
            trees[oid] = [blob_entry(path, content) for path, content in PACKING_SOURCES.items()]
        if (directory / "linked-answers").exists() and oid in (SRC_A, SRC_B):
            trees[oid] = [blob_entry("policy.rs", LINKED_DECLARATION),
                          blob_entry("settlement.rs", LINKED_USAGE)]
        if oid in trees:
            return 200, {"sha": oid, "truncated": (directory / "history-truncated").exists(), "tree": trees[oid]}
    for content in (SHARED, REMOVED, NEW, HISTORY_OLD, HISTORY_NEW, GRAPH_ROOT, GRAPH_TARGET, GRAPH_MAIN, ATTEMPT_DECOY, OBSERVED_SCOPE_DECOY,
                    HISTORY_CONTEXT_OLD, HISTORY_CONTEXT_NEW, HISTORY_CONTEXT_CHANGED,
                    HISTORY_CONTEXT_OPEN_OLD, HISTORY_CONTEXT_OPEN_NEW,
                    ALPHA_PURGE_SOURCE, LINKED_DECLARATION, LINKED_USAGE, *PACKING_SOURCES.values()):
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

    def do_POST(self):
        if self.path == "/fixture/alpha.git/git-upload-pack":
            self.smart_git()
        else:
            self.set_ui_markers(enabled=True)

    def do_DELETE(self):
        self.set_ui_markers(enabled=False)

    def set_ui_markers(self, enabled):
        names = UI_MARKERS.get(self.path) if ui_control else None
        if names is None:
            self.respond(404)
            return
        for name in names:
            marker = directory / name
            if enabled:
                marker.touch()
            else:
                marker.unlink(missing_ok=True)
        self.respond(204)

    def do_GET(self):
        route = urlsplit(self.path)
        if self.path == "/fixture/alpha.git/info/refs?service=git-upload-pack":
            self.smart_git()
            return
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
        query = parse_qs(route.query)
        if route.path == PREFIX + "/branches" and query.get("per_page") == ["100"]:
            hold = directory / "hold-branches"
            deadline = time.monotonic() + 10
            while hold.exists() and time.monotonic() < deadline:
                time.sleep(0.01)
            if (directory / "deny-contents").exists():
                self.respond(403, {"message": "Resource not accessible"})
            elif query.get("page") == ["1"]:
                self.respond(200, [{"name": "main"}, {"name": "feature/search"}], {
                    "Link": '<https://api.github.com/repos/fixture/alpha/branches?page=2>; rel="next"',
                })
            else:
                self.respond(200, [{"name": "release/v1"}])
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
        hold = directory / "hold-code-blob"
        if route.path == PREFIX + "/git/blobs/" + blob_id(NEW) and hold.exists():
            (directory / "held-code-blob").touch()
            deadline = time.monotonic() + 5
            while hold.exists() and time.monotonic() < deadline:
                time.sleep(0.01)
        hold = directory / "hold-history-branch"
        if route.path == PREFIX + "/branches/main" and hold.exists():
            (directory / "held-history-branch").touch()
            deadline = time.monotonic() + 5
            while hold.exists() and time.monotonic() < deadline:
                time.sleep(0.01)
        hold = directory / "hold-history-blob"
        if route.path.startswith(PREFIX + "/git/blobs/") and hold.exists():
            (directory / "held-history-blob").touch()
            deadline = time.monotonic() + 5
            while hold.exists() and time.monotonic() < deadline:
                time.sleep(0.01)
        if (candidate := collect_route(route.path)) is not None:
            self.respond(*candidate)
            return
        if route.path == PREFIX and (directory / "history-repo-changed").exists():
            if f"GET {PREFIX}/git/commits/{COMMIT_B}" in hits.read_text().splitlines():
                self.respond(200, dict(ALPHA, id=42))
                return
        beta = dict(BETA, private=False) if (directory / "public-beta").exists() else BETA
        if (directory / "organization-access").exists():
            if route.path == "/user/repos":
                self.respond(200, [ORGANIZATION] if token in replacements else [])
                return
            if route.path.startswith("/repos/fixture-org/"):
                if token not in replacements:
                    self.respond(404, {"message": "Not Found"})
                elif route.path == "/repos/fixture-org/private":
                    self.respond(200, ORGANIZATION)
                elif route.path == "/repos/fixture-org/private/branches/main":
                    self.respond(200, {})
                elif route.path == "/repos/fixture-org/private/actions/runs":
                    self.respond(200, {"workflow_runs": []})
                else:
                    self.respond(404)
                return
        routes = {
            "/user": (200, {"login": "fixture-user"}),
            "/repos/fixture/alpha": (200, ALPHA),
            "/repos/fixture/beta": (200, beta),
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
                self.respond(200, [beta] if page == "2" else [])
        elif route.path == "/repos/fixture/redirect":
            self.respond(301, headers={"Location": "/repositories/41"})
        elif route.path == "/repos/fixture/escape":
            self.respond(302, headers={"Location": "https://example.invalid/repos/fixture/alpha"})
        else:
            self.respond(*routes.get(route.path, (404, {"message": "Not Found"})))

    def smart_git(self):
        if not (directory / "smart-git").exists():
            self.respond(404)
            return
        with hits.open("a", encoding="utf-8") as log:
            log.write(f"{self.command} {self.path}\n")
        token = self.headers.get("Authorization")
        expected = "Basic " + base64.b64encode(b"x-access-token:github_pat_fixture_primary").decode()
        if token != expected and not (public_benchmark and token is None):
            self.respond(401)
            return
        if (directory / "throttle-pack").exists() and self.command == "POST":
            self.respond(429, {}, {"Retry-After": "2"})
            return
        path = urlsplit(self.path)
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        rejected_pin = directory / "reject-pack-pin"
        if rejected_pin.exists() and b"want " + rejected_pin.read_bytes() + b"\n" in body:
            message = b"ERR upload-pack: not our ref " + rejected_pin.read_bytes() + b"\n"
            packet = f"{len(message) + 4:04x}".encode() + message
            self.send_response(200)
            self.send_header("Content-Type", "application/x-git-upload-pack-result")
            self.send_header("Content-Length", str(len(packet)))
            self.end_headers()
            self.wfile.write(packet)
            return
        environment = {
            **os.environ,
            "GIT_PROJECT_ROOT": str(directory),
            "GIT_HTTP_EXPORT_ALL": "1",
            "PATH_INFO": path.path,
            "QUERY_STRING": path.query,
            "REQUEST_METHOD": self.command,
            "CONTENT_TYPE": self.headers.get("Content-Type", ""),
            "CONTENT_LENGTH": str(len(body)),
            "HTTP_GIT_PROTOCOL": self.headers.get("Git-Protocol", ""),
        }
        reply = subprocess.run(["git", "http-backend"], input=body, env=environment,
                               capture_output=True, check=True).stdout
        headers, content = reply.split(b"\r\n\r\n", 1)
        if self.command == "POST":
            haves = re.findall(rb"have ([0-9a-f]{40})\n", body)
            with (directory / "git-negotiation.log").open("a", encoding="utf-8") as log:
                log.write(f"{len(content)} {' '.join(oid.decode() for oid in haves)}\n")
        self.send_response(200)
        for line in headers.decode().splitlines():
            key, value = line.split(": ", 1)
            self.send_header(key, value)
        self.send_header("Content-Length", str(len(content)))
        self.end_headers()
        self.wfile.write(content)


def git_fixture(*args):
    return subprocess.check_output(["git", "-C", str(directory / "fixture/alpha.git"), *args],
                                   text=True).strip()


def prepare_public_benchmark(root):
    """Serve one immutable code corpus without contacting GitHub."""
    remote = root / "fixture/alpha.git"
    remote.parent.mkdir()
    with tempfile.TemporaryDirectory(dir=root) as checkout:
        worktree = Path(checkout)
        environment = {
            **os.environ,
            "GIT_AUTHOR_NAME": "Benchmark Fixture",
            "GIT_COMMITTER_NAME": "Benchmark Fixture",
            "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
            "GIT_COMMITTER_EMAIL": "fixture@example.invalid",
            "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
            "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
        }
        git = ["git", "-C", checkout]
        subprocess.run([*git, "init", "-q", "--object-format=sha1", "-b", "main"], check=True, env=environment)
        for delay in [205, 108]:
            (worktree / "queue.txt").write_text(f"Driftmarker waits {delay} ms.", encoding="utf-8")
            subprocess.run([*git, "add", "queue.txt"], check=True, env=environment)
            subprocess.run(
                [*git, "-c", "commit.gpgsign=false", "commit", "-qm", f"Wait {delay} ms"],
                check=True, env=environment,
            )
        subprocess.run([*git, "clone", "-q", "--bare", ".", str(remote)], check=True, env=environment)
    git_fixture("config", "uploadpack.allowReachableSHA1InWant", "true")
    (root / "smart-git").touch()
    (root / "revision").write_text(git_fixture("rev-parse", "HEAD"), encoding="ascii")
    (root / "previous-revision").write_text(git_fixture("rev-parse", "HEAD^"), encoding="ascii")


class LoopbackHTTPServer(ThreadingHTTPServer):
    def server_bind(self):
        # The numeric loopback address needs no hostname lookup to serve HTTP.
        TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address


directory = Path(sys.argv[1])
ui_control = sys.argv[2:] == ["--ui-control"]
public_benchmark = sys.argv[2:] == ["--public-benchmark"]
hits = directory / "hits.log"
expired = directory / "expire"
references = pinned_routes(os.getenv("CTXFAB_REFERENCE_CORPUS"))
if public_benchmark:
    prepare_public_benchmark(directory)
if STARTUP_DIAGNOSTICS:
    print("GitHub fixture: binding to loopback", file=sys.stderr, flush=True)
with LoopbackHTTPServer(("127.0.0.1", 0), GitHub) as server:
    port_file = directory / "port"
    pending = port_file.with_suffix(".pending")
    pending.write_text(str(server.server_port), encoding="ascii")
    pending.replace(port_file)
    if STARTUP_DIAGNOSTICS:
        print("GitHub fixture: listening port published", file=sys.stderr, flush=True)
    server.serve_forever()
