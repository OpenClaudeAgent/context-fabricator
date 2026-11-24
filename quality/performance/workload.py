"""Run supervised local inference, GitHub sync and embedding on the same packaged helper."""

import hashlib
import json
import math
import os
import re
import shutil
import sqlite3
import subprocess
import sys
import time
from collections import deque
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing, contextmanager
from itertools import accumulate, pairwise
from pathlib import Path
from tempfile import TemporaryDirectory
from threading import Event, Lock

quality = Path(__file__).resolve().parents[1]
sys.path[:0] = [str(quality / name) for name in ("github", "release")]
from app_distribution import RUNTIME_MAIN
from dogfood import (
    BASE,
    FEATURES,
    GENERATION_PROFILE,
    REDIRECT_SOURCE,
    TIP,
    backend,
    command,
    prepare_answers,
    read_manifest,
    save,
    verify_evidence,
    verify_quote_link,
    verify_unscoped_french_cooldown,
    wait_for_commit,
    wait_until,
)
from generation_recovery import run_generation_recovery, verify_runtime_workers
from git_origin import FIXTURE_TOKEN, REPOSITORY, REPOSITORY_ID, GitHistory, origin
from overlap_measurement import certify_overlap, stamp
from process_sampling import ProcessSampler
from release import read_package, version_key
from runtime_package import digest

INDEX = "/v1/retrieval/index"
REFRESH = f"/v1/github/repositories/{REPOSITORY_ID}/refresh"
QUESTION = "In the GitHub REST request adapter, what maximum does MAX_REDIRECTS set?"
ANCHOR = "const MAX_REDIRECTS: usize = 3;"
WEIGHTS = ("Qwen3-4B-Instruct-2507-Q4_K_M.gguf", "Qwen3-Embedding-0.6B-Q8_0.gguf")
HOLD_LIMIT_SECONDS = 13
TIP_TREE_DEADLINE_SECONDS = 20


class OriginTimeline:
    def __init__(self, tip_tree, controlled):
        self.tip_tree = "/git/trees/" + tip_tree
        self.controlled = controlled
        self.lock = Lock()
        self.tip_tree_entered = Event()
        self.release = Event()
        self.records = []
        self.tip_tree_request = None
        self.armed = False

    @contextmanager
    def observe(self, path):
        event = {"path": path, "started": stamp()}
        with self.lock:
            first_change = self.armed and self.tip_tree_request is None and path.endswith(self.tip_tree)
            if first_change:
                self.tip_tree_request = event
        if first_change:
            self.tip_tree_entered.set()
            if self.controlled:
                event["releasedByHarness"] = self.release.wait(timeout=HOLD_LIMIT_SECONDS)
        try:
            yield
        finally:
            event["finished"] = stamp()
            with self.lock:
                self.records.append(event)

    def since(self, moment):
        with self.lock:
            return [event for event in self.records if event["started"]["mono_us"] >= moment]


def answer_verified(git, answer, revision):
    assert answer["outcome"] == "answered" and answer["model"] == GENERATION_PROFILE, answer
    assert answer["evidenceRepositoryIds"] == [REPOSITORY_ID], answer
    assert re.search(r"\b3\b", answer["text"]), answer
    quotes = []
    for citation in answer["citations"]:
        excerpt = verify_evidence(git, citation["evidence"], revision)
        verify_quote_link(citation, excerpt)
        if citation["evidence"]["location"]["path"] == REDIRECT_SOURCE:
            quotes.append(citation["quote"])
    assert any(ANCHOR in quote for quote in quotes), answer
    return True


def progress_sample(api, polling):
    start = stamp()
    progress, _ = api("GET", "/v1/retrieval")
    polling.append({"started": start, "finished": stamp(), "indexed": progress["indexed"],
                    "total": progress["total"], "state": progress["state"]})
    return progress


def sample_while(api, operation, polling, poll_seconds=0.09, status_times=None):
    while not operation.done():
        progress_sample(api, polling)
        if status_times is not None:
            status_times.append(status_sample(api))
        time.sleep(poll_seconds)
    return operation.result()


