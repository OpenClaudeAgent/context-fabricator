"""Source-anchored metrics. No model participates in producing or grading gold."""

import hashlib
import json
import math
import sqlite3
from collections import defaultdict
from pathlib import Path
from statistics import mean, median

GOLD = Path(__file__).with_name("gold.json")


def load_gold(git, dataset=GOLD):
    raw = dataset.read_bytes()
    gold = json.loads(raw)
    assert gold["commit"] == git.tip
    cases = gold["cases"]
    assert len({case["id"] for case in cases}) == len(cases)
    for case in cases:
        assert set(case["questions"]) == {"en", "fr"}
        assert case["split"] in ("dev", "heldout", "blind")
        assert bool(case["facts"]) != bool(case.get("insufficientReason"))
        for fact in case["facts"]:
            assert fact["anchors"] and fact["expected"]
            for anchor in fact["anchors"]:
                assert git.files[git.tip][anchor["path"]]["sha"] == anchor["blob"]
                lines = git.blob(anchor["blob"]).decode().splitlines(keepends=True)
                start, end = anchor["startLine"], anchor["endLine"]
                assert 1 <= start <= end <= len(lines)
                window = lines[start - 1 : end]
                assert "".join(window).splitlines() == anchor["text"].splitlines()
                offset = len("".join(lines[: start - 1]).encode())
                required = []
                for line in window:
                    if line.strip():
                        leading = len(line.encode()) - len(line.lstrip().encode())
                        required.append((offset + leading, offset + len(line.rstrip().encode())))
                    offset += len(line.encode())
                assert required, (case["id"], anchor)
                anchor["requiredBytes"] = required
    return cases, hashlib.sha256(raw).hexdigest()


def covers(location, anchor):
    return (
        location["path"] == anchor["path"]
        and location["blob"] == anchor["blob"]
        and location["startLine"] <= anchor["startLine"]
        and location["endLine"] >= anchor["endLine"]
    )


def score_search(case, hits):
    facts = case["facts"]
    if not facts:
        return None
    anchor_ranks = [[next((i for i, hit in enumerate(hits, 1)
                           if covers(hit["location"], anchor)), None)
                     for anchor in fact["anchors"]] for fact in facts]
    # Blind judgments use all listed spans as necessary evidence; legacy
    # datasets use anchors as alternatives. Preserve the latter semantics.
    ranks = [aggregate_ranks(r, case.get("split") == "blind") for r in anchor_ranks]
    return {
        "firstRanks": ranks,
        "anchorFirstRanks": anchor_ranks,
        "rrAt20": max((1 / r for r in ranks if r), default=0),
        "factRecall": {
            str(k): sum(r is not None and r <= k for r in ranks) / len(facts)
            for k in (1, 5, 10, 20)
        },
        "allFactsAt5": all(r is not None and r <= 5 for r in ranks),
    }


def includes_ranges(passages, required):
    for start, end in required:
        covered = start
        for left, right in passages:
            if left > covered:
                break
            covered = max(covered, right)
            if covered >= end:
                break
        if covered < end:
            return False
    return True


def aggregate_ranks(ranks, require_all):
    if require_all:
        return max(ranks) if all(rank is not None for rank in ranks) else None
    return min((rank for rank in ranks if rank is not None), default=None)


def score_exact_search(case, hits):
    """Measure exact source bytes, with and without combining passages of one blob."""
    if not case["facts"]:
        return None
    single, combined = [], []
    all_single, all_combined = [], []
    for fact in case["facts"]:
        single_ranks, combined_ranks = [], []
        for anchor in fact["anchors"]:
            single_rank, combined_rank = None, None
            required = anchor["requiredBytes"]
            spans = []
            for rank, hit in enumerate(hits, 1):
                location = hit["location"]
                if (location["path"], location["blob"]) != (anchor["path"], anchor["blob"]):
                    continue
                span = (location["startByte"], location["endByte"])
                if single_rank is None and includes_ranges([span], required):
                    single_rank = rank
                spans.append(span)
                if combined_rank is None and includes_ranges(sorted(spans), required):
                    combined_rank = rank
            single_ranks.append(single_rank)
            combined_ranks.append(combined_rank)
        all_single.append(single_ranks)
        all_combined.append(combined_ranks)
        single.append(aggregate_ranks(single_ranks, case.get("split") == "blind"))
        combined.append(aggregate_ranks(combined_ranks, case.get("split") == "blind"))
    return {
        "singlePassageFirstRanks": single,
        "combinedPassagesFirstRanks": combined,
        "singlePassageAnchorFirstRanks": all_single,
        "combinedPassagesAnchorFirstRanks": all_combined,
    }


