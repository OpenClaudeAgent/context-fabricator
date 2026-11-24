"""Paired intent-level differences in top-five supporting-fact recall."""

import argparse
import itertools
import json
from pathlib import Path
from statistics import mean


def paired(a, b):
    if a.keys() != b.keys():
        raise ValueError("Comparisons require exactly the same answerable intents.")
    deltas = {key: b[key] - a[key] for key in sorted(a)}
    values = list(deltas.values())
    observed = mean(values)
    signs = itertools.product((-1, 1), repeat=len(values))
    extreme = sum(
        abs(mean(s * v for s, v in zip(sign, values))) >= abs(observed) - 1e-12
        for sign in signs
    )
    return {
        "pairedIntents": len(values),
        "meanDelta": observed,
        "intentDeltas": deltas,
        "twoSidedExactSignFlipP": extreme / 2 ** len(values),
    }


def compare(rows):
    buckets = {}
    for row in rows:
        if row["retrieval"] is None or row["split"] != "heldout":
            continue
        key = (row["mode"], row["language"])
        by_id = buckets.setdefault(key, {})
        assert row["id"] not in by_id
        by_id[row["id"]] = row["retrieval"]["factRecall"]["5"]
    result = []
    for left, right in itertools.combinations(sorted(buckets), 2):
        if left[0] != right[0] and left[1] != right[1]:
            continue
        result.append(
            {"left": left, "right": right, **paired(buckets[left], buckets[right])}
        )
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rows", type=Path)
    args = parser.parse_args()
    print(json.dumps(compare(json.loads(args.rows.read_text())), indent=2))
