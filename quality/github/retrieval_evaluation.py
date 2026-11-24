"""Evaluate passage recall on frozen identifier-free GitHub questions."""

import json
import time
from pathlib import Path
from statistics import mean

MODES = ("bm25", "semantic", "hybrid")
FUSION_MODE = "equalWeightRrf"


def paraphrases():
    return [
        {**case, "language": language, "query": case[language]}
        for case in json.loads(
            Path(__file__).with_name("retrieval-judgments.json").read_text(encoding="utf-8")
        )
        for language in ("en", "fr")
    ]


def identity(hit):
    location = hit["location"]
    return (
        hit["source"], hit["revision"], hit["kind"], hit["trackedRef"],
        location["path"], location["blob"], location["startByte"], location["endByte"],
        location["startLine"], location["endLine"],
    )


def fuse(lexical, dense, dense_weight):
    scores = {}
    for hits, weight in ((lexical, 1), (dense, dense_weight)):
        for rank, key in enumerate(hits, 61):
            scores[key] = scores.get(key, 0) + weight / rank
    return sorted(scores, key=scores.__getitem__, reverse=True)


def rank_case(search, reopen, case):
    ranked, elapsed, relevant = {}, {}, set()
    line = case.get("line")
    for mode in MODES:
        started = time.monotonic()
        hits = search(case["query"], mode, "main")
        elapsed[mode] = round((time.monotonic() - started) * 1000, 3)
        assert len(hits) <= 20
        ranked[mode] = []
        for hit in hits:
            assert hit["kind"] == "current"
            key = identity(hit)
            ranked[mode].append(key)
            passage = reopen(hit)
            location = hit["location"]
            if (location["path"] == case["path"] and case["anchor"] in passage
                    and (line is None or location["startLine"] <= line <= location["endLine"])):
                relevant.add(key)
        assert len(set(ranked[mode])) == len(ranked[mode])
    lexical, dense = ranked["bm25"], ranked["semantic"]
    equal = fuse(lexical, dense, 1)[:20]
    expected = equal
    if case["language"] != "identifier":
        expected = fuse(lexical, dense, 1.25)
        if lexical:
            position = expected.index(lexical[0])
            if position >= 5:
                expected.insert(4, expected.pop(position))
        expected = expected[:20]
    assert expected == ranked["hybrid"], case["id"]
    ranked[FUSION_MODE] = equal
    return [
        {
            "id": case["id"], "language": case["language"], "mode": mode, "question": case["query"],
            "expectedPath": case["path"], "expectedAnchor": case["anchor"],
            "anchorRank": next((i for i, key in enumerate(keys, 1) if key in relevant), None),
            "firstHitPath": keys[0][4] if keys else None,
            "elapsedMilliseconds": elapsed.get(mode),
        }
        for mode, keys in ranked.items()
    ]


def evaluate(search, reopen, source, cases=None):
    if not __debug__:
        raise RuntimeError("Retrieval qualification requires Python assertions.")
    if cases is None:
        cases = paraphrases()
    results = []
    for case in cases:
        blob = source(case["path"])
        if "line" in case:
            assert case["anchor"].encode() in blob.splitlines()[case["line"] - 1], case["id"]
        else:
            assert blob.count(case["anchor"].encode()) == 1, case["id"]
        results.extend(rank_case(search, reopen, case))

    for mode in MODES:
        assert search(cases[0]["query"], mode, "unindexed") == []
    summary = {}
    for mode in (*MODES, FUSION_MODE):
        summary[mode] = {}
        for language in sorted({case["language"] for case in cases}):
            ranks = [row["anchorRank"] or 0 for row in results
                     if row["mode"] == mode and row["language"] == language]
            summary[mode][language] = {
                "queries": len(ranks),
                "hitsAt5": sum(0 < rank <= 5 for rank in ranks),
                "hitsAt20": sum(rank > 0 for rank in ranks),
                "mrrAt20": round(mean(1 / rank if rank else 0 for rank in ranks), 4),
            }
    return {"summary": summary, "rankings": results, "emptyUnindexedBranchModes": list(MODES)}
