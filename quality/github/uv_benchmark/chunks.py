"""Audit the published line projection against every original Git blob and frozen anchor."""

import argparse
import hashlib
import json
import math
import sqlite3
from collections import defaultdict
from pathlib import Path

GOLD = Path(__file__).with_name("gold.json")


def audit(data, dataset=GOLD):
    gold = json.loads(dataset.read_text())
    facts_by_path = defaultdict(list)
    observed = set()
    for case in gold["cases"]:
        for i, fact in enumerate(case["facts"]):
            for a in fact["anchors"]:
                facts_by_path[a["path"]].append(((case["id"], i), a))
    ranges = defaultdict(list)
    lengths = []
    with sqlite3.connect(
        (data / "context.sqlite3").resolve().as_uri() + "?mode=ro", uri=True
    ) as db:
        for encoded, text in db.execute(
            "SELECT source_location, content FROM documents ORDER BY source, passage_start"
        ):
            loc = json.loads(encoded)
            ranges[loc["path"]].append((loc, text.encode()))
        sql = """SELECT f.path, f.blob, b.content, s.revision FROM source_snapshot_files f
                 JOIN source_blobs b USING (collection,blob)
                 JOIN source_snapshots s ON s.id=f.snapshot_id WHERE s.phase='published'"""
        paths = 0
        whitespace_only = 0
        for path, oid, text, rev in db.execute(sql):
            raw = text.encode()
            paths += 1
            assert rev == gold["commit"]
            assert hashlib.sha1(f"blob {len(raw)}\0".encode() + raw).hexdigest() == oid
            offset = 0
            parts = ranges.pop(path, [])
            whitespace_only += not parts
            for loc, content in parts:
                start, end = loc["startByte"], loc["endByte"]
                assert start >= offset and not raw[offset:start].decode().strip()
                assert (
                    loc["blob"] == oid
                    and 0 <= start < end <= len(raw)
                    and raw[start:end] == content
                )
                assert len(content) <= 2048
                lengths.append(len(content))
                for fact, a in facts_by_path.get(path, []):
                    if (
                        loc["startLine"]
                        <= a["startLine"]
                        <= a["endLine"]
                        <= loc["endLine"]
                        and a["text"].encode() in content
                    ):
                        observed.add(fact)
                offset = end
            assert not raw[offset:].decode().strip()
    assert not ranges and paths > 0 and lengths
    lengths.sort()
    return {
        "commit": gold["commit"],
        "publishedPaths": paths,
        "pathsWithOnlyWhitespaceOrEmpty": whitespace_only,
        "passages": len(lengths),
        "bytes": sum(lengths),
        "minBytes": lengths[0],
        "medianBytes": lengths[(len(lengths) - 1) // 2],
        "p95Bytes": lengths[math.ceil(0.95 * len(lengths)) - 1],
        "maxBytes": lengths[-1],
        "facts": sum(len(c["facts"]) for c in gold["cases"]),
        "factsWithCompleteAnchorInOnePassage": len(observed),
        "gaps": [
            {"id": c["id"], "factIndex": i}
            for c in gold["cases"]
            for i in range(len(c["facts"]))
            if (c["id"], i) not in observed
        ],
    }


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Assertions must be enabled.")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("data", type=Path)
    args = parser.parse_args()
    print(json.dumps(audit(args.data), indent=2))
