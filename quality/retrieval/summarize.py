"""Report the pinned multilingual judgments from the packaged HTTP retrieval measurements."""

import json
import math
from pathlib import Path
from statistics import mean, median
import sys


def summary(cases):
    ranks = [case["rank"] for case in cases]
    latencies = sorted(case["micros"] / 1000 for case in cases)
    return {
        "queries": len(cases),
        **{f"recallAt{k}": mean(rank is not None and rank <= k for rank in ranks) for k in (1, 3, 5)},
        "mrr": mean(0 if rank is None else 1 / rank for rank in ranks),
        "medianMillis": round(median(latencies), 3),
        "p95Millis": round(latencies[math.ceil(0.95 * len(latencies)) - 1], 3),
    }


report = json.loads(Path(sys.argv[1]).read_text())
rss = [int(line) for line in Path(sys.argv[2]).read_text().splitlines() if line.strip()]
cases = report["cases"]
families = ["all", *sorted({case["family"] for case in cases})]
measurements = {
    family: {
        mode: summary(
            [case for case in cases if case["mode"] == mode and (family == "all" or case["family"] == family)]
        )
        for mode in ["bm25", "semantic", "hybrid"]
    }
    for family in families
}
print(json.dumps({
    "documents": report["documents"],
    "profile": report["profile"],
    "rebuildMillisIncludingRestart": report["rebuildMicrosIncludingRestart"] / 1000,
    "vectorBytes": report["vectorBytes"],
    "workerPeakSampledRssKiB": max(rss),
    "measurements": measurements,
}, indent=2))