def audit_index(data, git, commit):
    path = data / "context.sqlite3"
    with sqlite3.connect(path.as_uri() + "?mode=ro", uri=True) as db:
        rows = db.execute(
            "SELECT source_location, content, revision FROM documents ORDER BY source, passage_start"
        )
        count, last_blob, source = 0, None, b""
        for encoded, content, rev in rows:
            loc = json.loads(encoded)
            assert (
                rev == commit and git.files[commit][loc["path"]]["sha"] == loc["blob"]
            )
            if last_blob != loc["blob"]:
                last_blob = loc["blob"]
                source = git.blob(last_blob)
            start, end = loc["startByte"], loc["endByte"]
            assert source[start:end] == content.encode()
            assert loc["startLine"] == 1 + source.count(b"\n", 0, start)
            assert loc["endLine"] == source.count(b"\n", 0, end) + (
                not content.endswith("\n")
            )
            count += 1
        assert count > 0
        # docsize contains one row per canonical FTS document, including contentless FTS tables.
        assert (
            db.execute("SELECT count(*) FROM documents_fts_docsize").fetchone()[0]
            == count
        )
        assert not db.execute(
            "SELECT id FROM documents EXCEPT SELECT id FROM documents_fts_docsize"
        ).fetchone()
        return {
            "passages": count,
            "sqliteBytes": path.stat().st_size,
            "walBytes": Path(str(path) + "-wal").stat().st_size
            if Path(str(path) + "-wal").exists()
            else 0,
        }


def score_answer(case, answer, reopen, git, mode):
    import re
    from urllib.parse import urlsplit

    from dogfood import verify_quote_link

    assert answer["mode"] == mode
    facts = case["facts"]
    sources = answer.get("modelContext", {}).get("sources")
    context = set()
    for source in sources or []:
        for part in source["parts"]:
            assert part["revision"] == git.tip
            for i, fact in enumerate(facts):
                for a in fact["anchors"]:
                    if (
                        part["path"] == a["path"]
                        and part["startLine"] <= a["startLine"]
                        and a["endLine"] <= part["endLine"]
                        and a["text"] in part["text"]
                    ):
                        context.add(i)
    cited, relevant = set(), 0
    for citation in answer["citations"]:
        passage = reopen(citation["evidence"])
        verify_quote_link(citation, passage)
        fragment = urlsplit(citation.get("lineUrl", "")).fragment
        span = re.fullmatch(r"L(\d+)(?:-L(\d+))?", fragment)
        if not span:
            continue
        loc = dict(
            citation["evidence"]["location"],
            startLine=int(span[1]),
            endLine=int(span[2] or span[1]),
        )
        supported = {
            i
            for i, f in enumerate(facts)
            if any(
                covers(loc, a) and a["text"].strip() in citation["quote"]
                for a in f["anchors"]
            )
        }
        cited.update(supported)
        relevant += bool(supported)
    withheld = answer["outcome"] == "insufficientEvidence"
    reason = answer.get("evidenceReview", {}).get("reason")
    return {
        "answerabilityCorrect": (
            not facts
            and withheld
            and reason in ("noCandidates", "noUsableCandidates", "noCitations")
        )
        or (bool(facts) and answer["outcome"] == "answered"),
        "outcome": answer["outcome"],
        "reason": reason,
        "contextFactRecall": len(context) / len(facts)
        if facts and sources is not None
        else None,
        "citationFactRecall": len(cited) / len(facts) if facts else None,
        "knownAnchorCitationPrecision": relevant / len(answer["citations"])
        if answer["citations"]
        else None,
    }


def summarize(rows):
    groups = defaultdict(list)
    for row in rows:
        groups[row["split"], row["language"], row["mode"]].append(row)
    results = []
    for (split, lang, mode), group in sorted(groups.items()):
        positives = [r["retrieval"] for r in group if r["retrieval"] is not None]
        ms = sorted(r["searchMs"] for r in group)
        results.append(
            {
                "split": split,
                "language": lang,
                "mode": mode,
                "queries": len(group),
                "answerable": len(positives),
                "meanReciprocalRankAt20": mean(p["rrAt20"] for p in positives),
                "macroFactRecallAt5": mean(p["factRecall"]["5"] for p in positives),
                "allFactsAt5Count": sum(p["allFactsAt5"] for p in positives),
                "medianSearchMs": median(ms),
                "p95SearchMs": ms[math.ceil(0.95 * len(ms)) - 1],
            }
        )
    return results


def vector_census(data, required):
    """Require exactly one correctly sized embedding per searchable passage."""
    with sqlite3.connect(
        (data / "context.sqlite3").resolve().as_uri() + "?mode=ro", uri=True
    ) as db:
        n = db.execute("SELECT count(*) FROM documents").fetchone()[0]
        rows = db.execute("""SELECT profile, count(*), min(length(embedding)), max(length(embedding)),
                           sum(length(embedding)) FROM document_vectors GROUP BY profile""").fetchall()
        missing = db.execute(
            "SELECT count(*) FROM documents d LEFT JOIN document_vectors v ON v.document_id=d.id WHERE v.document_id IS NULL"
        ).fetchone()[0]
        extra = db.execute(
            "SELECT count(*) FROM document_vectors v LEFT JOIN documents d ON d.id=v.document_id WHERE d.id IS NULL"
        ).fetchone()[0]
    assert rows == [
        (
            "qwen3-embedding-0.6b-q8_0-06507c7b-d1024-v1",
            required,
            4096,
            4096,
            4096 * required,
        )
    ]
    assert n == required and missing == extra == 0
    return {
        "profile": rows[0][0],
        "indexedPassages": n,
        "payloadBytes": rows[0][4],
        "missingDocumentVectors": missing,
        "orphanVectors": extra,
    }
