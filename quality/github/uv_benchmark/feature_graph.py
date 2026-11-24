"""Qualify Rust graph locations and negative controls against a tiny local Git history."""

import argparse
import json
import os
import subprocess
import tempfile
from pathlib import Path
from urllib.parse import urlencode

from features import GOLD, backend, origin, save, stage, wait_for_commit
from git_origin import FIXTURE_TOKEN, GitHistory

REPOSITORY = "fixture/graph-benchmark"
REPOSITORY_ID = 8241
BRANCH = "main"
ROOT = "src/lib.rs"
TARGET = "src/target.rs"
SOURCE = (
    "mod target;\r\n"
    "use crate::target::run as alias;\r\n"
    "fn invoke() { /*é*/ crate::target::run(); alias(); }\r\n"
).encode()
DECLARATION = b"pub fn run() {}\n"


def fixture_history(repo):
    env = {
        "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_NO_REPLACE_OBJECTS": "1",
        "GIT_AUTHOR_NAME": "Graph fixture",
        "GIT_AUTHOR_EMAIL": "graph@example.invalid",
        "GIT_COMMITTER_NAME": "Graph fixture",
        "GIT_COMMITTER_EMAIL": "graph@example.invalid",
        "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
        "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
    }

    def git(*args):
        return (
            subprocess.check_output(["git", "-C", repo, *args], env=env)
            .decode()
            .strip()
        )

    repo.mkdir()
    git("init", "--quiet", "--template=", "--initial-branch", BRANCH)
    (repo / ROOT).parent.mkdir()
    (repo / ROOT).write_bytes(SOURCE)
    (repo / TARGET).write_bytes(DECLARATION)
    git("add", ROOT, TARGET)
    git("commit", "--quiet", "-m", "Declare and use the same symbol")
    previous = git("rev-parse", "HEAD")
    git("rm", "--quiet", TARGET)
    git("commit", "--quiet", "-m", "Remove the target module")
    updated = git("rev-parse", "HEAD")
    return GitHistory(repo, [updated, previous])


def reopen(git, revision, location, name):
    path = location["path"]
    blob = git.files[revision][path]["sha"]
    assert location["blob"] == blob
    assert (
        location["fileUrl"] == f"https://github.com/{REPOSITORY}/blob/{revision}/{path}"
    )
    source = git.blob(blob)
    start, end = location["startByte"], location["endByte"]
    assert source[start:end] == name.encode()
    assert (
        location["startLine"] == location["endLine"] == source[:start].count(b"\n") + 1
    )
    return location


def assert_resolution(git, revision, snapshot):
    assert snapshot["revision"] == revision and snapshot["coverage"] == "published"
    assert not snapshot["truncated"]
    hits = snapshot["hits"]
    assert {hit["role"] for hit in hits} == {"definition", "import", "call"}
    [definition] = [hit for hit in hits if hit["role"] == "definition"]
    assert definition["resolution"] == "definition"
    expected = reopen(git, revision, definition["site"], "run")
    assert expected["path"] == TARGET
    for hit in hits:
        if hit["role"] != "definition":
            assert hit["resolution"] == "resolved"
            assert reopen(git, revision, hit["site"], "run")["path"] == ROOT
            assert reopen(git, revision, hit["target"], "run") == expected


def run(bundle, report):
    report.mkdir(parents=True, exist_ok=False)
    with tempfile.TemporaryDirectory(prefix="ctxfab-feature-graph-") as workspace:
        work = Path(workspace)
        git = fixture_history(work / "repo")
        updated, previous = git.commits
        git.tip = previous
        data = work / "data"
        data.mkdir(mode=0o700)
        with (
            origin(git, repository=REPOSITORY, repository_id=REPOSITORY_ID) as (
                port,
                requests,
            ),
            backend(bundle, data, port, report, preview_runtime=False) as api,
        ):
            api(
                "PUT",
                "/v1/github/credential",
                {"revision": 0, "token": FIXTURE_TOKEN},
            )
            api(
                "PUT",
                "/v1/github/repositories",
                {
                    "revision": 1,
                    "repository": REPOSITORY,
                    "expectedId": REPOSITORY_ID,
                    "branch": BRANCH,
                    "intervalMinutes": 1440,
                    "features": {
                        "currentCode": False,
                        "codeGraph": True,
                        "history": False,
                        "actions": False,
                    },
                },
            )
            wait_for_commit(api, previous)
            prefix = f"/v1/github/repositories/{REPOSITORY_ID}"

            def lookup(symbol, branch=BRANCH):
                query = urlencode({"branch": branch, "root": ROOT, "symbol": symbol})
                return api("GET", prefix + "/graph?" + query)[0]

            def initial_examples():
                return {
                    "run": lookup("run"),
                    "alias": lookup("alias"),
                    "absent": lookup("absent_symbol_control"),
                    "otherBranch": lookup("run", branch="unpublished-control"),
                }

            results, read_io = stage(api, requests, initial_examples)
            save(
                report,
                "resolved.json",
                {"responses": results, "readMetrics": read_io},
            )
            assert_resolution(git, previous, results["run"]["snapshot"])
            assert results["absent"]["snapshot"]["hits"] == []
            assert results["otherBranch"]["snapshot"] is None
            aliases = results["alias"]["snapshot"]["hits"]
            assert aliases and all(
                hit["resolution"] == "unresolved" and not hit.get("target")
                for hit in aliases
            )
            assert (
                read_io["loopbackProviderRequests"]
                == read_io["backendOutboundAttempts"]
                == 0
            )

            git.tip = updated
            change, update_io = stage(
                api, requests, lambda: api("POST", prefix + "/refresh")[0]
            )
            assert change == {"commit": updated, "changed": True}
            final = lookup("run")["snapshot"]
            save(
                report,
                "removed-target.json",
                {"response": final, "updateMetrics": update_io},
            )
            assert final["revision"] == updated and final["coverage"] == "published"
            hits = final["hits"]
            assert hits and all(
                hit["resolution"] == "unresolved" and not hit.get("target")
                for hit in hits
            )
            for hit in hits:
                reopen(git, updated, hit["site"], "run")
    cases = json.loads(GOLD.read_text())["cases"]
    save(
        report,
        "summary.json",
        {
            "questions": [
                case
                for case in cases
                if case["id"] in ("graph-resolution", "graph-unresolved")
            ],
            "initialRevision": previous,
            "advancedRevision": updated,
            "roles": sorted(
                {hit["role"] for hit in results["run"]["snapshot"]["hits"]}
            ),
            "initialOccurrences": len(results["run"]["snapshot"]["hits"]),
            "unresolvedAfterDelete": len(hits),
            "status": "passed",
        },
    )


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Assertions must be enabled.")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path)
    parser.add_argument("report", type=Path)
    args = parser.parse_args()
    run(args.bundle.resolve(strict=True), args.report)
