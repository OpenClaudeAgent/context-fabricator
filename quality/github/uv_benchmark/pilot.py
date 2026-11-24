"""Run the frozen reduced candidate-pool comparison through the packaged product."""

import argparse
import hashlib
import json
import os
import re
import sqlite3
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from compare import compare
from dogfood import (
    FEATURES,
    FIXTURE_TOKEN,
    backend,
    command,
    read_manifest,
    save,
    verify_evidence,
    wait_for_commit,
)
from git_origin import origin
from public_reference import REFERENCE, inspect_git
from score import load_gold, score_search, summarize, vector_census

EMBED = "qwen3-embedding-0.6b-q8_0-06507c7b-d1024-v1"
FILENAME = "Qwen3-Embedding-0.6B-Q8_0.gguf"
HERE = Path(__file__).parent


def freeze(git, cases):
    target = {a["path"] for c in cases for f in c["facts"] for a in f["anchors"]}
    retained, _ = git.coverage(git.tip)

    def tokens(path):
        return set(re.findall(r"[a-z0-9]+", path.lower())) - {
            "rs",
            "md",
            "toml",
            "uv",
            "crates",
            "docs",
            "src",
            "lib",
            "concepts",
        }

    vocabulary = set.union(*(tokens(p) for p in target))
    pool = [
        p
        for p, row in retained.items()
        if p not in target and 1 <= row["size"] <= 12288
    ]

    def stable(path):
        return hashlib.sha256(("uv-pilot-v1\0" + path).encode()).hexdigest()

    ranked = sorted(
        (p for p in pool if tokens(p) & vocabulary),
        key=lambda p: (-len(tokens(p) & vocabulary), stable(p)),
    )
    hard = ranked[:12]
    neutral = sorted(set(pool) - set(hard), key=stable)[:8]
    assert len(target) == 8 and len(hard) == 12 and len(neutral) == 8
    roles = (
        {p: "judged" for p in target}
        | {p: "path-token" for p in hard}
        | {p: "hash-control" for p in neutral}
    )
    return {
        "commit": git.tip,
        "selectionVersion": 1,
        "files": [
            {
                "path": p,
                "blob": retained[p]["sha"],
                "bytes": retained[p]["size"],
                "role": roles[p],
            }
            for p in sorted(roles)
        ],
    }


def render(obj):
    return (json.dumps(obj, indent=2, ensure_ascii=False) + "\n").encode()


def project(data, paths):
    with sqlite3.connect(data / "context.sqlite3") as db:
        db.execute("PRAGMA foreign_keys=ON")
        db.execute("CREATE TEMP TABLE keep(path TEXT PRIMARY KEY)")
        db.executemany("INSERT INTO keep VALUES (?)", ((p,) for p in paths))
        db.execute("DELETE FROM documents WHERE title NOT IN (SELECT path FROM keep)")
        assert {
            r[0] for r in db.execute("SELECT DISTINCT title FROM documents")
        } == paths
        count = db.execute("SELECT count(*) FROM documents").fetchone()[0]
        assert (
            count
            == db.execute("SELECT count(*) FROM documents_fts_docsize").fetchone()[0]
        )
        assert db.execute("SELECT count(*) FROM document_vectors").fetchone()[0] == 0
        return count


