"""Retain progress checkpoints and export a sanitized complete vector cache."""

import argparse
import json
import time
from pathlib import Path

from checkpoint import export


def observe(data, report):
    started = time.monotonic()
    samples = []
    while time.monotonic() - started < 15000:
        if not data.is_dir():
            raise RuntimeError("Benchmark profile exited before vector export")
        try:
            record = json.loads((report / "index-progress.json").read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            time.sleep(5)
            continue
        if not samples or record["elapsedSeconds"] != samples[-1]["elapsedSeconds"]:
            samples.append(record)
            tmp = report / "index-samples.new"
            tmp.write_text(json.dumps(samples, indent=2) + "\n")
            tmp.replace(report / "index-samples.json")
        if record["state"] == "ready" and record["indexed"] == record["total"] == 11188:
            cache = report / "public-vectors.sqlite3"
            descriptor = export(data, cache)
            (report / "public-vectors.json").write_text(
                json.dumps(descriptor, indent=2) + "\n"
            )
            return
        time.sleep(30)
    raise TimeoutError("Benchmark exceeded four-hour index observation window")


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Assertions must be enabled")
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("data", type=Path)
    p.add_argument("report", type=Path)
    args = p.parse_args()
    observe(args.data, args.report)
