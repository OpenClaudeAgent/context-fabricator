"""Pair independently reviewed answers by intent, retrieval mode and language."""

import argparse
import itertools
import json
from collections import defaultdict
from pathlib import Path
from statistics import mean

from compare import paired
from review import GOLD, adjudicate, sheet


def evaluate(rows, labels):
    cases = json.loads(GOLD.read_text())["cases"]
    by_id = {c["id"]: c for c in cases}
    expected = {(c["id"], lang) for c in cases for lang in c["questions"]}
    result = []
    for mode, decisions in labels.items():
        selected = [r for r in rows if "answer" in r and r["mode"] == mode]
        assert len(selected) == len(expected)
        assert {(r["id"], r["language"]) for r in selected} == expected
        reviewed = adjudicate(sheet(selected, cases), decisions)
        by_key = {r["key"]: r for r in reviewed["results"]}
        for row in selected:
            case = by_id[row["id"]]
            [blank] = sheet([row], [case])
            judgment = by_key[blank["key"]]
            operational = row["assessment"]
            result.append(
                {
                    "id": case["id"],
                    "language": row["language"],
                    "split": case["split"],
                    "type": case["style"],
                    "mode": mode,
                    "answerable": bool(case["facts"]),
                    "factRecall": judgment["factsCorrect"] / len(case["facts"])
                    if case["facts"]
                    else None,
                    "fullySupported": judgment["fullySupportedAnswer"],
                    "responseLanguageAppropriate": judgment["languageAppropriate"],
                    "outcome": operational["outcome"],
                    "reason": operational["reason"],
                    "answerabilityCorrect": operational["answerabilityCorrect"],
                    "knownAnchorCitationRecall": operational["citationFactRecall"],
                    "knownAnchorContextRecall": operational["contextFactRecall"],
                    "answerMs": row["answerMs"],
                }
            )
    grouped = defaultdict(list)
    for r in result:
        for split in (r["split"], "all"):
            grouped[(split, r["mode"], r["language"])].append(r)
    summaries = []
    for (split, mode, lang), group in sorted(grouped.items()):
        positive = [r for r in group if r["answerable"]]
        negative = [r for r in group if not r["answerable"]]
        summaries.append(
            {
                "split": split,
                "mode": mode,
                "language": lang,
                "questions": len(group),
                "answerable": len(positive),
                "unanswerable": len(negative),
                "answeredAnswerable": sum(r["outcome"] == "answered" for r in positive),
                "falseAbstentions": sum(
                    r["outcome"] == "insufficientEvidence" for r in positive
                ),
                "correctAbstentions": sum(r["answerabilityCorrect"] for r in negative),
                "unsupportedAnswersToUnanswerable": sum(
                    r["outcome"] == "answered" for r in negative
                ),
                "fullySupportedAnswers": sum(r["fullySupported"] for r in positive),
                "fullySupportedInQuestionLanguage": sum(
                    bool(r["fullySupported"] and r["responseLanguageAppropriate"])
                    for r in positive
                ),
                "macroFactRecall": mean(r["factRecall"] for r in positive),
            }
        )
    contrasts = []
    for split in ("dev", "heldout", "all"):
        systems = sorted({(mode, lang) for sp, mode, lang in grouped if sp == split})
        for left, right in itertools.combinations(systems, 2):
            if left[0] != right[0] and left[1] != right[1]:
                continue
            for metric in ("factRecall", "fullySupported"):

                def values(key, metric, split):
                    return {
                        r["id"]: float(r[metric])
                        for r in grouped[(split, *key)]
                        if r["answerable"]
                    }

                contrasts.append(
                    {
                        "split": split,
                        "metric": metric,
                        "left": left,
                        "right": right,
                        **paired(
                            values(left, metric, split), values(right, metric, split)
                        ),
                    }
                )
    return {"rows": result, "summary": summaries, "pairedComparisons": contrasts}


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Assertions must be enabled.")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rows", type=Path)
    parser.add_argument("--bm25", type=Path, required=True)
    parser.add_argument("--hybrid", type=Path)
    args = parser.parse_args()
    labels = {
        key: json.loads(path.read_text())
        for key, path in (("bm25", args.bm25), ("hybrid", args.hybrid))
        if path
    }
    print(
        json.dumps(
            evaluate(json.loads(args.rows.read_text()), labels),
            indent=2,
            ensure_ascii=False,
        )
    )
