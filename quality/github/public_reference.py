"""Stabilize the external Git snapshot and check its captured Actions attempt separately."""

import argparse
import hashlib
import json
import os
import subprocess
from collections import Counter
from pathlib import Path
from urllib.error import URLError

from dogfood import save
from git_origin import GitHistory
from hosted import PublicGithub, ReferenceChanged

REFERENCE = Path(__file__).with_name("uv-reference.json")


def require_equal(description, actual, expected):
    if actual != expected:
        raise ReferenceChanged(f"{description} differs from the recorded reference.")


def inspect_git_tree(repo, reference):
    commit = reference["commit"]
    parent = reference["parent"]
    git = GitHistory(repo, [commit, parent])
    require_equal("Source tree", git.roots[commit], reference["tree"])
    require_equal("Commit parents", git.parents[commit], [parent])
    files = git.files[commit]
    require_equal("Tracked path count", len(files), reference["trackedPaths"])
    for path, oid in reference["blobs"].items():
        require_equal(f"Blob {path}", files.get(path, {}).get("sha"), oid)
    return git


def inspect_git(repo, reference):
    commit = reference["commit"]
    run = reference["run"]
    workflow = reference["workflow"]
    require_equal("Recorded Actions head", run["head_sha"], commit)
    require_equal("Recorded Actions repository", run["head_repository"]["id"], reference["repositoryId"])
    require_equal("Recorded Actions workflow", run["workflow_id"], workflow["id"])
    require_equal("Workflow source blob is pinned", workflow["path"] in reference["blobs"], True)
    git = inspect_git_tree(repo, reference)
    files = git.files[commit]
    return git, {
        "mode": "offline-pinned-git",
        "status": "passed",
        **reference,
        "trackedBytes": sum(entry.get("size", 0) for entry in files.values()),
    }


def inspect_upstream(reference):
    public = PublicGithub(reference["repository"])
    commit = public(f"/git/commits/{reference['commit']}")
    require_equal("Public Git object", (commit["sha"], commit["tree"]["sha"],
                  [parent["sha"] for parent in commit["parents"]]),
                  (reference["commit"], reference["tree"], [reference["parent"]]))
    expected_run = reference["run"]
    run = public(f"/actions/runs/{expected_run['id']}/attempts/{expected_run['run_attempt']}")
    head_repository = run["head_repository"]
    require_equal("Public Actions repository", (head_repository["full_name"], head_repository["private"]),
                  (reference["repository"], False))
    observed_run = {key: run[key] for key in expected_run}
    observed_run["head_repository"] = {"id": head_repository["id"]}
    require_equal("Public Actions attempt", observed_run, expected_run)
    require_equal("Public Actions workflow path", run["path"], reference["workflow"]["path"])
    return {
        "mode": "live-github-reference",
        "status": "passed",
        "repository": reference["repository"],
        "commit": reference["commit"],
        "run": observed_run,
        "url": f"{run['html_url']}/attempts/{run['run_attempt']}",
        "requests": public.requests,
    }


def inspect_publication(git, reference):
    accepted, skipped = git.coverage(reference["commit"])
    manifest = hashlib.sha256()
    for path, entry in sorted(accepted.items()):
        manifest.update(f"{path}\0{entry['sha']}\n".encode())
    result = {"paths": len(accepted), "manifestSha256": manifest.hexdigest()}
    require_equal("Expected source publication", result, reference["expectedPublication"])
    return {"mode": "offline-publication-oracle", "status": "passed", **result,
            "skipped": dict(sorted(Counter(skipped.values()).items()))}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repo", type=Path,
                        help="Local Git object store containing the pinned commit and first parent.")
    parser.add_argument("report", type=Path)
    parser.add_argument("--check-upstream", action="store_true", help="Also check GitHub's two public endpoints.")
    parser.add_argument("--check-publication", action="store_true",
                        help="Read every source blob to independently check the publication manifest.")
    args = parser.parse_args()
    reports = ("pinned-git.json", "upstream-actions.json", "publication-oracle.json")
    if any((args.report / name).exists() for name in reports):
        parser.error("Select a fresh report directory.")
    os.environ["GIT_NO_LAZY_FETCH"] = "1"
    os.environ["GIT_NO_REPLACE_OBJECTS"] = "1"
    reference = json.loads(REFERENCE.read_text(encoding="utf-8"))
    try:
        git, pinned = inspect_git(args.repo, reference)
        args.report.mkdir(mode=0o700, parents=True, exist_ok=True)
        save(args.report, "pinned-git.json", pinned)
        if args.check_publication:
            save(args.report, "publication-oracle.json", inspect_publication(git, reference))
        if args.check_upstream:
            save(args.report, "upstream-actions.json", inspect_upstream(reference))
    except (ReferenceChanged, URLError, subprocess.CalledProcessError) as error:
        parser.exit(2, f"{error}\n")


if __name__ == "__main__":
    main()
