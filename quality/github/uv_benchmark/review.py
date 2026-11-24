"""Make a blinded answer-review sheet, or validate and aggregate a completed sheet."""

import argparse
import hashlib
import json
import random
from pathlib import Path
from statistics import mean

GOLD = Path(__file__).with_name("gold.json")


def sheet(rows, cases):
    references = {c["id"]: c for c in cases}
    items = []
    for row in rows:
        if "answer" not in row:
            continue
        answer = row["answer"]
        case = references[row["id"]]
        item = {
            "question": case["questions"][row["language"]],
            "text": answer.get("text"),
            "outcome": answer["outcome"],
            "facts": [f["expected"] for f in case["facts"]],
            "insufficientReason": case.get("insufficientReason"),
            "quotes": [
                {
                    "quote": q["quote"],
                    "url": q["workflowRun"]["runUrl"]
                    if "workflowRun" in q
                    else (q.get("lineUrl", q["evidence"]["location"]["fileUrl"])),
                }
                for q in answer["citations"]
            ],
        }
        item["key"] = hashlib.sha256(
            json.dumps(item, sort_keys=True).encode()
        ).hexdigest()
        item.update(
            factCorrect=[None] * len(case["facts"]),
            factSupported=[None] * len(case["facts"]),
            unsupportedMaterialClaim=None,
            contradictoryClaim=None,
            languageAppropriate=None,
        )
        items.append(item)
    if not items:
        raise ValueError("No generated answers in the supplied rows.")
    random.Random(3721).shuffle(items)
    return items


def adjudicate(blank, labels):
    assert len(labels) == len(blank)
    by_key = {r["key"]: r for r in labels}
    assert len(by_key) == len(blank)
    results = []
    for item in blank:
        row = by_key[item["key"]]
        for key in (
            "question",
            "text",
            "outcome",
            "facts",
            "quotes",
            "insufficientReason",
        ):
            assert row[key] == item[key]
        for key in ("factCorrect", "factSupported"):
            assert len(row[key]) == len(row["facts"]) and all(
                type(v) is bool for v in row[key]
            )
        assert all(
            type(row[k]) is bool
            for k in (
                "unsupportedMaterialClaim",
                "contradictoryClaim",
                "languageAppropriate",
            )
        )
        flags = row["factCorrect"]
        supported = row["factSupported"]
        correct = (
            bool(flags)
            and row["outcome"] == "answered"
            and all(flags)
            and not row["contradictoryClaim"]
        )
        results.append(
            {
                "key": row["key"],
                "factsCorrect": sum(flags),
                "factsRequired": len(flags),
                "allFactsCorrectWithoutContradiction": correct if flags else None,
                "fullySupportedAnswer": (
                    correct and all(supported) and not row["unsupportedMaterialClaim"]
                )
                if flags
                else None,
                "languageAppropriate": row["languageAppropriate"]
                if row["text"]
                else None,
            }
        )
    positives = [r for r in results if r["factsRequired"]]
    return {
        "answersReviewed": len(results),
        "answerable": len(positives),
        "macroFactCorrectness": mean(
            r["factsCorrect"] / r["factsRequired"] for r in positives
        )
        if positives
        else None,
        "fullySupportedAnswers": sum(r["fullySupportedAnswer"] for r in positives),
        "results": results,
    }


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Assertions must be enabled.")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rows", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--labels", type=Path)
    parser.add_argument("--gold", type=Path, default=GOLD)
    parser.add_argument("--mode", choices=("bm25", "hybrid"))
    args = parser.parse_args()
    rows = json.loads(args.rows.read_text())
    if args.mode:
        rows = [r for r in rows if r["mode"] == args.mode]
    blank = sheet(rows, json.loads(args.gold.read_text())["cases"])
    output = (
        adjudicate(blank, json.loads(args.labels.read_text())) if args.labels else blank
    )
    with args.output.open("x") as f:
        json.dump(output, f, indent=2, ensure_ascii=False)
        f.write("\n")
