import json
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from answer_pairs import evaluate
from review import GOLD, adjudicate, sheet
from score import score_answer, score_exact_search, score_search


def loc(path, start, end):
    return {"path": path, "blob": path, "startLine": start, "endLine": end}


class ScoringTests(unittest.TestCase):
    def setUp(self):
        self.case = {
            "id": "pair",
            "questions": {"en": "Which pair?"},
            "facts": [
                {"expected": "first", "anchors": [loc("a", 1, 2)]},
                {"expected": "second", "anchors": [loc("b", 2, 3)]},
            ],
        }

    def test_duplicate_does_not_supply_second_fact(self):
        hits = [{"location": loc("a", 1, 5)}] * 2 + [{"location": loc("b", 2, 2)}]
        score = score_search(self.case, hits)
        self.assertEqual(score["firstRanks"], [1, None])
        self.assertEqual(score["factRecall"]["5"], 0.5)
        self.assertFalse(score["allFactsAt5"])

    def test_all_facts_and_alternative(self):
        self.case["facts"][1]["anchors"].append(loc("c", 3, 4))
        hits = [{"location": loc("c", 1, 5)}, {"location": loc("a", 1, 5)}]
        self.assertTrue(score_search(self.case, hits)["allFactsAt5"])

    def test_unanswerable_is_excluded_from_retrieval(self):
        self.assertIsNone(score_search({"facts": []}, []))

    def test_blind_fact_requires_each_anchor(self):
        anchors = [dict(loc(p, 1, 1), requiredBytes=[(0, 4)]) for p in ("a", "b")]
        case = {"split": "blind", "facts": [{"anchors": anchors}]}
        hits = [{"location": dict(loc(p, 1, 1), startByte=0, endByte=4)} for p in ("a", "b")]
        self.assertEqual(score_search(case, hits[:1])["firstRanks"], [None])
        self.assertEqual(score_search(case, hits)["firstRanks"], [2])
        self.assertEqual(score_exact_search(case, hits)["combinedPassagesFirstRanks"], [2])
        self.assertEqual(score_exact_search(case, hits[:1])["singlePassageFirstRanks"], [None])

    def test_byte_coverage_distinguishes_partial_lines_and_combined_passages(self):
        anchor = dict(loc("a", 1, 1), requiredBytes=[(10, 30)])
        case = {"facts": [{"anchors": [anchor]}]}
        hits = [
            {"location": dict(loc("a", 1, 1), startByte=10, endByte=20)},
            {"location": dict(loc("a", 1, 1), blob="other", startByte=20, endByte=30)},
            {"location": dict(loc("a", 1, 1), startByte=20, endByte=30)},
        ]
        self.assertEqual(score_search(case, hits)["firstRanks"], [1])
        score = score_exact_search(case, hits)
        self.assertEqual(score["singlePassageFirstRanks"], [None])
        self.assertEqual(score["combinedPassagesFirstRanks"], [3])

    def test_unavailable_worker_is_not_a_correct_abstention(self):
        answer = {
            "outcome": "insufficientEvidence",
            "mode": "bm25",
            "citations": [],
            "evidenceReview": {"reason": "workerUnavailable"},
        }
        self.assertFalse(
            score_answer({"facts": []}, answer, None, None, "bm25")[
                "answerabilityCorrect"
            ]
        )
        answer["evidenceReview"]["reason"] = "noCitations"
        self.assertTrue(
            score_answer({"facts": []}, answer, None, None, "bm25")[
                "answerabilityCorrect"
            ]
        )

    def test_quote_from_correct_line_must_cover_the_anchor_text(self):
        location = dict(loc("a", 1, 1), fileUrl="https://example.test/a")
        case = {
            "facts": [
                {
                    "expected": "eight",
                    "anchors": [dict(loc("a", 1, 1), text="The answer is eight.")],
                }
            ]
        }
        citation = {
            "lineUrl": "https://example.test/a#L1",
            "quote": "The answer",
            "evidence": {"location": location},
        }
        answer = {"mode": "bm25", "outcome": "answered", "citations": [citation]}
        result = score_answer(
            case,
            answer,
            lambda _: "The answer is eight.",
            SimpleNamespace(tip="abc"),
            "bm25",
        )
        self.assertEqual(result["citationFactRecall"], 0)
        self.assertIsNone(result["contextFactRecall"])

    def test_paired_answer_denominators_keep_abstentions(self):
        cases = json.loads(GOLD.read_text())["cases"]
        rows = []
        for c in cases:
            for lang in c["questions"]:
                rows.append(
                    {
                        "id": c["id"],
                        "mode": "bm25",
                        "language": lang,
                        "answerMs": 10,
                        "answer": {"outcome": "insufficientEvidence", "citations": []},
                        "assessment": {
                            "outcome": "insufficientEvidence",
                            "reason": "noCitations",
                            "answerabilityCorrect": not c["facts"],
                            "contextFactRecall": None,
                            "citationFactRecall": 0,
                        },
                    }
                )
        labels = sheet(rows, cases)
        for r in labels:
            r.update(
                factCorrect=[False] * len(r["facts"]),
                factSupported=[False] * len(r["facts"]),
                contradictoryClaim=False,
                unsupportedMaterialClaim=False,
                languageAppropriate=False,
            )
        result = evaluate(rows, {"bm25": labels})
        en = next(
            s
            for s in result["summary"]
            if s["split"] == "all" and s["language"] == "en"
        )
        self.assertEqual(
            (en["questions"], en["falseAbstentions"], en["correctAbstentions"]),
            (11, 9, 2),
        )
        self.assertEqual(len(result["pairedComparisons"]), 6)

    def test_review_requires_a_complete_and_consistent_answer(self):
        row = {
            "id": "pair",
            "language": "en",
            "answer": {
                "outcome": "answered",
                "text": "first and second and their opposites",
                "citations": [],
            },
        }
        blank = sheet([row], [self.case])
        completed = [
            dict(
                blank[0],
                factCorrect=[True, True],
                factSupported=[True, True],
                contradictoryClaim=True,
                unsupportedMaterialClaim=False,
                languageAppropriate=True,
            )
        ]
        self.assertEqual(adjudicate(blank, completed)["fullySupportedAnswers"], 0)
        with self.assertRaises(AssertionError):
            adjudicate(blank, blank)


if __name__ == "__main__":
    unittest.main()
