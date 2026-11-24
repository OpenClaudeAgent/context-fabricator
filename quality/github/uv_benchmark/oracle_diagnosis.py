"""Pair oracle drafts, separately reviewed product answers, and trace-supported failure stages."""

import argparse
import json
import re
from collections import Counter
from pathlib import Path

from review import GOLD, adjudicate, sheet


def read(path):
    return json.loads(path.read_text())


def coverage(case, places, commit):
    def contains(place, anchor):
        return (
            place.get("revision") == commit
            and place.get("reference", place.get("path")) == anchor["path"]
            and place.get("startLine", 0) <= anchor["startLine"]
            and place.get("endLine", 0) >= anchor["endLine"]
        )

    return sum(
        any(contains(p, a) for p in places for a in f["anchors"]) for f in case["facts"]
    )


def product(row, judgment, case, commit):
    answer = row["answer"]
    facts = len(case["facts"])
    audit = answer.get("evidenceReview", {})
    places = [p for p in audit.get("candidates", []) if p["source"] == "localPassages"]
    reason = audit.get("reason")
    data = {
        "outcome": answer["outcome"],
        "productReason": reason,
        "fullySupported": judgment["fullySupported"],
        "standaloneRanks": None if not facts else row["retrieval"]["firstRanks"],
        "disclosedContextFactRecall": row["assessment"]["contextFactRecall"],
    }
    if not facts:
        data["stage"] = "control"
    elif judgment["fullySupported"]:
        data["stage"] = "fully-supported"
    elif answer["outcome"] == "answered":
        data["stage"] = "published-with-factual-or-support-gap"
    elif not places:
        data["stage"] = "request-or-observation-unavailable"
    else:
        ranked = [
            p for p in places if p.get("rank") and p["disposition"] != "fixtureExcluded"
        ]
        proposed = [p for p in places if "slot" in p]
        admitted = [
            p
            for p in proposed
            if p["disposition"] in ("notCited", "citationUnverified")
        ]
        counts = [coverage(case, z, commit) for z in (ranked, proposed, admitted)]
        data["knownFactsInRankedProposedAdmitted"] = counts
        # A follower can supply evidence missing in the initial ranking. Diagnose
        # downstream first to avoid blaming retrieval for a recovered anchor.
        if counts[2] == facts:
            data["stage"] = "all-anchors-admitted-" + str(reason)
        elif counts[1] == facts:
            data["stage"] = "admission-lost-known-support"
        elif counts[0] == facts:
            data["stage"] = "selection-lost-known-support"
        else:
            data["stage"] = "candidate-support-incomplete"
    return data


def quote_category(quote, passage):
    literal = quote.strip()
    if len(literal.encode()) < 8:
        return "empty-or-short"
    if passage.count(literal) == 1:
        return "unique-literal"
    if literal in passage:
        return "repeated-literal"

    def flatten(text):
        return " ".join(text.split())

    if flatten(literal) in flatten(passage):
        return "line-or-whitespace-flattened"

    def drop_markup(text):
        return flatten(
            re.sub(r"(?m)^\s*(?:///|//!|>)\s?", "", text)
            .replace("`", "")
            .replace("*", "")
        )

    if drop_markup(literal) in drop_markup(passage):
        return "formatting-or-comment-prefix-removed"
    return "other-text-change"


