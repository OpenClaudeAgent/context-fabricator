"""Save backend trace evidence for the answer rows in a running fixture campaign."""

import argparse
import json
import sys
import time
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from dogfood import BackendSession


def collect(data, report):
    endpoint = json.loads((data / "endpoint.json").read_text())
    assert urlsplit(endpoint["url"]).hostname == "127.0.0.1"
    api = BackendSession(endpoint, 0)
    saved = {}
    path = report / "rows.json"
    stamp = None
    while data.is_dir():
        if path.is_file() and path.stat().st_mtime_ns != stamp:
            current = path.stat().st_mtime_ns
            rows = json.loads(path.read_text())
            for r in rows:
                if "answer" not in r or r["trace"] in saved:
                    continue
                try:
                    trace, _ = api("GET", "/v1/telemetry/traces/" + r["trace"])
                except HTTPError as e:
                    if e.code != 404:
                        raise
                    e.close()
                    break
                except URLError:
                    return
                if not any(span["name"] == "api_answer" for span in trace["spans"]):
                    break
                saved[r["trace"]] = {
                    "id": r["id"],
                    "language": r["language"],
                    "mode": r["mode"],
                    "trace": trace,
                }
                target = report / "answer-traces.json"
                tmp = target.with_suffix(".new")
                tmp.write_text(json.dumps(saved, indent=2, ensure_ascii=False) + "\n")
                tmp.replace(target)
            else:
                stamp = current
                if len(saved) == 44:
                    return
        time.sleep(0.15 if len(saved) != 22 else 2)


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("data", type=Path)
    p.add_argument("report", type=Path)
    args = p.parse_args()
    collect(args.data, args.report)