def timed_cli(bundle, data, git, revision):
    start = stamp()
    answer = command(bundle, data, "ask", QUESTION)
    end = stamp()
    answer_verified(git, answer, revision)
    return {"started": start, "finished": end, "wallMilliseconds": (end["mono_us"]-start["mono_us"])/1000,
            "answer": answer}


def require_backpressure(bundle, data):
    started = time.monotonic()
    try:
        command(bundle, data, "ask", QUESTION)
    except subprocess.CalledProcessError as error:
        assert error.returncode == 1 and b"generation worker is busy" in error.stderr, error.stderr
        return {"rejectedBusy": True, "wallMilliseconds": round((time.monotonic()-started)*1000, 2)}
    raise AssertionError("The overlapping answer was accepted instead of reporting busy")


def answer_span(api, call):
    def finished_span():
        telemetry, _ = api("GET", "/v1/telemetry")
        trace_ids = {span["traceId"] for span in telemetry["traces"]
                     if span["name"] == "answer_generation" and span.get("level") == "ok"}
        spans = []
        for trace_id in trace_ids:
            detail, _ = api("GET", f"/v1/telemetry/traces/{trace_id}")
            for span in detail["spans"]:
                start = span.get("startedAtUnixMicros", 0)
                duration = span.get("value", 0)
                if (span["name"] == "answer_generation" and span.get("level") == "ok"
                        and call["started"]["unix_us"] <= start
                        and start + duration <= call["finished"]["unix_us"]):
                    spans.append(span)
        assert len(spans) <= 1, "More than one generation span matched the measured CLI window"
        return spans[0] if spans else None
    return wait_until(8, finished_span)


def status_sample(api):
    since = time.monotonic_ns()
    state, _ = api("GET", "/v1/status")
    assert state["status"] == "ready", state
    return (time.monotonic_ns() - since) / 1_000_000


def rate_window(records):
    arrivals = sorted(record["started"]["mono_us"] for record in records)
    window = deque()
    longest = 0
    for arrival in arrivals:
        while window and arrival - window[0] >= 60_000_000:
            window.popleft()
        window.append(arrival)
        longest = max(longest, len(window))
    boundaries = sorted((record[edge]["mono_us"], delta) for record in records
                        for edge, delta in (("started", 1), ("finished", -1)))
    peak_in_flight = max(accumulate(delta for _, delta in boundaries))
    return {"peakArrivalsPerRollingMinute": longest, "peakConcurrentOriginRequests": peak_in_flight,
            "minArrivalSpacingMs": min((b-a)/1000 for a, b in pairwise(arrivals))}


def storage_sizes(data):
    return {p.name: p.stat().st_size for p in data.glob("*.sqlite3*") if p.is_file()}


def first_phase(api, data, git, bundle):
    api("PUT", "/v1/github/credential", {"revision": 0, "token": FIXTURE_TOKEN})
    api("PUT", "/v1/github/repositories", {"revision": 1, "repository": REPOSITORY,
        "expectedId": REPOSITORY_ID, "branch": "main", "intervalMinutes": 60, "features": FEATURES})
    wait_for_commit(api, BASE)
    base_manifest = read_manifest(data, git, BASE)
    cold = timed_cli(bundle, data, git, BASE)
    assert cold["answer"]["mode"] == "bm25"
    state, _ = api("GET", "/v1/github")
    updated, _ = api("PUT", "/v1/github/rate-limit", {
        "revision": state["revision"], "requestsPerMinute": 60})
    traffic = updated["traffic"]
    assert traffic["effectiveRequestsPerMinute"] == 60 and traffic["concurrency"] == 1, traffic
    index, _ = api("PUT", INDEX, {"enabled": True})
    return base_manifest, cold, index["epoch"], traffic["attempts"]


