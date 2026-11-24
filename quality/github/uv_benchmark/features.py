"""Exercise pinned GitHub feature contracts without opening the app or loading a model."""

import argparse
import hashlib
import json
import os
import sys
import tempfile
import time
from collections import Counter
from functools import lru_cache
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlencode

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from dogfood import (
    FIXTURE_TOKEN,
    backend,
    read_manifest,
    save,
    verify_evidence,
    wait_for_commit,
)
from git_origin import origin
from public_reference import REFERENCE, inspect_git

HERE = Path(__file__).resolve().parent
GOLD = HERE / "feature-gold.json"


def configure(api, ref, branch, *, auxiliary=False, collect_current=True):
    revision = api("GET", "/v1/github")[0]["revision"]
    api(
        "PUT",
        "/v1/github/repositories",
        {
            "revision": revision,
            "repository": ref["repository"],
            "expectedId": ref["repositoryId"],
            "branch": branch,
            "intervalMinutes": 1440,
            "features": {
                "currentCode": collect_current,
                "codeGraph": collect_current,
                "history": auxiliary,
                "actions": auxiliary,
            },
        },
    )


def status_counter(api):
    return api("GET", "/v1/status")[0]["outboundAttempts"]


def stage(api, requests, exercise):
    start = time.monotonic()
    offset = len(requests)
    previous = status_counter(api)
    result = exercise()
    ledger = status_counter(api) - previous
    observed = requests[offset:]
    counts = {"blob": 0, "tree": 0, "other": 0}
    for path in observed:
        key = (
            "blob"
            if "/git/blobs/" in path
            else "tree"
            if "/git/trees/" in path
            else "other"
        )
        counts[key] += 1
    return result, {
        "elapsedSeconds": round(time.monotonic() - start, 3),
        "loopbackProviderRequests": len(observed),
        "backendOutboundAttempts": ledger,
        "requestsByKind": counts,
        "requestAccountingAgrees": ledger == len(observed),
    }


def current(api, requests, data, git, ref, gold, report):
    branch, parent, tip = gold["branch"], gold["parent"], gold["commit"]
    prefix = f"/v1/github/repositories/{ref['repositoryId']}"
    git.tip = parent

    def initial():
        configure(api, ref, branch)
        return wait_for_commit(api, parent, timeout=900)

    first, first_io = stage(api, requests, initial)
    manifest_before = read_manifest(data, git, parent)
    unchanged, unchanged_io = stage(
        api, requests, lambda: api("POST", prefix + "/refresh")[0]
    )
    assert unchanged == {"commit": parent, "changed": False}, unchanged
    assert unchanged_io["requestsByKind"]["blob"] == 0
    assert unchanged_io["requestsByKind"]["tree"] == 0
    assert read_manifest(data, git, parent) == manifest_before
    save(
        report,
        "initial-branch.json",
        {
            "publication": first,
            "metrics": first_io,
            "manifest": manifest_before,
            "unchanged": unchanged,
            "unchangedMetrics": unchanged_io,
        },
    )

    git.tip = tip
    advanced, advance_io = stage(
        api, requests, lambda: api("POST", prefix + "/refresh")[0]
    )
    assert advanced == {"commit": tip, "changed": True}, advanced
    manifest_after = read_manifest(data, git, tip)
    assert manifest_after["publishedPaths"] == ref["expectedPublication"]["paths"]
    changed = git.git("diff", "--name-status", "-M", parent, tip).decode().splitlines()
    assert changed == [
        "M\tcrates/uv-fastid/src/lib.rs",
        "M\tcrates/uv-fs/src/which.rs",
    ], changed

    case = {item["id"]: item for item in gold["cases"]}["branch-transition"]

    def search(name):
        params = urlencode(
            {
                "query": case["query"],
                "repositoryId": ref["repositoryId"],
                "branch": name,
            }
        )
        return api("GET", "/v1/search?" + params)[0]

    results, query_io = stage(
        api, requests, lambda: (search(branch), search("unpublished-control"))
    )
    hits, wrong_branch = results
    assert hits and wrong_branch == []
    passages = [
        verify_evidence(
            git,
            hit,
            tip,
            repository=ref["repository"],
            repository_id=ref["repositoryId"],
        )
        for hit in hits
    ]
    supporting = [
        i
        for i, (hit, passage) in enumerate(zip(hits, passages, strict=True), 1)
        if hit["location"]["path"] == case["sourcePath"]
        and case["sourceText"] in passage
    ]
    assert (
        query_io["backendOutboundAttempts"] == query_io["loopbackProviderRequests"] == 0
    )
    for measurements in (first_io, unchanged_io, advance_io):
        assert measurements["requestAccountingAgrees"], measurements
    return {
        "publication": advanced,
        "metrics": advance_io,
        "gitDiff": changed,
        "manifest": manifest_after,
        "search": {
            "query": case["query"],
            "metrics": query_io,
            "hits": hits,
            "supportingRanks": supporting,
            "unpublishedBranchHits": wrong_branch,
        },
    }


