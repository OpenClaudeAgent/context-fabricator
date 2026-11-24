"""Crash one isolated generation worker and verify a cited recovery.

The caller owns the helper process and supplies an authenticated loopback API and
an evidence verifier. This fixture never searches for an unrelated process to kill.
"""

import hashlib
import json
import os
import re
import signal
import subprocess
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from threading import Event
from urllib.error import HTTPError

_GUARD = re.compile(r"(?:^|\s)--guard-inference-worker\s+(generation|embedding)(?:\s|$)")


def _processes():
    """Read only PID and parent PID for system-wide child discovery."""
    result = subprocess.run(
        ["/bin/ps", "-axo", "pid=,ppid="],
        check=True, capture_output=True, text=True, timeout=5,
    )
    rows = {}
    for line in result.stdout.splitlines():
        fields = line.split()
        if len(fields) == 2 and all(field.isdigit() for field in fields):
            rows[int(fields[0])] = int(fields[1])
    return rows


def _fixture_process(pid, expected_parent):
    """Read a command only for the fixture helper or an identified descendant."""
    result = subprocess.run(
        ["/bin/ps", "-p", str(pid), "-o", "pid=,ppid=,lstart=,command="],
        check=False, capture_output=True, text=True, timeout=5,
    )
    fields = result.stdout.strip().split(None, 7)
    if (result.returncode != 0 or len(fields) != 8 or not fields[0].isdigit()
            or not fields[1].isdigit() or int(fields[0]) != pid
            or int(fields[1]) != expected_parent):
        raise AssertionError("Fixture process identity or parent changed during inspection")
    return {"pid": pid, "parent": expected_parent, "started": " ".join(fields[2:7]),
            "command": fields[7]}


def _executable(pid):
    result = subprocess.run(
        ["/bin/ps", "-p", str(pid), "-o", "comm="],
        check=False, capture_output=True, text=True, timeout=5,
    )
    if result.returncode != 0 or not result.stdout.strip():
        raise AssertionError(f"Process {pid} is no longer identifiable")
    return result.stdout.strip()


def _owned_workers(helper_pid, runtime, helper_identity=None):
    runtime = runtime.resolve(strict=True)
    parents = _processes()
    if helper_pid not in parents:
        raise AssertionError("The fixture helper changed or exited")
    helper = _fixture_process(helper_pid, parents[helper_pid])
    if helper_identity is not None and _identity(helper) != helper_identity:
        raise AssertionError("The fixture helper changed or exited")
    helper_executable = _executable(helper_pid)
    if not Path(helper_executable).name.startswith("ctxfab-server"):
        raise AssertionError("The supplied PID is not the fixture backend helper")
    found = {"generation": [], "embedding": []}
    for guardian_pid, parent in parents.items():
        if parent != helper_pid:
            continue
        guardian = _fixture_process(guardian_pid, helper_pid)
        match = _GUARD.search(guardian["command"])
        if not match:
            continue
        if _executable(guardian_pid) != helper_executable:
            raise AssertionError("Guardian executable differs from its helper")
        children = [pid for pid, worker_parent in parents.items() if worker_parent == guardian_pid]
        if len(children) != 1:
            raise AssertionError("Expected exactly one worker owned by each guardian")
        worker = _fixture_process(children[0], guardian_pid)
        if Path(_executable(worker["pid"])).resolve(strict=True) != runtime:
            raise AssertionError("Guardian child does not execute the bundled inference engine")
        role = match.group(1)
        is_embedding = bool(re.search(r"(?:^|\s)--embedding(?:\s|$)", worker["command"]))
        if is_embedding != (role == "embedding"):
            raise AssertionError("Worker role does not match its guardian")
        found[role].append({"guardian": _identity(guardian), "worker": _identity(worker)})
    if len(found["generation"]) != 1 or len(found["embedding"]) > 1:
        raise AssertionError("Expected one generation pair and at most one embedding pair")
    return _identity(helper), found["generation"][0], found["embedding"]


def _identity(row):
    return {"pid": row["pid"], "parent": row["parent"], "started": row["started"],
            "commandSha256": hashlib.sha256(row["command"].encode()).hexdigest()}


def verify_runtime_workers(helper_pid, runtime, *, embedding):
    _, generation, embeddings = _owned_workers(helper_pid, runtime)
    assert len(embeddings) == int(embedding), "Unexpected embedding-worker state"
    return {"generation": generation, "embedding": embeddings}


def _has_established_connection(pid):
    # This is supporting evidence, not proof that an individual request reached
    # generation: a connection may be kept alive between requests.
    try:
        result = subprocess.run(
            ["/usr/sbin/lsof", "-nP", "-a", "-p", str(pid), "-iTCP", "-sTCP:ESTABLISHED"],
            check=False, capture_output=True, text=True, timeout=3,
        )
        return result.returncode == 0 and len(result.stdout.splitlines()) > 1
    except (OSError, subprocess.TimeoutExpired):
        return None


def _assert_same_embedding(before, after):
    if before != after:
        raise AssertionError("The unrelated embedding worker or guardian changed")

