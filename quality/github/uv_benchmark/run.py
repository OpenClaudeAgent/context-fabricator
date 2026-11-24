"""Run one isolated, headless benchmark against the packaged local API and CLI."""

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
from contextlib import ExitStack
from pathlib import Path
from urllib.error import HTTPError

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from dogfood import (
    FEATURES,
    FIXTURE_TOKEN,
    backend,
    command,
    prepare_answers,
    read_manifest,
    save,
    verify_evidence,
    wait_for_commit,
    wait_until,
)
from git_origin import origin
from public_reference import REFERENCE, inspect_git, inspect_git_tree


def run_identity(bundle, commit, gold_digest, profile):
    helpers = bundle / "Contents/MacOS"
    return {
        "commit": commit,
        "goldSha256": gold_digest,
        "binaries": {
            name: hashlib.sha256((helpers / name).read_bytes()).hexdigest()
            for name in ("ctxfab", "ctxfab-server")
        },
        "profile": profile,
    }


def request_answer(api, body):
    try:
        answer, trace = api("POST", "/v1/answers", body)
    except HTTPError as error:
        with error:
            problem = json.load(error)
        return (
            None,
            error.headers.get("X-Context-Trace-Id"),
            {
                "status": error.code,
                "problem": problem,
            },
        )
    return answer, trace, None


