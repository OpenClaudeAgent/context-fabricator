"""Offline diagnostic. Supply source-derived evidence directly to the pinned Qwen worker."""

import argparse
import hashlib
import json
import os
import random
import secrets
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import tomllib
from contextlib import contextmanager
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, Request, build_opener

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(HERE.parent))
from public_reference import REFERENCE, inspect_git_tree
from review import sheet
from score import load_gold

ENGINE = "b2c106037defa2b63bb4067d964c32a14f0a9498f2f6a1ee451f0e4d1e2b357e"
WEIGHTS = "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597"
ALIAS = "qwen3-4b-instruct-2507-q4-k-m-3605803b-v1"
CONTROL = {"local-run": "id-entropy", "local-config": "config-precedence"}


def cache_directory(*flags):
    if os.environ.get("CI"):
        return ROOT / ".quality"
    locator = ROOT / "quality/toolchain/cache-directory.py"
    return Path(subprocess.check_output([sys.executable, "-B", locator, *flags], text=True).strip())


def digest(path):
    with path.open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def save(path, data):
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n")
    tmp.replace(path)


def renderer(tmp):
    manifest = tmp / "Cargo.toml"
    deps = {
        p["name"]: p["version"]
        for p in tomllib.loads((ROOT / "backend/Cargo.lock").read_text())["package"]
    }
    manifest.write_text(
        '[package]\nname="oracle-render"\nversion="0.1.0"\nedition="2024"\n[[bin]]\n'
        + f'name="oracle-render"\npath={json.dumps(str(HERE / "oracle_policy.rs"))}\n'
        + f'[dependencies]\nserde={{version="={deps["serde"]}",features=["derive"]}}\nserde_json="={deps["serde_json"]}"\n'
    )
    tools = cache_directory() / "tools"
    subprocess.run(
        [
            tools / "cargo/bin/cargo",
            "build",
            "--quiet",
            "--offline",
            "-j",
            "1",
            "--manifest-path",
            manifest,
        ],
        check=True,
        cwd=ROOT,
        env=dict(
            os.environ,
            CARGO_HOME=str(tools / "cargo"),
            RUSTUP_HOME=str(tools / "rustup"),
        ),
    )
    return tmp / "target/debug/oracle-render"


def evidence(git, case):
    by_path = {}
    for fact in case["facts"]:
        for anchor in fact["anchors"]:
            by_path.setdefault(anchor["path"], []).append(
                (anchor["startLine"], anchor["endLine"])
            )
    parts = []
    for path, intervals in sorted(by_path.items()):
        oid = git.files[git.tip][path]["sha"]
        lines = git.blob(oid).splitlines(keepends=True)
        merged = []
        for a, b in sorted(intervals):
            a, b = max(1, a - 2), min(len(lines), b + 2)
            if merged and a <= merged[-1][1] + 1:
                merged[-1][1] = max(b, merged[-1][1])
            else:
                merged.append([a, b])
        for a, b in merged:
            parts.append(
                {
                    "id": len(parts) + 1,
                    "kind": "current",
                    "text": b"".join(lines[a - 1 : b]).decode(),
                    "source": {
                        "path": path,
                        "revision": git.tip,
                        "startLine": a,
                        "endLine": b,
                    },
                }
            )
    return parts


def indexed_evidence(git, case, checkpoint):
    groups = {}
    for fact in case["facts"]:
        for anchor in fact["anchors"]:
            groups.setdefault(anchor["path"], []).append(anchor)
    parts = []
    for path, anchors in sorted(groups.items()):
        oid = git.files[git.tip][path]["sha"]
        raw = git.blob(oid)
        lines = raw.splitlines(keepends=True)
        offsets = [0]
        for line in lines:
            offsets.append(offsets[-1] + len(line))
        spans = [(offsets[a["startLine"] - 1], offsets[a["endLine"]]) for a in anchors]
        selected = []
        for blob, start, end in checkpoint.execute(
            "SELECT blob,start_byte,end_byte FROM vectors WHERE path=? ORDER BY start_byte",
            (path,),
        ):
            assert blob == oid
            if any(start < b and a < end for a, b in spans):
                selected.append((start, end))
                body = raw[start:end]
                source = {
                    "path": path,
                    "revision": git.tip,
                    "startLine": 1 + raw.count(b"\n", 0, start),
                    "endLine": raw.count(b"\n", 0, end) + int(not body.endswith(b"\n")),
                }
                parts.append(
                    {
                        "id": len(parts) + 1,
                        "kind": "current",
                        "text": body.decode(),
                        "source": source,
                    }
                )
        for a, b in spans:
            assert (
                sum(max(0, min(end, b) - max(start, a)) for start, end in selected)
                == b - a
            )
    assert 1 <= len(parts) <= 5
    return parts


