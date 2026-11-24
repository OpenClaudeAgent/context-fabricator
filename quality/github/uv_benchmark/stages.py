"""Describe observed failure stages without inferring missing completion context."""

import argparse
import json
from collections import Counter, defaultdict
from pathlib import Path
from statistics import mean, median


def analyze(rows, traces):
    groups = defaultdict(list)
    for r in rows:
        if "answer" not in r:
            continue
        t = traces.get(r["trace"], {}).get("trace", {})
        generations = [
            span for span in t.get("spans", []) if span["name"] == "answer_generation"
        ]
        budgets = [s["contextBudget"] for s in generations if "contextBudget" in s]
        timing = [s["inferenceTiming"] for s in generations if "inferenceTiming" in s]
        scored = r["assessment"]
        answer = r["answer"]
        ctx = answer.get("modelContext")
        groups[r["mode"]].append(
            {
                "id": r["id"],
                "language": r["language"],
                "split": r["split"],
                "standaloneSearchCompleteAt5": r["retrieval"]["allFactsAt5"]
                if r["retrieval"]
                else None,
                "outcome": answer["outcome"],
                "reason": scored["reason"],
                "answerMs": r["answerMs"],
                "terminalAnswerTracePresent": bool(t),
                "measuredCompletionBudgets": budgets,
                "generationTimings": timing,
                "exactContextSourceCount": len(ctx["sources"]) if ctx else None,
                "knownAnchorContextRecall": scored["contextFactRecall"],
                "citations": len(answer["citations"]),
                "knownAnchorCitationRecall": scored["citationFactRecall"],
            }
        )
    output = {}
    for mode, items in groups.items():
        p = [r for r in items if r["standaloneSearchCompleteAt5"] is not None]
        missed = [r for r in p if r["outcome"] == "insufficientEvidence"]
        budgets = [b for r in items for b in r["measuredCompletionBudgets"]]
        tokens = [b["inputTokens"] for b in budgets if "inputTokens" in b]
        elapsed = [r["answerMs"] for r in items]
        output[mode] = {
            "questions": len(items),
            "answerable": len(p),
            "falseAbstentions": len(missed),
            "falseAbstentionReasons": dict(Counter(r["reason"] for r in missed)),
            "falseAbstentionsWithAllStandaloneKnownFactsAt5": sum(
                r["standaloneSearchCompleteAt5"] for r in missed
            ),
            "answered": sum(r["outcome"] == "answered" for r in items),
            "questionsWithExactContext": sum(
                r["exactContextSourceCount"] is not None for r in items
            ),
            "questionsWithTerminalTrace": sum(
                r["terminalAnswerTracePresent"] for r in items
            ),
            "questionsWithMeasuredBudget": sum(
                bool(r["measuredCompletionBudgets"]) for r in items
            ),
            "budgetRecords": len(budgets),
            "budgetRecordsTruncated": sum(
                b["admitted"] < b["proposed"] for b in budgets
            ),
            "medianInputTokensPerBudgetRecord": median(tokens) if tokens else None,
            "medianAnswerMs": median(elapsed),
            "meanAnswerMs": mean(elapsed),
            "rows": items,
        }
    return output


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("report", type=Path)
    args = p.parse_args()
    rows = json.loads((args.report / "rows.json").read_text())
    traces = json.loads((args.report / "answer-traces.json").read_text())
    print(json.dumps(analyze(rows, traces), indent=2))
