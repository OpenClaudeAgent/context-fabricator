"""Compare warmed search on the same published corpus in counterbalanced order."""

import argparse
import hashlib
import json
import math
import os
import random
import sqlite3
import subprocess
import sys
import tempfile
import time
from collections import defaultdict
from pathlib import Path
from statistics import median
from urllib.error import HTTPError
from urllib.parse import urlencode

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from checkpoint import PROFILE, restore_into_fixture
from dogfood import read_manifest, verify_evidence, wait_until
from features import (
    REFERENCE,
    backend,
    configure,
    inspect_git,
    origin,
    save,
    wait_for_commit,
)
from git_origin import FIXTURE_TOKEN
from public_reference import inspect_git_tree
from score import audit_index, load_gold, score_search, score_exact_search, vector_census

WEIGHTS = "Qwen3-Embedding-0.6B-Q8_0.gguf"
MODES = ("bm25", "semantic", "hybrid")


def wait_for_vectors(api, expected):
    def ready():
        state, _ = api("GET", "/v1/retrieval")
        return state if state["state"] == "ready" else None

    state = wait_until(180, ready)
    assert state["indexed"] == state["total"] == expected, state


def quantiles(values):
    ordered = sorted(values)
    return {
        "medianMs": round(median(values), 3),
        "p95Ms": round(ordered[math.ceil(len(values) * 0.95) - 1], 3),
        "samples": len(values),
    }


def span_millis(api, row):
    stage = "lexical_lookup" if row["mode"] == "bm25" else "vector_retrieval"
    identifier = row["trace"]

    def published():
        try:
            detail, _ = api("GET", f"/v1/telemetry/traces/{identifier}")
        except HTTPError as error:
            if error.code != 404:
                raise
            error.close()
            return None
        for span in detail["spans"]:
            if span["name"] == stage:
                return span["value"] / 1000
        return None

    return wait_until(10, published)


