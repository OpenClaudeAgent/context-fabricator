"""Recheck retained JSON rankings, generated contexts, quotes and scores against Git."""

import argparse
import json
import os
import sys
from functools import lru_cache
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from dogfood import verify_evidence
from public_reference import REFERENCE, inspect_git
from score import load_gold, score_answer, score_search, summarize


def verify(repo, report):
    ref = json.loads(REFERENCE.read_text())
    git, _ = inspect_git(repo, ref)
    git.blob = lru_cache(maxsize=128)(git.blob)
    cases, digest = load_gold(git)
    cases = {c["id"]: c for c in cases}
    summary = json.loads((report / "summary.json").read_text())
    assert summary["goldSha256"] == digest and summary["commit"] == git.tip
    rows = json.loads((report / "rows.json").read_text())
    counted = set()
    contexts = citations = 0
    for r in rows:
        c = cases[r["id"]]
        key = (c["id"], r["language"], r["mode"])
        assert key not in counted and r["split"] == c["split"]
        counted.add(key)

        def reopen(hit):
            assert hit["kind"] == "current"
            return verify_evidence(
                git,
                hit,
                git.tip,
                repository=ref["repository"],
                repository_id=ref["repositoryId"],
            )

        hits = []
        for loc in r["ranking"]:
            h = {
                "source": f"github:{ref['repositoryId']}:{loc['path']}",
                "kind": "current",
                "revision": git.tip,
                "trackedRef": "refs/heads/main",
                "location": loc,
            }
            reopen(h)
            hits.append(h)
        assert len(hits) <= 20 and len(
            {(h["source"], h["location"]["startByte"]) for h in hits}
        ) == len(hits)
        assert score_search(c, hits) == r["retrieval"]
        if "answer" in r:
            a = r["answer"]
            assert score_answer(c, a, reopen, git, r["mode"]) == r["assessment"]
            citations += len(a["citations"])
            ctx = a.get("modelContext")
            if ctx is not None:
                assert ctx["question"] == c["questions"][r["language"]]
                for source in ctx["sources"]:
                    for part in source["parts"]:
                        assert part["revision"] == git.tip
                        raw = git.blob(git.files[git.tip][part["path"]]["sha"]).decode()
                        lines = raw.splitlines(keepends=True)
                        assert part["text"] in "".join(
                            lines[part["startLine"] - 1 : part["endLine"]]
                        )
                        contexts += 1
    assert summarize(rows) == summary["results"]
    return {
        "goldSha256": digest,
        "rows": len(rows),
        "rankedHits": sum(len(r["ranking"]) for r in rows),
        "contextExcerptsReopened": contexts,
        "citationsChecked": citations,
    }


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Assertions must be enabled.")
    os.environ.update(GIT_NO_LAZY_FETCH="1", GIT_NO_REPLACE_OBJECTS="1")
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("repo", type=Path)
    p.add_argument("report", type=Path)
    args = p.parse_args()
    print(json.dumps(verify(args.repo, args.report), indent=2))
