"""Qualify a full Qwen transfer and its first answer in a disposable preview session."""

import hashlib
import json
import re
import shutil
import subprocess
import sys
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Event, Thread

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "github"))
from dogfood import (
    BASE,
    FEATURES,
    GENERATION_PROFILE,
    TIP,
    backend,
    client_environment,
    save,
    wait_for_commit,
    wait_until,
)
from git_origin import (
    FIXTURE_TOKEN,
    REPOSITORY,
    REPOSITORY_ID,
    GitHistory,
    LoopbackServer,
    origin,
)
from workload import timed_cli

DOWNLOADS = "/v1/model-downloads"
PREFIX_BYTES = 32 * 1024 * 1024
ETAG = '"ctxfab-full-qwen-fixture"'


@contextmanager
def hub(weights):
    released = Event()
    traffic = []
    size = weights.stat().st_size
    route = None

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            return

        def do_GET(self):
            supplied = self.headers.get("Range")
            validator = self.headers.get("If-Range")
            observed = {"route": self.path, "range": supplied, "ifRange": validator}
            traffic.append(observed)
            if self.path != route:
                self.send_error(404)
                return
            start = 0
            if supplied is not None:
                match = re.fullmatch(r"bytes=([1-9][0-9]*)-", supplied)
                if match is None or validator != ETAG or int(match[1]) >= size:
                    self.send_error(416)
                    return
                start = int(match[1])
            self.send_response(206 if start else 200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(size - start))
            self.send_header("ETag", ETAG)
            self.send_header("Accept-Ranges", "bytes")
            if start:
                self.send_header("Content-Range", f"bytes {start}-{size - 1}/{size}")
            self.end_headers()
            with weights.open("rb") as source:
                source.seek(start)
                if start:
                    shutil.copyfileobj(source, self.wfile, 1024 * 1024)
                else:
                    self.wfile.write(source.read(PREFIX_BYTES))
                    released.wait(timeout=90)

    def accept(path):
        nonlocal route
        route = path

    with LoopbackServer(("127.0.0.1", 0), Handler) as server:
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield server.server_port, traffic, accept, released
        finally:
            released.set()
            server.shutdown()
            thread.join()


def model(api):
    catalog, _ = api("GET", DOWNLOADS)
    return next(entry for entry in catalog if entry["profile"] == GENERATION_PROFILE)


def wait_for_model(api, state, *, bytes_expected=None):
    def reached():
        entry = model(api)
        assert entry["state"] != "failed", entry
        if entry["state"] == state and (bytes_expected is None or entry["downloadedBytes"] == bytes_expected):
            return entry
        return None
    return wait_until(120, reached)


def cli(bundle, data, command):
    response = subprocess.check_output(
        [bundle / "Contents/MacOS/ctxfab", command, GENERATION_PROFILE],
        env={**client_environment(), "CTXFAB_DATA_DIRECTORY": str(data)},
        timeout=120,
    )
    return json.loads(response) if response.strip() else None


def sha256(path):
    with path.open("rb") as contents:
        return hashlib.file_digest(contents, "sha256").hexdigest()


def run(bundle, repo, runtime, weights, report, expected):
    if not __debug__:
        raise RuntimeError("Weight qualification requires Python assertions.")
    assert sha256(weights) == expected
    git = GitHistory(repo, [BASE, TIP])
    size = weights.stat().st_size
    with (TemporaryDirectory(prefix="ctxfab-full-model-") as temporary,
          origin(git) as (git_port, git_calls), hub(weights) as (hub_port, requests, accept, released)):
        data = Path(temporary) / "data"
        data.mkdir(mode=0o700)
        inference = data / "inference"
        inference.mkdir(mode=0o700)
        (inference / "llama-server").symlink_to(runtime.resolve(strict=True))
        target = inference / weights.name
        with backend(bundle, data, git_port, report, hub_port=hub_port) as first:
            initial = model(first)
            assert initial["state"] == "notInstalled" and initial["totalBytes"] == size
            models, _ = first("GET", "/v1/generation/models")
            assert models["profiles"] == []
            source = initial["source"]
            accept(f"/{source['repository']}/resolve/{source['revision']}/{weights.name}?download=true")
            cli(bundle, data, "install-model")
            progress = wait_for_model(first, "installing", bytes_expected=PREFIX_BYTES)
            operation = progress["operationId"]
            cli(bundle, data, "pause-model")
            paused = wait_for_model(first, "paused", bytes_expected=PREFIX_BYTES)
            assert paused["operationId"] == operation and not target.exists()
            assert len(requests) == first("GET", "/v1/status")[0]["outboundAttempts"] == 1
            released.set()

        with backend(bundle, data, git_port, report, hub_port=hub_port) as second:
            assert first.process_id != second.process_id
            restored = model(second)
            assert restored["state"] == "paused" and restored["operationId"] == operation
            assert restored["downloadedBytes"] == PREFIX_BYTES and len(requests) == 1
            cli(bundle, data, "resume-model")
            installed = wait_for_model(second, "installed", bytes_expected=size)
            assert target.is_file() and not target.is_symlink()
            assert sha256(target) == expected
            assert len(requests) == second("GET", "/v1/status")[0]["outboundAttempts"] == 2
            assert [item["range"] for item in requests] == [None, f"bytes={PREFIX_BYTES}-"]
            assert requests[1]["ifRange"] == ETAG
            activation = cli(bundle, data, "use-model")
            assert activation["activeProfile"] == GENERATION_PROFILE
            second("PUT", "/v1/github/credential", {"revision": 0, "token": FIXTURE_TOKEN})
            second("PUT", "/v1/github/repositories", {
                "revision": 1, "repository": REPOSITORY, "expectedId": REPOSITORY_ID,
                "branch": "main", "intervalMinutes": 60, "features": FEATURES,
            })
            wait_for_commit(second, BASE)
            answer = timed_cli(bundle, data, git, BASE)
            assert answer["answer"]["mode"] == "bm25"
            attempted = second("GET", "/v1/status")[0]["outboundAttempts"]
            assert attempted == len(git_calls) + len(requests)
            save(report, "weight-workload.json", {
                "artifactSHA256": expected, "bytes": size, "initialProcess": first.process_id,
                "answerProcess": second.process_id, "paused": paused, "restored": restored,
                "installed": installed, "requests": requests, "gitRequests": len(git_calls),
                "ledgerAttempts": attempted, "answer": answer,
            })
    print("Full Qwen weight resumed after restart, verified and activated before the cited answer.")


if __name__ == "__main__":
    run(*map(Path, sys.argv[1:-1]), sys.argv[-1])
