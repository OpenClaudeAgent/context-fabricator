"""Sample macOS resident memory of the benchmark's helper and inference processes."""

import argparse
import json
import subprocess
import time
from pathlib import Path


def sample(root):
    out = subprocess.check_output(
        ["/bin/ps", "-axo", "pid=,ppid=,rss=,comm="], text=True
    )
    processes = {}
    for line in out.splitlines():
        fields = line.split(maxsplit=3)
        if len(fields) == 4:
            pid, ppid, rss, comm = fields
            processes[int(pid)] = (int(ppid), int(rss), Path(comm).name)
    if root not in processes:
        return None
    parents = {root}
    while True:
        new = {pid for pid, (ppid, _, _) in processes.items() if ppid in parents}
        if not new - parents:
            break
        parents.update(new)
    return {
        kind: sum(
            rss
            for pid, (_, rss, name) in processes.items()
            if pid in parents and name == kind
        )
        for kind in ("ctxfab-server", "llama-server")
    }


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("benchmark_pid", type=int)
    args = p.parse_args()
    maxima = {}
    started = time.monotonic()
    n = 0
    worker_samples = 0
    while (row := sample(args.benchmark_pid)) is not None:
        for kind, value in row.items():
            maxima[kind] = max(value, maxima.get(kind, 0))
        n += 1
        worker_samples += row["llama-server"] > 0
        time.sleep(1)
    print(
        json.dumps(
            {
                "samplingSeconds": round(time.monotonic() - started, 2),
                "intervalSeconds": 1,
                "samples": n,
                "samplesWithInferenceWorker": worker_samples,
                "maximumObservedAggregatedRssKiBByExecutable": maxima,
            },
            indent=2,
        )
    )
