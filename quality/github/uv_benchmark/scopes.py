"""Check additional frozen provenance cases without constructing a synthetic answer."""

import argparse
import hashlib
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from public_reference import REFERENCE, inspect_git


def verify(repo, evidence):
    ref = json.loads(REFERENCE.read_text())
    git, _ = inspect_git(repo, ref)
    tip, parent = ref["commit"], ref["parent"]

    def lines(rev, path, first, last):
        blob = git.files[rev][path]["sha"]
        excerpt = "\n".join(git.blob(blob).decode().splitlines()[first - 1 : last])
        return {
            "revision": rev,
            "path": path,
            "blob": blob,
            "startLine": first,
            "endLine": last,
            "text": excerpt,
        }

    path = "crates/uv-fs/src/which.rs"
    old = lines(parent, path, 19, 19)
    new = lines(tip, path, 19, 20)
    assert "SAFETY: winapi call" in old["text"]
    assert all(
        s in new["text"]
        for s in ("null-terminated UTF-16", "writable", "duration of the call")
    )
    path = "crates/uv-python-managed/download-metadata.json"
    excluded = lines(tip, path, 2, 15)
    sha = "fab6c060de94630e7e0b6e5f61c27da0c41236ed5fb94a31f34b62c3cfce8796"
    assert sha in excluded["text"] and git.files[tip][path]["size"] > 256 * 1024
    # A retained duplicate of the literal would invalidate this absence control.
    retained, skipped = git.coverage(tip)
    assert skipped[path] == "oversized"
    occurrences = git.git("grep", "-l", "-F", sha, tip, "--").decode().splitlines()
    assert occurrences and not {s.split(":", 1)[1] for s in occurrences}.intersection(
        retained
    )
    recorded = json.loads(evidence.read_text())
    assert recorded["run"] == ref["run"]
    assert recorded["url"].endswith("/44806099739/attempts/1")
    return {
        "scope": "independent source and provider oracles; separate from ranking measurements",
        "history": {
            "question": "What safety guarantees replaced the short WinAPI comment in get_binary_type?",
            "expected": "The name is null-terminated UTF-16 and binary_type is a writable u32. Both remain valid during the call.",
            "before": old,
            "after": new,
        },
        "actions": {
            "question": "What conclusion did the selected CI attempt report?",
            "expected": "success",
            "field": "run.conclusion",
            "record": recorded["run"],
            "url": recorded["url"],
            "recordSha256": hashlib.sha256(evidence.read_bytes()).hexdigest(),
        },
        "coverage": {
            "question": "Give the SHA-256 for the managed cpython-3.16.0a2-darwin-aarch64-none download.",
            "expected": "insufficientEvidence",
            "excludedSource": excluded,
            "reason": skipped[path],
            "trackedLiteralPaths": occurrences,
            "publishedLiteralPaths": [],
        },
    }


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Assertions must be enabled.")
    os.environ.update(GIT_NO_LAZY_FETCH="1", GIT_NO_REPLACE_OBJECTS="1")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repo", type=Path)
    parser.add_argument("captured_attempt", type=Path)
    args = parser.parse_args()
    print(
        json.dumps(
            verify(args.repo, args.captured_attempt), indent=2, ensure_ascii=False
        )
    )