def overlap_answer(api, bundle, data, git, timeline, pool, measure, epoch, report):
    assert timeline.tip_tree_entered.wait(timeout=TIP_TREE_DEADLINE_SECONDS), (
        "The paced refresh did not request the changed TIP tree")
    polling = []
    status_times = []
    try:
        single = pool.submit(timed_cli, bundle, data, git, BASE)
        time.sleep(0.3)
        assert not single.done(), "First CLI answer ended before concurrent admission test"
        pressure = pool.submit(require_backpressure, bundle, data)
        warm = sample_while(api, single, polling, status_times=status_times)
        rejected = pressure.result(timeout=5)
        after, _ = api("GET", INDEX)
    finally:
        timeline.release.set()
    event = wait_until(3, lambda: timeline.tip_tree_request if "finished" in timeline.tip_tree_request else None)
    observed_status = {"samples": len(status_times), "p95Milliseconds": percentile95(status_times)}
    evidence = {"tipTreeRequest": event, "progressPolls": polling, "cliAnswer": warm,
                "concurrentRequest": rejected, "statusLatency": observed_status,
                "refreshStartedMonotonicUs": measure}
    save(report, "overlap-evidence.json", evidence)
    trace = answer_span(api, warm)
    evidence["successfulGenerationSpan"] = trace
    if timeline.controlled:
        evidence["proof"] = certify_overlap(event, trace, polling, epoch, after["epoch"])
    else:
        assert after["epoch"] == epoch and polling[-1]["indexed"] > polling[0]["indexed"]
    save(report, "overlap-evidence.json", evidence)
    if timeline.controlled:
        assert evidence["proof"]["certified"], evidence["proof"]
    return warm


def percentile95(values):
    return sorted(values)[math.ceil(len(values)*.95)-1]


def index_complete(api):
    result, _ = api("GET", "/v1/retrieval")
    return result if result["state"] == "ready" and result["total"] > 0 and result["indexed"] == result["total"] else None


def exercise_fault(api, runtime, git, timeline, pool, refresh, report):
    assert not refresh.done(), "TIP published before the recovery exercise could begin"
    before_crash = stamp()
    crash_progress = []
    progress_sample(api, crash_progress)
    crash = pool.submit(run_generation_recovery, api, api.process_id, runtime, QUESTION,
        lambda answer: answer_verified(git, answer, BASE), report,
        request_scope={"repositoryId": REPOSITORY_ID, "branch": "main"})
    failure = sample_while(api, crash, crash_progress, poll_seconds=0.15)
    progress_sample(api, crash_progress)
    after_crash = stamp()
    assert crash_progress[-1]["indexed"] > crash_progress[0]["indexed"], "Index made no progress through worker recovery"
    ongoing = [item for item in timeline.since(before_crash["mono_us"])
               if item["finished"]["mono_us"] <= after_crash["mono_us"]]
    assert ongoing, "No provider request completed during generation recovery"
    save(report, "recovery-progress.json", {"started": before_crash, "finished": after_crash,
         "collectionRequestsCompleted": len(ongoing), "progressPolls": crash_progress,
         "recoveringWorker": failure["replacementGeneration"]})


def persisted_vectors(data, profile):
    database = (data / "context.sqlite3").as_uri() + "?mode=ro"
    checksum = hashlib.sha256()
    count = 0
    with closing(sqlite3.connect(database, uri=True)) as connection:
        rows = connection.execute(
            "SELECT v.document_id, d.source, d.revision, d.tracked_ref, d.passage_start, v.embedding "
            "FROM document_vectors v JOIN documents d ON d.id = v.document_id "
            "WHERE v.profile = ? ORDER BY v.document_id", (profile,),
        )
        for *identity, embedding in rows:
            checksum.update(json.dumps(identity).encode())
            checksum.update(embedding)
            count += 1
    return {"count": count, "sha256": checksum.hexdigest()}


def corpus_schema_version(data):
    database = (data / "context.sqlite3").as_uri() + "?mode=ro"
    with closing(sqlite3.connect(database, uri=True)) as connection:
        return connection.execute("PRAGMA user_version").fetchone()[0]


