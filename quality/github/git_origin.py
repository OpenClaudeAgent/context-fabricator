"""Read-only GitHub fixture for exact Git objects from pinned local commits."""

import base64
import json
import subprocess
from contextlib import contextmanager, nullcontext
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from socketserver import TCPServer
from threading import Thread
from urllib.parse import parse_qs, urlsplit

REPOSITORY = "OpenClaudeAgent/context-fabricator"
REPOSITORY_ID = 1465278301
FIXTURE_TOKEN = "github_pat_fixture_primary"
MAX_BYTES = 256 * 1024


class GitHistory:
    def __init__(self, repo, commits):
        self.repo = repo
        self.commits = commits
        self.tip = commits[0]
        self.roots = {commit: self.git("rev-parse", commit + "^{tree}").decode().strip()
                      for commit in commits}
        self.parents = {
            commit: self.git("show", "-s", "--format=%P", commit).decode().split()
            for commit in commits
        }
        self.trees = set(self.roots.values())
        self.blobs = set()
        self.files = {}
        for commit in commits:
            entries = self.entries(self.roots[commit], recursive=True)
            files = {}
            for path, entry in entries:
                if entry["type"] == "tree":
                    self.trees.add(entry["sha"])
                else:
                    files[path] = entry
                    if entry["type"] == "blob":
                        self.blobs.add(entry["sha"])
            self.files[commit] = files

    def git(self, *arguments):
        return subprocess.check_output(["git", "-C", self.repo, *arguments])

    def entries(self, tree, recursive=False):
        flags = ["-r", "-t"] if recursive else []
        raw = self.git("ls-tree", "-l", "-z", *flags, tree)
        for record in raw.split(b"\0"):
            if not record:
                continue
            header, path = record.decode().split("\t", 1)
            mode, kind, oid, size = header.split()
            entry = {"path": path, "mode": mode, "type": kind, "sha": oid}
            if size != "-":
                entry["size"] = int(size)
            yield path, entry

    def blob(self, oid):
        assert oid in self.blobs
        return self.git("cat-file", "blob", oid)

    def coverage(self, commit):
        accepted = {}
        skipped = {}
        for path, entry in self.files[commit].items():
            if entry["mode"] not in ("100644", "100755"):
                skipped[path] = "unsupportedGitEntry"
            elif entry["size"] > MAX_BYTES:
                skipped[path] = "oversized"
            else:
                content = self.blob(entry["sha"])
                if b"\0" in content:
                    skipped[path] = "binary"
                elif content.startswith(b"version https://git-lfs.github.com/spec/v1\n"):
                    skipped[path] = "lfsPointer"
                else:
                    try:
                        content.decode()
                    except UnicodeDecodeError:
                        skipped[path] = "nonUtf8"
                    else:
                        accepted[path] = entry
        return accepted, skipped


class LoopbackServer(ThreadingHTTPServer):
    def server_bind(self):
        TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address


@contextmanager
def origin(git, observe=None, *, token=FIXTURE_TOKEN, repository=REPOSITORY,
           repository_id=REPOSITORY_ID, actions_reference=None, branch="main"):
    info = {"id": repository_id, "full_name": repository,
            "default_branch": branch, "private": False}
    prefix = "/repos/" + repository
    requests = []
    actions = actions_reference
    if actions is None and repository == REPOSITORY:
        actions = json.loads(Path(__file__).with_name("actions-reference.json").read_text(encoding="utf-8"))

    def response(path, query):
        if path == "/user":
            return {"login": "dogfood-fixture"}
        if path == prefix:
            return info
        if path == prefix + "/branches":
            return []
        if path == prefix + "/branches/" + branch:
            return {"name": branch, "commit": {
                "sha": git.tip, "commit": {"tree": {"sha": git.roots[git.tip]}}}}
        if actions is not None and path == prefix + "/actions/workflows":
            workflows = actions.get("workflows") or [actions["workflow"]]
            return {"total_count": len(workflows), "workflows": workflows}
        if actions is not None and path == prefix + "/actions/runs" and (
            query.get("branch") == [branch] or query.get("per_page") == ["1"]
        ):
            eligible = [run for run in actions["runs"] if run["head_sha"] in git.commits]
            count = int(query["per_page"][0])
            return {"total_count": len(eligible), "workflow_runs": eligible[:count]}
        oid = path.rsplit("/", 1)[-1]
        if path == f"{prefix}/git/trees/{oid}" and oid in git.trees:
            return {"sha": oid, "truncated": False,
                    "tree": [entry for _, entry in git.entries(oid)]}
        if path == f"{prefix}/git/blobs/{oid}" and oid in git.blobs:
            blob = git.blob(oid)
            return {"sha": oid, "size": len(blob), "encoding": "base64",
                    "content": base64.b64encode(blob).decode()}
        if path == f"{prefix}/git/commits/{oid}" and oid in git.roots:
            return {"sha": oid, "tree": {"sha": git.roots[oid]},
                    "parents": [{"sha": parent} for parent in git.parents[oid]],
                    "message": git.git("show", "-s", "--format=%s", oid).decode().strip()}
        return None

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            return

        def do_GET(self):
            route = urlsplit(self.path)
            path = route.path
            requests.append(path)
            with observe(path) if observe is not None else nullcontext():
                self.respond(path, parse_qs(route.query))

        def respond(self, path, query):
            authorized = (
                self.headers.get("Authorization") == f"Bearer {token}"
                and self.headers.get("X-GitHub-Api-Version") == "2026-03-10"
            )
            result = response(path, query) if authorized else None
            body = json.dumps(result if result is not None else {}).encode()
            self.send_response(200 if result is not None else 404 if authorized else 401)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    with LoopbackServer(("127.0.0.1", 0), Handler) as server:
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield server.server_port, requests
        finally:
            server.shutdown()
            thread.join()