def run(args):
    if bool(args.reference) != bool(args.gold):
        raise ValueError("Supply --reference and --gold together.")
    reference_path = args.reference or REFERENCE
    reference = json.loads(reference_path.read_text())
    descriptor = json.loads(args.checkpoint.read_text())
    if args.reference:
        git = inspect_git_tree(args.repo, reference)
    else:
        git, _ = inspect_git(args.repo, reference)
    cases, digest = load_gold(git, args.gold) if args.gold else load_gold(git)
    assert descriptor["sourceCommit"] == git.tip and descriptor["profile"] == PROFILE
    assert descriptor["manifestSha256"] == reference["expectedPublication"]["manifestSha256"]
    assert (args.checkpoint.parent / descriptor["file"]).stat().st_size == descriptor["bytes"]
    with (args.assets / WEIGHTS).open("rb") as weight:
        assert hashlib.file_digest(weight, "sha256").hexdigest() == "06507c7b42688469c4e7298b0a1e16deff06caf291cf0a5b278c308249c3e439"
    for bundle in (args.before, args.after):
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(bundle)], check=True)
    args.report.mkdir(parents=True, exist_ok=False)
    repository, rid = reference["repository"], reference["repositoryId"]
    scope = {"repositoryId": rid, "branch": "main"}
    workload = [
        (case, language, mode)
        for case in cases
        for language in case["questions"]
        for mode in MODES
    ]
    for_warmup = [
        "/v1/search?" + urlencode({"mode": mode, "query": "uv configuration", **scope})
        for mode in MODES
    ]
    random.Random(3721).shuffle(workload)
    rows, rankings, timings, server_timings = (
        [],
        {},
        defaultdict(list),
        defaultdict(list),
    )
    with (
        tempfile.TemporaryDirectory(prefix="ctxfab-warm-retrieval-") as tmp,
        origin(git, repository=repository, repository_id=rid) as (port, requests),
    ):
        data = Path(tmp) / "data"
        data.mkdir(mode=0o700)
        if args.published_fixture:
            source = args.published_fixture.resolve(strict=True)
            with (
                sqlite3.connect(source.as_uri() + "?mode=ro", uri=True) as src,
                sqlite3.connect(data / "context.sqlite3") as dst,
            ):
                src.backup(dst)
                passages = dst.execute("SELECT count(*) FROM documents").fetchone()[0]
        else:
            ingest = args.report / "publication"
            ingest.mkdir()
            with backend(
                args.before, data, port, ingest, hub_port=port, preview_runtime=False
            ) as api:
                api(
                    "PUT",
                    "/v1/github/credential",
                    {"revision": 0, "token": FIXTURE_TOKEN},
                )
                configure(api, reference, "main")
                wait_for_commit(api, git.tip, timeout=900)
                passages = audit_index(data, git, git.tip)["passages"]
        assert read_manifest(data, git, git.tip)["publishedPaths"] == reference["expectedPublication"]["paths"]
        index = audit_index(data, git, git.tip)
        restored = restore_into_fixture(
            data, args.checkpoint.parent / descriptor["file"], descriptor["sha256"], reference_path
        )
        assert restored == passages == descriptor["rows"] == index["passages"]
        save(args.report, "vector-census.json", vector_census(data, restored))
        inference = data / "inference"
        inference.mkdir(exist_ok=True)
        subprocess.run(
            [
                "/bin/cp",
                "-c",
                (args.assets / WEIGHTS).resolve(strict=True),
                inference / WEIGHTS,
            ],
            check=True,
        )
        origin_count = len(requests)
        for block, name in enumerate(("before", "after", "after", "before"), 1):
            path = args.report / f"{block}-{name}"
            path.mkdir()
            bundle = getattr(args, name)
            with backend(
                bundle, data, port, path, hub_port=port, preview_runtime=False
            ) as api:
                if block == 1:
                    api("POST", "/v1/model-downloads", {"profile": PROFILE})
                wait_for_vectors(api, restored)
                before = api("GET", "/v1/status")[0]["outboundAttempts"]
                for route in for_warmup:
                    api("GET", route)
                start_of_block = len(rows)
                for case, language, mode in workload:
                    route = "/v1/search?" + urlencode(
                        {"query": case["questions"][language], "mode": mode, **scope}
                    )
                    started = time.monotonic_ns()
                    hits, trace = api("GET", route)
                    millis = (time.monotonic_ns() - started) / 1e6
                    for hit in hits:
                        verify_evidence(git, hit, git.tip, repository=repository, repository_id=rid)
                    identity = (case["id"], language, mode)
                    variants = rankings.setdefault(identity, {})
                    prior = variants.setdefault(name, hits)
                    assert hits == prior, (
                        f"Ranking changed between repeats for {identity} in {name}"
                    )
                    if not args.compare_rankings and len(variants) == 2:
                        assert variants["before"] == variants["after"], (
                            f"Ranking changed for {identity}"
                        )
                    timings[name, mode].append(millis)
                    rows.append(
                        {
                            "case": case["id"],
                            "language": language,
                            "mode": mode,
                            "block": block,
                            "variant": name,
                            "searchMs": millis,
                            "trace": trace,
                        }
                    )
                # Trace polling runs after the timed block, outside the search workload.
                for row in rows[start_of_block:]:
                    row["retrievalSpanMs"] = span_millis(api, row)
                    server_timings[name, row["mode"]].append(row["retrievalSpanMs"])
                assert api("GET", "/v1/status")[0]["outboundAttempts"] == before
                assert len(requests) == origin_count
                save(args.report, "timings.json", rows)
    case_by_id = {case["id"]: case for case in cases}
    save(
        args.report,
        "rankings.json",
        [
            {
                "case": case_id,
                "language": language,
                "mode": mode,
                "quality": score_search(case_by_id[case_id], variants["before"]),
                "exactQuality": score_exact_search(case_by_id[case_id], variants["before"]),
                "hits": variants["before"],
                **(
                    {
                        "afterQuality": score_search(
                            case_by_id[case_id], variants["after"]
                        ),
                        "afterExactQuality": score_exact_search(case_by_id[case_id], variants["after"]),
                        "afterHits": variants["after"],
                    }
                    if args.compare_rankings
                    else {}
                ),
            }
            for (case_id, language, mode), variants in rankings.items()
        ],
    )
    save(
        args.report,
        "summary.json",
        {
            "commit": git.tip,
            "passages": restored,
            "goldSha256": digest,
            "vectorsSha256": descriptor["sha256"],
            "blocks": ["before", "after", "after", "before"],
            "binarySha256": {
                name: hashlib.sha256(
                    (getattr(args, name) / "Contents/MacOS/ctxfab-server").read_bytes()
                ).hexdigest()
                for name in ("before", "after")
            },
            "retrievalRowsWithIdenticalRankings": sum(
                variants["before"] == variants["after"]
                for variants in rankings.values()
            ),
            "requestCount": len(rows),
            "providerRequestsDuringQueries": 0,
            "publicationReused": bool(args.published_fixture),
            "latency": {
                name: {mode: quantiles(timings[name, mode]) for mode in MODES}
                for name in ("before", "after")
            },
            "retrievalSpanLatency": {
                name: {mode: quantiles(server_timings[name, mode]) for mode in MODES}
                for name in ("before", "after")
            },
        },
    )


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Assertions must be enabled.")
    os.environ.update(GIT_NO_LAZY_FETCH="1", GIT_NO_REPLACE_OBJECTS="1")
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("repo", "before", "after", "assets", "checkpoint", "report"):
        parser.add_argument(name, type=Path)
    parser.add_argument(
        "--published-fixture",
        type=Path,
        help="Published uv fixture copy, verified against the vector checkpoint.",
    )
    parser.add_argument(
        "--compare-rankings",
        action="store_true",
        help="Record both stable rankings when comparing a retrieval quality change.",
    )
    parser.add_argument("--reference", type=Path, help="Pinned external repository reference JSON.")
    parser.add_argument("--gold", type=Path, help="Frozen dataset for the external reference.")
    run(parser.parse_args())