def verify_release_readback(release, data, git, report, index, vectors, attempts, *,
                            previous=False, retained_settings=None):
    bundle, package = release
    report.mkdir()
    assert vectors["count"] == index["indexed"]
    assert persisted_vectors(data, index["profile"]) == vectors
    schema_before = corpus_schema_version(data)
    with backend(bundle, data, None, report, preview_runtime=False) as api:
        assert api("GET", "/v1/github")[0]["credential"] == "unconfigured"
        assert api("GET", "/v1/runtime")[0]["state"] == "ready"
        assert persisted_vectors(data, index["profile"]) == vectors
        assert wait_until(30, lambda: index_complete(api)) == index
        assert api("GET", "/v1/status")[0]["outboundAttempts"] == attempts
        settings, _ = api("GET", "/v1/settings")
        if previous:
            change, _ = api("PUT", "/v1/settings", {
                "revision": settings["revision"],
                "answerLanguage": settings["answerLanguage"],
                "diagnosticRetentionHours": 48,
            })
            assert change == settings | {
                "revision": settings["revision"] + 1, "diagnosticRetentionHours": 48,
            }
            settings, _ = api("GET", "/v1/settings")
            assert settings == change
        if retained_settings is not None:
            assert settings == retained_settings
        answer = timed_cli(bundle, data, git, TIP)
        assert answer["answer"]["mode"] == "hybrid"
        if not previous:
            verify_unscoped_french_cooldown(api, git, report)
        workers = verify_runtime_workers(api.process_id, (bundle / RUNTIME_MAIN).resolve(strict=True),
                                         embedding=True)
        assert api("GET", "/v1/status")[0]["outboundAttempts"] == attempts
        assert persisted_vectors(data, index["profile"]) == vectors
        schema_after = corpus_schema_version(data)
        assert schema_after >= schema_before
        save(report, "package-readback.json", {"package": package, "index": index, "answer": answer,
             "vectors": vectors, "settings": settings, "workers": workers,
             "schemaVersionBefore": schema_before, "schemaVersionAfter": schema_after,
             "outboundAttemptsBefore": attempts, "outboundAttemptsAfter": attempts})
    return settings