def _failed_span(api, trace, fault_unix):
    if not trace:
        raise AssertionError("The interrupted request returned no trace ID")
    fault_us = round(fault_unix * 1_000_000)
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        try:
            detail, _ = api("GET", f"/v1/telemetry/traces/{trace}")
        except HTTPError as error:
            if error.code != 404:
                raise
            error.close()
        else:
            for span in detail["spans"]:
                start = span.get("startedAtUnixMicros", 0)
                if (span["name"] == "answer_generation" and span.get("level") == "error"
                        and start <= fault_us <= start + span.get("value", 0)):
                    return span
        time.sleep(0.1)
    raise AssertionError("No failed generation span contained the worker stop")

def run_generation_recovery(
    api, helper_pid, runtime, known_question, verify_answer, report, *, request_scope=None
):
    """Exercise one crash within the caller-owned helper and persist evidence."""
    if not isinstance(helper_pid, int) or helper_pid <= 1 or not known_question:
        raise ValueError("A fixture helper PID and known question are required")
    scope = {} if request_scope is None else dict(request_scope)
    if set(scope) - {"repositoryId", "branch"}:
        raise ValueError("Only repositoryId and branch may be supplied in request_scope")
    payload = {"query": known_question, **scope}
    helper, original, embedding = _owned_workers(helper_pid, runtime)
    if len(embedding) != 1:
        raise AssertionError("The mixed workload has no active embedding worker")
    entered = Event()

    def ask():
        entered.set()
        return api("POST", "/v1/answers", payload)

    with ThreadPoolExecutor(max_workers=1, thread_name_prefix="generation-recovery") as pool:
        active = pool.submit(ask)
        if not entered.wait(timeout=2):
            raise AssertionError("The fixture answer request did not start")
        time.sleep(0.15)  # Match the native supervised-answer crash test's active-request window.
        if active.done():
            raise AssertionError("The answer completed before the crash window")
        connection = _has_established_connection(original["worker"]["pid"])
        _, current, current_embedding = _owned_workers(helper_pid, runtime, helper)
        if current != original:
            raise AssertionError("The generation pair changed before the signal")
        _assert_same_embedding(embedding, current_embedding)
        if active.done():
            raise AssertionError("The answer completed before the signal")
        fault_monotonic = time.monotonic()
        fault_unix = time.time()
        os.kill(original["worker"]["pid"], signal.SIGKILL)
        outcome = active.exception(timeout=165)
    if not isinstance(outcome, HTTPError) or outcome.code != 503:
        raise AssertionError("The crashed request did not return the expected HTTP 503")
    failed_trace = outcome.headers.get("X-Context-Trace-Id") if outcome.headers else None
    with outcome:
        failure = json.load(outcome)
    if failure.get("code") != "generation_runtime_unavailable":
        raise AssertionError(f"Unexpected generation failure: {failure!r}")
    if failure.get("text") or failure.get("citations") or failure.get("answer"):
        raise AssertionError("The crashed request returned a usable answer")
    recovery_start = time.monotonic()
    recovery_start_unix = time.time()
    answer, trace = api("POST", "/v1/answers", payload)
    recovery_end = time.monotonic()
    recovery_end_unix = time.time()
    if verify_answer(answer) is False:
        raise AssertionError("The recovery answer failed citation verification")
    failed_span = _failed_span(api, failed_trace, fault_unix)
    _, replacement, after_embedding = _owned_workers(helper_pid, runtime, helper)
    _assert_same_embedding(embedding, after_embedding)
    if (replacement["worker"]["pid"] == original["worker"]["pid"]
            or replacement["guardian"]["pid"] == original["guardian"]["pid"]):
        raise AssertionError("The generation worker and guardian were not both replaced")
    deadline = time.monotonic() + 10
    while True:
        rows = _processes()
        alive = [identity for identity in original.values() if identity["pid"] in rows]
        if not alive:
            break
        if time.monotonic() >= deadline:
            raise AssertionError("The original generation process was not reaped")
        time.sleep(0.05)
    evidence = {
        "helper": helper, "originalGeneration": original,
        "replacementGeneration": replacement, "embedding": embedding,
        "workerEstablishedConnectionObserved": connection,
        "failedStatus": 503, "failedCode": failure["code"], "failedTraceId": failed_trace,
        "failedGenerationSpan": failed_span,
        "faultMonotonicSeconds": fault_monotonic, "faultUnixSeconds": fault_unix,
        "recoveryStartMonotonicSeconds": recovery_start, "recoveryStartUnixSeconds": recovery_start_unix,
        "recoveryEndMonotonicSeconds": recovery_end, "recoveryEndUnixSeconds": recovery_end_unix,
        "recoveryWallMs": round((recovery_end - recovery_start) * 1000, 3),
        "requestScope": scope,
        "recoveryTraceId": trace, "recoveryCitationVerified": True,
    }
    destination = Path(report) / "generation-recovery.json"
    destination.write_text(json.dumps(evidence, indent=2) + "\n", encoding="utf-8")
    return evidence