def history(api, prefix, git, gold, report):
    tip, parent = gold["commit"], gold["parent"]
    lineage, trace = api("POST", prefix + "/history")
    save(report, "history-lineage.json", {"trace": trace, "response": lineage})
    assert lineage["tip"] == tip and lineage["branch"] == gold["branch"]
    assert [commit["id"] for commit in lineage["commits"][:2]] == [tip, parent]
    assert lineage["commits"][0]["parents"] == [parent]
    assert lineage["commits"][0]["tree"] == git.roots[tip]
    route = f"{prefix}/history/comparisons/{tip}"
    try:
        comparison, trace = api("GET", route)
    except HTTPError as error:
        with error:
            problem = json.load(error)
            trace = error.headers.get("X-Context-Trace-Id")
        trees = (
            sum(
                e["type"] == "tree"
                for _, e in git.entries(git.roots[tip], recursive=True)
            )
            + 1
        )
        save(
            report,
            "history-comparison-limit.json",
            {
                "trace": trace,
                "httpStatus": error.code,
                "problem": problem,
                "treeCountIncludingRoot": trees,
                "historyTreeLimit": 256,
            },
        )
        if (
            error.code != 422
            or problem.get("code") != "github_collection_limit"
            or trees <= 256
        ):
            raise
        try:
            api("POST", route)
        except HTTPError as publish_error:
            with publish_error:
                publish_problem = json.load(publish_error)
                publish_trace = publish_error.headers.get("X-Context-Trace-Id")
            assert publish_error.code == 422
            assert publish_problem.get("code") == problem["code"]
            save(
                report,
                "history-materialization-limit.json",
                {
                    "trace": publish_trace,
                    "httpStatus": publish_error.code,
                    "problem": publish_problem,
                },
            )
        else:
            raise AssertionError(
                "The immutable publication did not agree with the bounded read."
            )
        assert api("GET", f"{prefix}/history/evidence/{tip}")[0] is None
        return {"child": tip, "parent": parent, "comparison": "collectionLimit"}
    save(report, "history-comparison.json", {"trace": trace, "response": comparison})
    assert comparison["parent"] == parent and comparison["child"] == tip
    assert comparison["baseline"] == "firstParent"
    assert comparison["coverage"] == "completeTreesFullBlobs"
    assert len(comparison["changes"]) == 2
    for change in comparison["changes"]:
        assert change["kind"] == "modified", change
        for side, revision in (("before", parent), ("after", tip)):
            item = change[side]
            expected = git.files[revision][item["path"]]
            assert item["blob"] == expected["sha"]
            assert item["text"].encode() == git.blob(expected["sha"])
    materialized, trace = api("POST", route)
    save(
        report, "history-materialized.json", {"trace": trace, "response": materialized}
    )
    assert materialized["parent"] == parent and materialized["child"] == tip
    case = {item["id"]: item for item in gold["cases"]}["history-comparison"]
    [file] = [
        item
        for item in materialized["changes"]
        if item["change"]["after"]["path"] == case["sourcePath"]
    ]
    assert file["projection"]["coverage"] == "complete"
    for side, revision in (("before", parent), ("after", tip)):
        raw = git.blob(git.files[revision][case["sourcePath"]]["sha"])
        expectation = case[side]
        excerpt = "\n".join(
            raw.decode().splitlines()[
                expectation["startLine"] - 1 : expectation["endLine"]
            ]
        )
        assert expectation["text"] in excerpt
        anchors = [
            hunk[side] for hunk in file["projection"]["hunks"] if hunk[side] is not None
        ]
        assert anchors and any(
            a["startLine"] <= expectation["startLine"]
            and a["endLine"] >= expectation["endLine"]
            for a in anchors
        )
        for anchor in anchors:
            assert (
                anchor["text"].encode() == raw[anchor["startByte"] : anchor["endByte"]]
            )
    return {"child": tip, "parent": parent, "comparison": "materialized"}


def actions(api, prefix, git, ref, captured, report):
    snapshot, trace = api("POST", prefix + "/actions")
    save(report, "actions-attempt.json", {"trace": trace, "response": snapshot})
    assert snapshot["definitionCommit"] == ref["commit"]
    assert snapshot["executedDefinition"] == "unknown"
    assert snapshot["attemptCoverage"] == "observedLatestAttemptsOnly"
    [workflow] = snapshot["workflows"]
    [attempt] = snapshot["runs"]
    observed = captured["run"]
    path = ref["workflow"]["path"]
    source = git.files[ref["commit"]][path]
    assert (workflow["id"], workflow["path"], workflow["definitionBlob"]) == (
        observed["workflow_id"],
        path,
        source["sha"],
    )
    assert workflow["definition"].encode() == git.blob(source["sha"])
    assert (
        attempt["id"],
        attempt["attempt"],
        attempt["workflowId"],
        attempt["event"],
        attempt["headSha"],
        attempt["headBranch"],
        attempt["headRepositoryId"],
        attempt["status"],
        attempt["conclusion"],
    ) == (
        observed["id"],
        observed["run_attempt"],
        observed["workflow_id"],
        observed["event"],
        observed["head_sha"],
        observed["head_branch"],
        observed["head_repository"]["id"],
        observed["status"],
        observed["conclusion"],
    )
    assert attempt["commitProvenance"] == "verifiedRepositoryCommit"
    return attempt