def oracle(folder, cases):
    rows = read(folder / "rows.json")
    labels = folder / "review-graded.json"
    assessed = {}
    if labels.exists():
        reviewed = adjudicate(read(folder / "review.json"), read(labels))
        assessed = {x["key"]: x for x in reviewed["results"]}
    result = {}
    prompts = {
        (r["id"], r["language"]): json.loads(r["payload"]["messages"][1]["content"])
        for r in read(folder / "requests.json")
    }
    for r in rows:
        a = r.get("assessment", {})
        citations = a.get("quotes", [])
        item = {
            "id": r["id"],
            "language": r["language"],
            "answer": {
                "text": a.get("text"),
                "outcome": {
                    "draft": "answered",
                    "abstained": "insufficientEvidence",
                }.get(a.get("status"), "error"),
                "citations": [
                    {
                        "quote": q["quote"],
                        "evidence": {"location": {"fileUrl": q["url"]}},
                    }
                    for q in citations
                ],
            },
        }
        [blank] = sheet([item], [cases[r["id"]]])
        result[r["id"], r["language"]] = {
            "status": a.get("status", "error"),
            "allQuotesUniqueLiteralInCitedSlot": all(
                q["uniqueLiteral"] for q in citations
            )
            if citations
            else None,
            "quoteCategories": [
                quote_category(
                    q["quote"],
                    prompts[r["id"], r["language"]]["passages"][q["passage"] - 1][
                        "text"
                    ],
                )
                for q in citations
            ],
            "rawAnswer": a.get("text"),
            "judgment": assessed.get(blank["key"]),
            "inputTokens": r.get("inputTokens"),
        }
    return result


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("product", type=Path)
    ap.add_argument("narrow", type=Path)
    ap.add_argument("chunks", type=Path)
    ap.add_argument("output", type=Path)
    a = ap.parse_args()
    gold = read(GOLD)
    cases = {c["id"]: c for c in gold["cases"]}
    paired = {
        (x["id"], x["language"], x["mode"]): x
        for x in read(a.product / "paired-answers.json")["rows"]
    }
    actual = {
        (r["id"], r["language"], r["mode"]): product(
            r, paired[r["id"], r["language"], r["mode"]], cases[r["id"]], gold["commit"]
        )
        for r in read(a.product / "rows.json")
        if "answer" in r
    }
    nar = oracle(a.narrow, cases)
    wide = oracle(a.chunks, cases)
    assert len(nar) == len(wide) == 22 and len(actual) == 44
    rows = [
        {
            "id": i,
            "language": lang,
            "split": cases[i]["split"],
            "requiredFacts": len(cases[i]["facts"]),
            "bm25": actual[i, lang, "bm25"],
            "hybrid": actual[i, lang, "hybrid"],
            "oracleNarrow": nar[i, lang],
            "oracleIndexed": wide[i, lang],
        }
        for i, lang in sorted(nar)
    ]
    aggregates = {}
    for mode in ("bm25", "hybrid"):
        aggregates[mode] = dict(
            Counter(row[mode]["stage"] for row in rows if row["requiredFacts"])
        )
    for arm in ("oracleNarrow", "oracleIndexed"):
        positive = [r[arm] for r in rows if r["requiredFacts"]]
        negative = [r[arm] for r in rows if not r["requiredFacts"]]
        aggregates[arm] = {
            "positiveDrafts": sum(r["status"] == "draft" for r in positive),
            "positiveAllQuotesUniqueLiteral": sum(
                r["allQuotesUniqueLiteralInCitedSlot"] is True for r in positive
            ),
            "emptyControlDrafts": sum(r["status"] == "abstained" for r in negative),
        }
        aggregates[arm]["rawPositiveQuoteCategories"] = dict(
            Counter(cat for r in positive for cat in r["quoteCategories"])
        )
        if all(r["judgment"] for r in positive):
            aggregates[arm].update(
                allFactsCorrect=sum(
                    r["judgment"]["allFactsCorrectWithoutContradiction"]
                    for r in positive
                ),
                fullySupported=sum(
                    r["judgment"]["fullySupportedAnswer"] for r in positive
                ),
                fullySupportedAndAllQuotesUniqueLiteral=sum(
                    r["judgment"]["fullySupportedAnswer"]
                    and r["allQuotesUniqueLiteralInCitedSlot"] is True
                    for r in positive
                ),
            )
    a.output.write_text(
        json.dumps(
            {"aggregates": aggregates, "rows": rows}, ensure_ascii=False, indent=2
        )
        + "\n"
    )


if __name__ == "__main__":
    main()