@contextmanager
def worker(binary, model, tmp, out):
    with socket.socket() as reserve:
        reserve.bind(("127.0.0.1", 0))
        port = reserve.getsockname()[1]
    key = secrets.token_hex(32)
    keyfile = tmp / "key"
    keyfile.write_text(key)
    keyfile.chmod(0o600)
    args = [
        str(binary),
        "--offline",
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
        "--api-key-file",
        str(keyfile),
        "--model",
        str(model),
        "--alias",
        ALIAS,
        "--no-ui",
        "--log-disable",
        "--gpu-layers",
        "all",
        "--ctx-size",
        "4096",
        "--parallel",
        "1",
        "--threads",
        "4",
        "--no-context-shift",
    ]
    opener = build_opener(ProxyHandler({}))

    def call(path, body=None):
        headers = {"Authorization": "Bearer " + key, "Content-Type": "application/json"}
        req = Request(
            f"http://127.0.0.1:{port}" + path,
            data=None if body is None else json.dumps(body).encode(),
            headers=headers,
        )
        with opener.open(req, timeout=55) as response:
            return json.load(response)

    with (out / "worker.log").open("wb") as log:
        process = subprocess.Popen(args, env={}, stdout=log, stderr=subprocess.STDOUT)
        try:
            for _ in range(100):
                if process.poll() is not None:
                    raise RuntimeError(f"worker exited {process.returncode}")
                try:
                    if call("/health").get("status") == "ok":
                        break
                except (URLError, TimeoutError):
                    pass
                time.sleep(0.4)
            else:
                raise RuntimeError("worker never ready")
            yield call
        finally:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def assess(response, parts):
    choice = response["choices"][0]
    if response["model"] != ALIAS or choice["finish_reason"] != "stop":
        raise ValueError("Wrong model or incomplete generation")
    draft = json.loads(choice["message"]["content"])
    answer = draft["answer"]
    refs = draft["citations"]
    if not answer and not refs:
        return {"status": "abstained", "text": "", "quotes": []}
    if not isinstance(answer, str) or not answer.strip() or not 1 <= len(refs) <= 5:
        raise ValueError("Invalid answer/citation shape")
    quotes = []
    for ref in refs:
        idx, quote = ref["passage"], ref["quote"].strip()
        if type(idx) is not int or not 1 <= idx <= len(parts):
            raise ValueError("Invalid passage ID")
        part = parts[idx - 1]
        loc = part["source"]
        start = part["text"].find(quote)
        literal = (
            len(quote.encode()) >= 8 and start >= 0 and part["text"].count(quote) == 1
        )
        url = f"https://github.com/astral-sh/uv/blob/{loc['revision']}/{loc['path']}"
        if literal:
            line = loc["startLine"] + part["text"][:start].count("\n")
            url += f"#L{line}-L{line + quote.count(chr(10))}"
        quotes.append(
            {"quote": quote, "passage": idx, "uniqueLiteral": literal, "url": url}
        )
    return {"status": "draft", "text": answer, "quotes": quotes}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("repo", type=Path)
    ap.add_argument("out", type=Path)
    ap.add_argument(
        "--prepare-only",
        action="store_true",
        help="Reconstruct the requests without launching a model",
    )
    ap.add_argument(
        "--checkpoint",
        type=Path,
        help="Use original indexed passages intersecting the evidence lines",
    )
    ap.add_argument(
        "--bundle",
        type=Path,
        default=cache_directory("--build") / "foundation-xcode/Build/Products/Debug/Context Fabricator.app",
    )
    ap.add_argument(
        "--model",
        type=Path,
        default=cache_directory() / "m09-assets/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
    )
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    if any(args.out.iterdir()):
        raise ValueError("Use an empty output directory")
    engine = (
        args.bundle
        / "Contents/Helpers/Context Fabricator Runtime.app/Contents/MacOS/llama-server"
    )
    if digest(engine) != ENGINE or digest(args.model) != WEIGHTS:
        raise ValueError("Engine or model fingerprint mismatch")
    git = inspect_git_tree(args.repo, json.loads(REFERENCE.read_text()))
    cases, gold_hash = load_gold(git)
    cases_by_id = {c["id"]: c for c in cases}
    with tempfile.TemporaryDirectory() as directory:
        tmp = Path(directory)
        render = renderer(tmp)
        checkpoint = None
        if args.checkpoint:
            assert (
                digest(args.checkpoint)
                == "190af8250499c2f09f50dddccf01f50a0a51b0f59bf6ca5bf56aaaeddd7c08ac"
            )
            checkpoint = sqlite3.connect(
                args.checkpoint.resolve().as_uri() + "?mode=ro&immutable=1", uri=True
            )
            assert checkpoint.execute("SELECT commit_id FROM metadata").fetchone() == (
                git.tip,
            )
        rows = []
        for c in cases:
            evidence_case = cases_by_id[CONTROL.get(c["id"], c["id"])]
            parts = (
                indexed_evidence(git, evidence_case, checkpoint)
                if checkpoint
                else evidence(git, evidence_case)
            )
            for language, q in c["questions"].items():
                body = {
                    "question": q,
                    "answerLanguage": {"en": "English", "fr": "French"}[language],
                    "passages": parts,
                }
                payload = json.loads(
                    subprocess.check_output([render], input=json.dumps(body).encode())
                )
                rows.append(
                    {
                        "id": c["id"],
                        "language": language,
                        "split": c["split"],
                        "payload": payload,
                    }
                )
        if checkpoint:
            checkpoint.close()
        random.Random(8741).shuffle(rows)
        save(args.out / "requests.json", rows)
        save(
            args.out / "identity.json",
            {
                "engine": ENGINE,
                "weights": WEIGHTS,
                "alias": ALIAS,
                "commit": git.tip,
                "gold": gold_hash,
                "requests": digest(args.out / "requests.json"),
                "policy": digest(ROOT / "backend/inference/src/generation/policy.rs"),
                "arm": "indexed-passages" if args.checkpoint else "two-neighbor-lines",
                "renderer": digest(HERE / "oracle_policy.rs"),
                "harness": digest(Path(__file__)),
                "seed": 8741,
            },
        )
        if args.prepare_only:
            return
        results = []
        with worker(engine, args.model, tmp, args.out) as call:
            for row in rows:
                start = time.monotonic()
                r = {k: v for k, v in row.items() if k != "payload"}
                try:
                    payload = row["payload"]
                    r["inputTokens"] = call(
                        "/v1/chat/completions/input_tokens", payload
                    )["input_tokens"]
                    if r["inputTokens"] > 4096 - 641:
                        raise ValueError("Exact prompt exceeds production token limit")
                    r["raw"] = call("/v1/chat/completions", payload)
                    parts = json.loads(payload["messages"][1]["content"])["passages"]
                    r["assessment"] = assess(r["raw"], parts)
                except (
                    TimeoutError,
                    OSError,
                    HTTPError,
                    ValueError,
                    IndexError,
                    KeyError,
                ) as e:
                    r["error"] = str(e)
                r["wallSeconds"] = time.monotonic() - start
                results.append(r)
                save(args.out / "rows.json", results)
        for_review = []
        for r in results:
            grade = r.get("assessment", {})
            for_review.append(
                {
                    "id": r["id"],
                    "language": r["language"],
                    "answer": {
                        "outcome": "answered"
                        if grade.get("status") == "draft"
                        else "insufficientEvidence"
                        if grade.get("status") == "abstained"
                        else "error",
                        "text": grade.get("text"),
                        "citations": [
                            {
                                "quote": q["quote"],
                                "evidence": {"location": {"fileUrl": q["url"]}},
                            }
                            for q in grade.get("quotes", [])
                        ],
                    },
                }
            )
        save(args.out / "review.json", sheet(for_review, cases))


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Enable assertions")
    os.environ.update(GIT_NO_LAZY_FETCH="1", GIT_NO_REPLACE_OBJECTS="1")
    main()