def graph(api, prefix, git, gold, report):
    request = prefix + "/graph?" + urlencode({"branch": gold["branch"]})
    result, trace = api("GET", request)
    _, skipped = git.coverage(gold["commit"])
    roots = sorted(
        path
        for path in git.files[gold["commit"]].keys() - skipped.keys()
        if Path(path).parts[-2:] in (("src", "lib.rs"), ("src", "main.rs"))
    )
    save(
        report,
        "graph-full-uv.json",
        {
            "trace": trace,
            "response": result,
            "rustRootCandidates": roots,
        },
    )
    assert len(roots) > 8
    snapshot = result["snapshot"]
    assert snapshot["revision"] == gold["commit"]
    assert (
        snapshot["coverage"] == "limit"
        and snapshot["roots"] == []
        and snapshot["hits"] == []
    )
    return len(roots)


def run(repo, bundle, report, reuse_current_evidence):
    ref = json.loads(REFERENCE.read_text())
    git, _ = inspect_git(repo, ref)
    git.coverage = lru_cache(maxsize=2)(git.coverage)
    gold = json.loads(GOLD.read_text())
    captured = json.loads((HERE / "captured-attempt.json").read_text())
    assert (gold["repository"], gold["parent"], gold["commit"]) == (
        ref["repository"],
        ref["parent"],
        ref["commit"],
    )
    assert captured["run"] == ref["run"]
    assert len({case["id"] for case in gold["cases"]}) == len(gold["cases"])
    report.mkdir(parents=True, exist_ok=False)
    save(report, "questions.json", gold)
    fixture = {"workflow": ref["workflow"], "runs": [captured["run"]]}
    prefix = f"/v1/github/repositories/{ref['repositoryId']}"
    with (
        tempfile.TemporaryDirectory(prefix="ctxfab-feature-bench-") as workspace,
        origin(
            git,
            repository=ref["repository"],
            repository_id=ref["repositoryId"],
            actions_reference=fixture,
        ) as (port, requests),
    ):
        data = Path(workspace) / "data"
        data.mkdir(mode=0o700)
        with backend(bundle, data, port, report, preview_runtime=False) as api:
            api("PUT", "/v1/github/credential", {"revision": 0, "token": FIXTURE_TOKEN})
            if reuse_current_evidence is None:
                source = current(api, requests, data, git, ref, gold, report)
                save(report, "advanced-branch.json", source)
                roots = graph(api, prefix, git, gold, report)
            else:
                retained_gold = json.loads(
                    (reuse_current_evidence / "questions.json").read_text()
                )
                assert retained_gold == gold, (
                    "The retained publication uses different questions."
                )
                source = json.loads(
                    (reuse_current_evidence / "advanced-branch.json").read_text()
                )
                retained_graph = json.loads(
                    (reuse_current_evidence / "graph-full-uv.json").read_text()
                )
                assert (
                    retained_graph["response"]["snapshot"]["revision"] == gold["commit"]
                )
                assert retained_graph["response"]["snapshot"]["coverage"] == "limit"
                roots = len(retained_graph["rustRootCandidates"])
            configure(
                api,
                ref,
                gold["branch"],
                auxiliary=True,
                collect_current=reuse_current_evidence is None,
            )
            change = history(api, prefix, git, gold, report)
            attempt = actions(api, prefix, git, ref, captured, report)
            child = source["publication"]["commit"]
            assert child == change["child"] == attempt["headSha"] == gold["commit"]
    save(
        report,
        "summary.json",
        {
            "questionsSha256": hashlib.sha256(GOLD.read_bytes()).hexdigest(),
            "child": child,
            "parent": change["parent"],
            "rustCandidates": roots,
            "graphCoverage": "limit",
            "actionsAttempt": captured["url"],
            "providerRequests": len(requests),
            "providerRoutes": Counter(requests),
            "currentSourceSupportingRanks": source["search"]["supportingRanks"],
            "currentEvidenceReused": reuse_current_evidence is not None,
            "historyComparison": change["comparison"],
            "status": "measured",
            "generatedAnswers": 0,
        },
    )


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Assertions must be enabled.")
    os.environ.update(GIT_NO_LAZY_FETCH="1", GIT_NO_REPLACE_OBJECTS="1")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repo", type=Path)
    parser.add_argument("bundle", type=Path)
    parser.add_argument("report", type=Path)
    parser.add_argument("--reuse-current-evidence", type=Path)
    args = parser.parse_args()
    run(
        args.repo.resolve(strict=True),
        args.bundle.resolve(strict=True),
        args.report,
        args.reuse_current_evidence,
    )
