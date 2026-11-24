"""Compare retrieval modes on the full trees of pinned Requests and ripgrep sources."""

import argparse
import hashlib
import json
from pathlib import Path
from tempfile import TemporaryDirectory

from dogfood import (
    FEATURES,
    FIXTURE_TOKEN,
    backend,
    command,
    read_manifest,
    save,
    verify_evidence,
    wait_for_commit,
    wait_until,
)
from git_origin import GitHistory, origin
from retrieval_evaluation import evaluate


def ready(api):
    progress, _ = api("GET", "/v1/retrieval")
    return progress if (progress["state"] == "ready" and progress["indexed"] > 0) else None


def measure(bundle, assets, checkout, manifest, cases, report):
    repository, repo_id = manifest["repository"], manifest["repositoryId"]
    commit = manifest["commit"]
    git = GitHistory(checkout, [commit])

    def source(path):
        return git.blob(git.files[commit][path]["sha"])

    def reopen(hit):
        return verify_evidence(git, hit, commit, repository=repository, repository_id=repo_id)

    assert git.files[commit][manifest["path"]]["sha"] == manifest["blob"]
    assert hashlib.sha256(source(manifest["path"])).hexdigest() == manifest["sha256"]
    with TemporaryDirectory(prefix="ctxfab-reference-retrieval-") as temporary, origin(
        git, repository=repository, repository_id=repo_id,
    ) as (port, requests):
        data = Path(temporary) / "data"
        data.mkdir(mode=0o700)
        inference = data / "inference"
        inference.mkdir()
        for name in ("llama-server", "Qwen3-Embedding-0.6B-Q8_0.gguf"):
            (inference / name).symlink_to((assets / name).resolve(strict=True))
        with backend(bundle, data, port, report) as api:
            api("PUT", "/v1/github/credential", {"revision": 0, "token": FIXTURE_TOKEN})
            api("PUT", "/v1/github/repositories", {
                "revision": 1, "repository": repository, "expectedId": repo_id,
                "branch": "main", "intervalMinutes": 60, "features": FEATURES,
            })
            wait_for_commit(api, commit)
            snapshot = read_manifest(data, git, commit)
            print(f"{repository}: published {snapshot['publishedPaths']} paths", flush=True)
            progress = wait_until(1000, lambda: ready(api))
            assert progress["indexed"] == progress["total"]
            observed = len(requests)
            print(f"{repository}: indexed {progress['indexed']} passages", flush=True)
            results = evaluate(
                lambda query, mode, branch: command(
                    bundle, data, "search", query, branch=branch, mode=mode, repository_id=repo_id,
                ),
                reopen, source, cases,
            )
            assert len(requests) == observed == api("GET", "/v1/status")[0]["outboundAttempts"]
            save(report, "results.json", {
                "repository": repository, "snapshot": snapshot, "index": progress,
                "outboundAttempts": observed, **results,
            })
            for mode in ("bm25", "hybrid"):
                identifiers = results["summary"][mode]["identifier"]
                assert identifiers["hitsAt5"] == identifiers["queries"], (repository, mode)


def run(bundle, assets, checkouts, report):
    fixtures = Path(__file__).resolve().parent
    manifests = json.loads((fixtures / "reference-corpora.json").read_text(encoding="utf-8"))
    judgments = json.loads((fixtures / "source-retrieval-judgments.json").read_text(encoding="utf-8"))
    for manifest in manifests:
        repository = manifest["repository"]
        slug = repository.rsplit("/", 1)[1]
        cases = [{**case, "path": manifest["path"]} for case in judgments if case["repository"] == repository]
        assert {case["language"] for case in cases} == {"identifier", "en", "fr"}
        output = report / slug
        output.mkdir()
        measure(bundle, assets, checkouts / f"{slug}.git", manifest, cases, output)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("bundle", "assets", "checkouts", "report"):
        parser.add_argument(name, type=Path)
    options = parser.parse_args()
    run(options.bundle, options.assets, options.checkouts, options.report)