def campaign(args, git, ref, cases, digest, manifest):
    assert (HERE / "pilot-manifest.json").read_bytes() == render(manifest)
    report = args.report.resolve()
    report.mkdir(parents=True, exist_ok=False)
    name, rid, commit = ref["repository"], ref["repositoryId"], ref["commit"]
    with (
        tempfile.TemporaryDirectory(prefix="ctxfab-uv-pilot-") as tmp,
        origin(git, repository=name, repository_id=rid) as (port, calls),
    ):
        data = Path(tmp) / "data"
        data.mkdir(mode=0o700)
        first, second = report / "publication", report / "ranking"
        first.mkdir()
        second.mkdir()
        with backend(
            args.bundle, data, port, first, hub_port=port, preview_runtime=False
        ) as api:
            api("PUT", "/v1/github/credential", {"revision": 0, "token": FIXTURE_TOKEN})
            api(
                "PUT",
                "/v1/github/repositories",
                {
                    "revision": 1,
                    "repository": name,
                    "expectedId": rid,
                    "branch": "main",
                    "intervalMinutes": 1440,
                    "features": FEATURES,
                },
            )
            wait_for_commit(api, commit, timeout=1800)
            publication = read_manifest(data, git, commit)
            assert publication["publishedPaths"] == 1824
        paths = {f["path"] for f in manifest["files"]}
        count = project(data, paths)
        # The retained publication remains complete. Only candidate search rows are reduced.
        inf = data / "inference"
        inf.mkdir(exist_ok=True)
        subprocess.run(
            [
                "/bin/cp",
                "-c",
                (args.assets / FILENAME).resolve(strict=True),
                inf / FILENAME,
            ],
            check=True,
        )
        started = time.monotonic()
        with backend(
            args.bundle, data, port, second, hub_port=port, preview_runtime=False
        ) as api:
            initial_calls = len(calls)
            api("POST", "/v1/model-downloads", {"profile": EMBED})
            deadline = time.monotonic() + 240
            while time.monotonic() < deadline:
                progress, _ = api("GET", "/v1/retrieval")
                if (
                    progress["state"] == "ready"
                    and progress["indexed"] == progress["total"] == count
                ):
                    break
                models, _ = api("GET", "/v1/model-downloads")
                state = next(row for row in models if row["profile"] == EMBED)
                if state["state"] == "failed":
                    raise RuntimeError(state)
                time.sleep(1)
            else:
                raise TimeoutError(f"Shard indexing exceeded four minutes: {progress}")
            seconds = time.monotonic() - started
            save(report, "vector-census.json", vector_census(data, count))
            assert (
                len(calls)
                == initial_calls
                == api("GET", "/v1/status")[0]["outboundAttempts"]
            )
            rows = []
            for c in cases:
                for lang, q in c["questions"].items():
                    for mode in ("bm25", "semantic", "hybrid"):
                        t = time.monotonic()
                        hits = command(
                            args.bundle, data, "search", q, mode=mode, repository_id=rid
                        )
                        ms = round((time.monotonic() - t) * 1000, 2)
                        assert len(hits) <= 20
                        assert len(
                            {(h["source"], h["location"]["startByte"]) for h in hits}
                        ) == len(hits)
                        for h in hits:
                            assert (
                                h["kind"] == "current"
                                and h["location"]["path"] in paths
                            )
                            verify_evidence(
                                git, h, commit, repository=name, repository_id=rid
                            )
                        rows.append(
                            {
                                "id": c["id"],
                                "split": c["split"],
                                "language": lang,
                                "mode": mode,
                                "searchMs": ms,
                                "ranking": [h["location"] for h in hits],
                                "retrieval": score_search(c, hits),
                            }
                        )
            assert (
                len(calls)
                == initial_calls
                == api("GET", "/v1/status")[0]["outboundAttempts"]
            )
            save(report, "rows.json", rows)
            save(report, "paired.json", compare(rows))
            save(
                report,
                "summary.json",
                {
                    "commit": commit,
                    "goldSha256": digest,
                    "manifestSha256": hashlib.sha256(render(manifest)).hexdigest(),
                    "paths": len(paths),
                    "filesInRetainedPublication": publication["publishedPaths"],
                    "passages": count,
                    "secondsFromHelperLaunchThroughIndexReady": round(seconds, 2),
                    "index": progress,
                    "attemptsThroughPublication": initial_calls,
                    "attemptsAfterQueries": len(calls),
                    "results": summarize(rows),
                },
            )


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Assertions must be enabled.")
    os.environ.update(GIT_NO_LAZY_FETCH="1", GIT_NO_REPLACE_OBJECTS="1")
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("repo", type=Path)
    p.add_argument("--freeze", action="store_true")
    for key in ("bundle", "report", "assets"):
        p.add_argument("--" + key, type=Path)
    args = p.parse_args()
    ref = json.loads(REFERENCE.read_text())
    git, _ = inspect_git(args.repo, ref)
    cases, digest = load_gold(git)
    manifest = freeze(git, cases)
    if args.freeze:
        sys.stdout.buffer.write(render(manifest))
    else:
        if not all((args.report, args.bundle, args.assets)):
            p.error("Specify --report, --bundle and --assets.")
        campaign(args, git, ref, cases, digest, manifest)