def run(bundle, repo, assets, report):
    if not __debug__:
        raise RuntimeError("Workload qualification requires Python assertions.")
    runtime = (bundle / RUNTIME_MAIN).resolve(strict=True)
    git = GitHistory(repo, [BASE, TIP])
    assert git.roots[BASE] != git.roots[TIP]
    mode = os.getenv("CTXFAB_M11_WORKLOAD_MODE", "controlled")
    assert mode in ("controlled", "natural"), "Choose controlled or natural workload mode"
    timeline = OriginTimeline(git.roots[TIP], controlled=(mode == "controlled"))
    with TemporaryDirectory(prefix="ctxfab-mixed-workload-") as directory, origin(git, timeline.observe) as (port, requests):
        release = None
        previous = None
        if package := os.getenv("CTXFAB_M11_WORKLOAD_RELEASE_PACKAGE"):
            release = read_package(Path(package), Path(directory) / "release", None)
            assert digest(release[0] / RUNTIME_MAIN) == digest(runtime)
        if previous_package := os.getenv("CTXFAB_M11_WORKLOAD_PREVIOUS_PACKAGE"):
            if release is None:
                raise ValueError("Select a release package for upgrade qualification.")
            previous = read_package(Path(previous_package), Path(directory) / "previous", None)
            if version_key(previous[1]) >= version_key(release[1]):
                raise ValueError("The upgrade candidate must be newer than the previous package.")
        data = Path(directory) / "data"
        data.mkdir(mode=0o700)
        inference = data / "inference"
        inference.mkdir(mode=0o700)
        for filename in WEIGHTS:
            shutil.copyfile(assets / filename, inference / filename)
        setup = report / "setup"
        setup.mkdir()
        with backend(bundle, data, port, setup, hub_port=port, preview_runtime=False) as api:
            prepare_answers(api, setup, embeddings=True, expect_download=False)
            api("PUT", INDEX, {"enabled": False})
            assert not requests
        started = time.monotonic_ns()
        with backend(bundle, data, port, report, hub_port=port, preview_runtime=False) as api:
            status_sample(api)
            ready_ms = (time.monotonic_ns() - started)/1_000_000
            assert api("GET", "/v1/runtime")[0]["state"] == "ready"
            with ProcessSampler(api.process_id) as idle:
                time.sleep(1)
            idle_report = idle.report()
            assert [process["pid"] for process in idle_report["processes"]] == [api.process_id]
            with ProcessSampler(api.process_id) as resources:
                base, cold, epoch, original_attempts = first_phase(api, data, git, bundle)
                stats_before = storage_sizes(data)
                proof_progress = []
                wait_until(15, lambda: progress_sample(api, proof_progress)["indexed"] > 0 or None)
                assert proof_progress[-1]["indexed"] < proof_progress[-1]["total"]
                measure = stamp()["mono_us"]
                git.tip = TIP
                timeline.armed = True
                with ThreadPoolExecutor(max_workers=3) as pool:
                    refresh = pool.submit(api, "POST", REFRESH)
                    try:
                        warm = overlap_answer(api, bundle, data, git, timeline, pool, measure, epoch, report)
                        workers = verify_runtime_workers(api.process_id, runtime, embedding=True)
                        if timeline.controlled:
                            exercise_fault(api, runtime, git, timeline, pool, refresh, report)
                    finally:
                        timeline.release.set()
                    result, _ = refresh.result(timeout=150)
                    assert result == {"commit": TIP, "changed": True}, result
                tip = read_manifest(data, git, TIP)
                final_index = wait_until(300, lambda: index_complete(api))
                final_answer = timed_cli(bundle, data, git, TIP)
                assert final_answer["answer"]["mode"] == "hybrid"
                final_traffic = api("GET", "/v1/github")[0]["traffic"]
                final_status = api("GET", "/v1/status")[0]
                events = timeline.since(measure)
                assert final_traffic["attempts"] - original_attempts == len(events)
                assert final_status["outboundAttempts"] == final_traffic["attempts"] == len(requests)
                assert final_traffic["pending"] == final_traffic["uncertain"] == 0, final_traffic
                pacing = rate_window(events)
                assert pacing["peakArrivalsPerRollingMinute"] <= 60 and pacing["peakConcurrentOriginRequests"] == 1, pacing
                status_times = [status_sample(api) for _ in range(20)]
                save(report, "workload.json", {"mode": mode, "startupAuthenticatedReadyMs": ready_ms,
                     "startupEndpointDiscoveredMs": api.discovered_ms, "cold": cold, "warm": warm,
                     "tipHybridAnswer": final_answer, "base": base, "tip": tip, "index": final_index,
                     "traffic": final_traffic, "measuredOriginRequests": len(events), "pacing": pacing,
                     "runtimeWorkers": workers, "bundledRuntime": str(runtime.relative_to(bundle.resolve())),
                     "idleResources": idle_report, "storageBefore": stats_before, "storageAfter": storage_sizes(data),
                     "unloadedStatusP95Ms": percentile95(status_times)})
                save(report, "origin-intervals.json", events)
            save(report, "resources.json", resources.report())
            if release is not None:
                state, _ = api("GET", "/v1/github")
                disconnected, _ = api("DELETE", f"/v1/github/credential?revision={state['revision']}")
                assert disconnected["credential"] == "unconfigured"
                assert api("GET", "/v1/status")[0]["outboundAttempts"] == len(requests)
        if release is not None:
            vectors = persisted_vectors(data, final_index["profile"])
            retained = None
            if previous is not None:
                retained = verify_release_readback(
                    previous, data, git, report / "previous-readback", final_index, vectors, len(requests),
                    previous=True,
                )
            verify_release_readback(
                release, data, git, report / "release-readback", final_index, vectors, len(requests),
                retained_settings=retained,
            )


if __name__ == "__main__":
    run(*map(Path, sys.argv[1:]))