def run(args):
    from chunks import GOLD, audit
    from score import (
        audit_index,
        load_gold,
        score_answer,
        score_exact_search,
        score_search,
        summarize,
        vector_census,
    )

    if bool(args.reference) != bool(args.gold):
        raise ValueError("Supply the Git reference and its dataset together.")
    if args.reference and args.profile == "smoke":
        raise ValueError("The four smoke questions apply to the uv dataset.")
    dataset = args.gold or GOLD
    reference = json.loads((args.reference or REFERENCE).read_text())
    if args.reference:
        git = inspect_git_tree(args.repo, reference)
    else:
        git, _ = inspect_git(args.repo, reference)
    commit, name, rid = (
        reference["commit"],
        reference["repository"],
        reference["repositoryId"],
    )
    cases, gold_digest = load_gold(git, dataset)
    assets = args.assets
    full = args.profile == "full"
    smoke = args.profile == "smoke"
    vectors = args.profile == "vectors"
    if bool(assets) != (full or smoke or vectors):
        raise ValueError("Supply --assets for smoke, vectors and full profiles only.")
    if (args.vector_checkpoint or args.export_vector_checkpoint) and not (full or vectors):
        raise ValueError("Vector checkpoint operations require the vectors or full profile.")
    if args.vector_checkpoint and args.export_vector_checkpoint:
        raise ValueError("Choose either checkpoint restore or export for a run.")
    if args.resume_from and not args.vector_checkpoint:
        raise ValueError("Resuming requires the same verified uv vector checkpoint.")
    identity = run_identity(args.bundle, commit, gold_digest, args.profile)
    rows, traces = [], {}
    if args.resume_from:
        assert (
            json.loads((args.resume_from / "run-identity.json").read_text()) == identity
        )
        rows = json.loads((args.resume_from / "rows.json").read_text())
        traces = json.loads((args.resume_from / "answer-traces.json").read_text())
    retained = {(r["id"], r["language"], r["mode"]) for r in rows}
    assert len(retained) == len(rows)
    args.report.mkdir(parents=True, exist_ok=False)
    save(args.report, "run-identity.json", identity)
    if args.resume_from:
        save(args.report, "rows.json", rows)
        save(args.report, "answer-traces.json", traces)
    smoke_questions = {
        ("digest-endian", "en"),
        ("toolchains", "fr"),
        ("config-precedence", "en"),
        ("local-run", "fr"),
    }
    with (
        tempfile.TemporaryDirectory(prefix="ctxfab-rag-") as workspace,
        origin(git, repository=name, repository_id=rid) as (port, requests),
    ):
        data = Path(workspace) / "data"
        data.mkdir(mode=0o700)
        if assets:
            inference = data / "inference"
            inference.mkdir()
            filenames = ["Qwen3-4B-Instruct-2507-Q4_K_M.gguf"] if (full or smoke) else []
            for filename in filenames:
                # APFS creates an independent, copy-on-write local file.
                subprocess.run(
                    [
                        "/bin/cp",
                        "-c",
                        (assets / filename).resolve(strict=True),
                        inference / filename,
                    ],
                    check=True,
                )
        with ExitStack() as phase:
            api = phase.enter_context(
                backend(
                    args.bundle,
                    data,
                    port,
                    args.report,
                    hub_port=port,
                    preview_runtime=False,
                )
            )
            if assets and (full or smoke):
                prepare_answers(
                    api, args.report, embeddings=False, expect_download=False
                )
            api("PUT", "/v1/github/credential", {"revision": 0, "token": FIXTURE_TOKEN})
            started = time.monotonic()
            api(
                "PUT",
                "/v1/github/repositories",
                {
                    "revision": 1,
                    "repository": name,
                    "expectedId": rid,
                    "branch": "main",
                    "intervalMinutes": 1440,
                    "features": FEATURES,
                },
            )
            wait_for_commit(api, commit, timeout=1800)
            collection_seconds = time.monotonic() - started
            manifest = read_manifest(data, git, commit)
            assert (
                manifest["publishedPaths"] == reference["expectedPublication"]["paths"]
            )
            index = audit_index(data, git, commit)
            save(args.report, "chunks.json", audit(data, dataset))
            before = len(requests)

            def terminal_trace(identifier):
                try:
                    result, _ = api("GET", f"/v1/telemetry/traces/{identifier}")
                except HTTPError as error:
                    if error.code != 404:
                        raise
                    error.close()
                    return None
                return (
                    result
                    if any(span["name"] == "api_answer" for span in result["spans"])
                    else None
                )

            def reopen(hit):
                assert hit["kind"] == "current"
                return verify_evidence(
                    git, hit, commit, repository=name, repository_id=rid
                )

            def measure(modes):
                for case in cases:
                    for language, query in case["questions"].items():
                        if smoke and (case["id"], language) not in smoke_questions:
                            continue
                        for mode in modes:
                            if (case["id"], language, mode) in retained:
                                continue
                            started = time.monotonic()
                            hits = command(
                                args.bundle,
                                data,
                                "search",
                                query,
                                mode=mode,
                                repository_id=rid,
                            )
                            ms = round((time.monotonic() - started) * 1000, 2)
                            assert len(hits) <= 20
                            assert len(
                                {
                                    (h["source"], h["location"]["startByte"])
                                    for h in hits
                                }
                            ) == len(hits)
                            for hit in hits:
                                reopen(hit)
                            row = {
                                "id": case["id"],
                                "split": case["split"],
                                "language": language,
                                "mode": mode,
                                "searchMs": ms,
                                "ranking": [hit["location"] for hit in hits],
                                "retrieval": score_search(case, hits),
                                "byteRetrieval": score_exact_search(case, hits),
                            }
                            if smoke or (full and mode in ("bm25", "hybrid")):
                                started = time.monotonic()
                                answer, trace, problem = request_answer(
                                    api,
                                    {
                                        "query": query,
                                        "repositoryId": rid,
                                        "branch": "main",
                                        "includeModelContext": True,
                                    },
                                )
                                if problem:
                                    row["requestError"] = problem
                                    answer = {
                                        "mode": mode,
                                        "outcome": "requestError",
                                        "citations": [],
                                        "evidenceReview": {
                                            "reason": problem["problem"]["code"]
                                        },
                                    }
                                row.update(
                                    answerMs=round(
                                        (time.monotonic() - started) * 1000, 2
                                    ),
                                    trace=trace,
                                    answer=answer,
                                    assessment=score_answer(
                                        case, answer, reopen, git, mode
                                    ),
                                )
                                if trace:
                                    traces[trace] = {
                                        "id": case["id"],
                                        "language": language,
                                        "mode": mode,
                                        "trace": wait_until(
                                            10,
                                            lambda identifier=trace: terminal_trace(
                                                identifier
                                            ),
                                        ),
                                    }
                                    save(args.report, "answer-traces.json", traces)
                            rows.append(row)
                            temporary_rows = args.report / "rows.tmp"
                            temporary_rows.write_text(
                                json.dumps(rows, indent=2, ensure_ascii=False) + "\n"
                            )
                            temporary_rows.replace(args.report / "rows.json")

            measure(("bm25",))
            if full or vectors:
                if args.vector_checkpoint:
                    from checkpoint import restore_into_fixture

                    phase.close()
                    descriptor = json.loads(args.vector_checkpoint.read_text())
                    assert descriptor["sourceCommit"] == commit
                    assert descriptor["manifestSha256"] == reference["expectedPublication"]["manifestSha256"]
                    assert descriptor["profile"] == "qwen3-embedding-0.6b-q8_0-06507c7b-d1024-v1"
                    archive_path = args.vector_checkpoint.parent / descriptor["file"]
                    assert archive_path.stat().st_size == descriptor["bytes"]
                    restored = restore_into_fixture(
                        data,
                        archive_path,
                        descriptor["sha256"],
                        args.reference or REFERENCE,
                    )
                    assert restored == index["passages"] == descriptor["rows"]
                    index["vectorCheckpoint"] = {
                        "sha256": descriptor["sha256"],
                        "restored": restored,
                    }
                embed_name = "Qwen3-Embedding-0.6B-Q8_0.gguf"
                subprocess.run(
                    [
                        "/bin/cp",
                        "-c",
                        (assets / embed_name).resolve(strict=True),
                        data / "inference" / embed_name,
                    ],
                    check=True,
                )
                if args.vector_checkpoint:
                    dense_report = args.report / "dense"
                    dense_report.mkdir()
                    api = phase.enter_context(
                        backend(
                            args.bundle,
                            data,
                            port,
                            dense_report,
                            hub_port=port,
                            preview_runtime=False,
                        )
                    )
                started = time.monotonic()
                api(
                    "POST",
                    "/v1/model-downloads",
                    {"profile": "qwen3-embedding-0.6b-q8_0-06507c7b-d1024-v1"},
                )
                deadline = time.monotonic() + 14400
                while time.monotonic() < deadline:
                    progress, _ = api("GET", "/v1/retrieval")
                    save(
                        args.report,
                        "index-progress.json",
                        dict(
                            progress,
                            elapsedSeconds=round(time.monotonic() - started, 2),
                        ),
                    )
                    if (
                        progress["state"] == "ready"
                        and progress["indexed"]
                        == progress["total"]
                        == index["passages"]
                    ):
                        assert progress["profile"] == "qwen3-embedding-0.6b-q8_0-06507c7b-d1024-v1"
                        assert progress["dimensions"] == 1024
                        break
                    time.sleep(5)
                else:
                    raise TimeoutError(f"Incomplete dense index: {progress}")
                save(
                    args.report,
                    "vector-census.json",
                    vector_census(data, index["passages"]),
                )
                if args.export_vector_checkpoint:
                    from checkpoint import export

                    descriptor_path = args.export_vector_checkpoint
                    if descriptor_path.exists():
                        raise FileExistsError(descriptor_path)
                    archive = descriptor_path.with_suffix(".sqlite3")
                    descriptor = export(data, archive, args.reference or REFERENCE)
                    with descriptor_path.open("x") as output:
                        json.dump(descriptor, output, indent=2)
                        output.write("\n")
                    from check_restore import roundtrip

                    save(args.report, "vector-restore-check.json", roundtrip(
                        data, descriptor_path.parent, args.reference or REFERENCE,
                        descriptor_path,
                    ))
                index["vectors"] = progress
                index["secondsFromSelectionToReadyCheck"] = time.monotonic() - started
                if full:
                    measure(("semantic", "hybrid"))
            after = api("GET", "/v1/status")[0]["outboundAttempts"]
            assert len(requests) == before == after
            save(
                args.report,
                "summary.json",
                {
                    **identity,
                    "resumedFrom": args.resume_from.name if args.resume_from else None,
                    "retainedRows": len(retained),
                    "collectionSeconds": collection_seconds,
                    "index": index,
                    "publication": manifest,
                    "originAttemptsBefore": before,
                    "originAttemptsAfter": after,
                    "results": summarize(rows),
                },
            )


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Run without -O to enable integrity checks.")
    os.environ.update(GIT_NO_LAZY_FETCH="1", GIT_NO_REPLACE_OBJECTS="1")
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("repo", "bundle", "report"):
        parser.add_argument(key, type=Path)
    parser.add_argument("--reference", type=Path, help="Pinned Git reference JSON.")
    parser.add_argument(
        "--gold", type=Path, help="Bilingual dataset for that reference."
    )
    parser.add_argument("--profile", choices=("bm25", "smoke", "vectors", "full"), default="bm25")
    parser.add_argument(
        "--vector-checkpoint",
        type=Path,
        help="Verify and restore a pinned public-vectors.json checkpoint.",
    )
    parser.add_argument("--export-vector-checkpoint", type=Path,
                        help="Export the completed public vector index and descriptor to this JSON path.")
    parser.add_argument(
        "--resume-from",
        type=Path,
        help="Retain completed rows from an interrupted run with the same identity.",
    )
    parser.add_argument(
        "--assets",
        type=Path,
        help="Directory containing the pinned GGUF weights for the chosen model profile.",
    )
    run(parser.parse_args())
