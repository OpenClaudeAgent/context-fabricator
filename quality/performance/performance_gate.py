"""Evaluate paired workload reports against provisional reference-Mac bounds."""

import argparse
import json
import math
import re
import sys
from pathlib import Path

from workload import BASE, GENERATION_PROFILE, TIP, WEIGHTS, rate_window

MIB = 1024**2
LIMITS = {
    "authenticatedStartupMs": 600,
    "coldCitedAnswerMs": 12_000,
    "warmCitedAnswerMs": 3_000,
    "indexedHybridAnswerMs": 10_000,
    "loadedStatusP95Ms": 50,
    "idleStatusP95Ms": 50,
    "peakSampledFootprintBytes": 2_560 * MIB,
    "sampledProcessWriteBytes": 256 * MIB,
    "finalSqliteFilesBytes": 64 * MIB,
    "releaseReopenAnswerMs": 20_000,
    "replacementCitedAnswerMs": 20_000,
}


def require(condition, description):
    if not condition:
        raise ValueError(description)


def read_json(directory, relative):
    return json.loads((directory / relative).read_text(encoding="utf-8"))


def number(value, label, minimum=0):
    require(
        type(value) in (int, float) and math.isfinite(value) and value >= minimum,
        f"{label} must be finite and at least {minimum}",
    )
    return value


def sha256(value):
    require(
        isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value),
        "Invalid SHA-256",
    )
    return value


def fingerprints(directory, filename, names):
    actual = {}
    for line in (directory / filename).read_text(encoding="utf-8").splitlines():
        digest, path = line.split("  ", 1)
        name = Path(path).name
        require(name not in actual, f"{filename} has a duplicate {name}")
        actual[name] = sha256(digest)
    require(
        actual.keys() == set(names),
        f"{filename} does not identify all measured binaries",
    )
    return actual


def cited_answer(record, label):
    answer = record["answer"]
    require(
        answer["outcome"] == "answered"
        and answer["model"] == GENERATION_PROFILE
        and answer["citations"],
        f"{label} lacks the pinned model and a cited answer",
    )
    return number(record["wallMilliseconds"], label, 0.001)


def workload_evidence(directory, mode):
    workload = read_json(directory, "workload.json")
    overlap = read_json(directory, "overlap-evidence.json")
    require(workload["mode"] == mode, f"Expected the {mode} workload")
    require(
        (workload["base"]["commit"], workload["tip"]["commit"]) == (BASE, TIP),
        "The workload did not use the pinned BASE and TIP",
    )
    index = workload["index"]
    require(
        index["indexed"] == index["total"] == 753
        and index["state"] == "ready"
        and index["dimensions"] == 1024
        and index["profile"] == "qwen3-embedding-0.6b-q8_0-06507c7b-d1024-v1",
        "The full reference index is missing",
    )
    require(
        workload["cold"]["answer"]["mode"] == "bm25"
        and workload["tipHybridAnswer"]["answer"]["mode"] == "hybrid",
        "The cold lexical and indexed hybrid paths are required",
    )
    traffic = workload["traffic"]
    require(
        traffic["pending"] == traffic["uncertain"] == 0
        and traffic["concurrency"] == 1
        and traffic["effectiveRequestsPerMinute"] == 60,
        "Provider accounting did not settle under the reference limits",
    )
    events = read_json(directory, "origin-intervals.json")
    require(
        len(events) >= 2 and len(events) == workload["measuredOriginRequests"],
        "Origin intervals are incomplete",
    )
    for event in events:
        started = number(event["started"]["mono_us"], "Origin start", 1)
        number(event["finished"]["mono_us"], "Origin completion", started + 1)
    pacing = rate_window(events)
    require(
        pacing == workload["pacing"]
        and pacing["peakArrivalsPerRollingMinute"] <= 60
        and pacing["peakConcurrentOriginRequests"] == 1,
        "Observed provider traffic exceeds its admission limits",
    )
    require(
        overlap["concurrentRequest"]["rejectedBusy"] is True,
        "The concurrent answer was admitted",
    )
    require(
        number(overlap["statusLatency"]["samples"], "Warm status sample count") >= 5,
        "The warm status latency needs at least five observations",
    )
    recovery = None
    if mode == "controlled":
        require(
            overlap["proof"]["certified"] is True,
            "The three-way overlap was not established",
        )
        recovery = read_json(directory, "generation-recovery.json")
        require(
            (recovery["failedStatus"], recovery["failedCode"])
            == (503, "generation_runtime_unavailable")
            and recovery["recoveryCitationVerified"] is True,
            "Controlled worker recovery did not return its typed failure and a cited answer",
        )
    return workload, overlap, recovery


def resource_measurements(directory, workload):
    resources = read_json(directory, "resources.json")
    require(
        number(resources["sample_count"], "Process sample count") >= 100,
        "Insufficient resource samples",
    )
    fatal = {
        "root_unavailable",
        "child_enumeration_failed",
        "child_list_limit_reached",
        "process_limit_reached",
    }
    require(
        not any(
            key in fatal or key.startswith("sampler_error_")
            for key in resources["missing_counters"]
        ),
        "Resource sampling had an unrecoverable gap",
    )
    processes = resources["processes"]
    samples = {
        item["pid"]: number(item["samples"], "Per-process sample count", 1)
        for item in processes
    }
    workers = workload["runtimeWorkers"]
    expected = {resources["root_pid"], workers["generation"]["worker"]["pid"]}
    expected.update(item["worker"]["pid"] for item in workers["embedding"])
    require(
        len(expected) >= 3 and expected <= samples.keys(),
        "Helper and runtime worker samples are required",
    )
    writes = sum(
        number(item["write_delta_bytes"], "Sampled process writes")
        for item in processes
    )
    storage = workload["storageAfter"]
    require(
        {"context.sqlite3", "telemetry.sqlite3"} <= storage.keys(),
        "Final SQLite file measurements are missing",
    )
    return {
        "peakSampledFootprintBytes": number(
            resources["peak_simultaneous_footprint_bytes"], "Peak footprint", 1
        ),
        "sampledProcessWriteBytes": writes,
        "finalSqliteFilesBytes": sum(
            number(size, "SQLite file size") for size in storage.values()
        ),
    }


def release_measurement(directory, workload):
    readback = read_json(directory, "release-readback/package-readback.json")
    require(
        readback["index"] == workload["index"]
        and readback["vectors"]["count"] == workload["index"]["indexed"]
        and readback["answer"]["answer"]["mode"] == "hybrid",
        "The extracted release did not reopen the fully indexed corpus",
    )
    attempts = workload["traffic"]["attempts"]
    require(
        readback["outboundAttemptsBefore"]
        == readback["outboundAttemptsAfter"]
        == attempts,
        "The extracted release made outbound requests while answering from retained data",
    )
    package = readback["package"]
    sha256(package["archiveSHA256"])
    sha256(readback["vectors"]["sha256"])
    hashes = {
        binary["file"]: sha256(binary["sha256"]) for binary in package["binaries"]
    }
    require(
        hashes.keys() == {"Context Fabricator", "ctxfab-server", "ctxfab"}
        and len(hashes) == len(package["binaries"]),
        "The extracted package needs all three binary fingerprints",
    )
    return package, cited_answer(readback["answer"], "Release reread answer latency")


def evaluate(directory, mode):
    host = read_json(directory, "reference-host.json")
    require(
        host["architecture"] == "arm64"
        and host["cpu"] == "Apple M1 Max"
        and host["memoryBytes"] == 32 * 1024**3
        and host["os"].startswith("26."),
        "The reference host is Apple M1 Max, 32 GiB, macOS 26",
    )
    bundle = {
        **fingerprints(
            directory, "helper-fingerprints.txt", ("ctxfab-server", "ctxfab")
        ),
        **fingerprints(directory, "runtime-fingerprint.txt", ("llama-server",)),
    }
    model_checks = (directory / "model-integrity.log").read_text(encoding="utf-8")
    require(
        all(f"{file}: OK" in model_checks.splitlines() for file in WEIGHTS),
        "Pinned model integrity checks are missing",
    )
    runtime = (directory / "runtime.log").read_text(encoding="utf-8")
    require(
        "build 12359, commit b1b770d90" in runtime, "Unexpected bundled runtime build"
    )
    workload, overlap, recovery = workload_evidence(directory, mode)
    package, release_ms = release_measurement(directory, workload)
    values = {
        "authenticatedStartupMs": number(
            workload["startupAuthenticatedReadyMs"], "Authenticated startup", 0.001
        ),
        "coldCitedAnswerMs": cited_answer(workload["cold"], "Cold cited answer"),
        "warmCitedAnswerMs": cited_answer(workload["warm"], "Warm cited answer"),
        "indexedHybridAnswerMs": cited_answer(
            workload["tipHybridAnswer"], "Indexed hybrid answer"
        ),
        "loadedStatusP95Ms": number(
            overlap["statusLatency"]["p95Milliseconds"], "Warm status p95", 0.001
        ),
        "idleStatusP95Ms": number(
            workload["unloadedStatusP95Ms"], "Idle status p95", 0.001
        ),
        **resource_measurements(directory, workload),
        "releaseReopenAnswerMs": release_ms,
    }
    if recovery is not None:
        values["replacementCitedAnswerMs"] = number(
            recovery["recoveryWallMs"], "Worker recovery", 0.001
        )
    checked = {
        name: {
            "observed": value,
            "maximum": LIMITS[name],
            "passed": value <= LIMITS[name],
        }
        for name, value in values.items()
    }
    return {
        "mode": mode,
        "host": host,
        "binaries": bundle,
        "releasePackage": package,
        "warmStatusSamples": overlap["statusLatency"]["samples"],
        "checks": checked,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("controlled", type=Path)
    parser.add_argument("natural", type=Path)
    arguments = parser.parse_args()
    try:
        runs = [
            evaluate(arguments.controlled, "controlled"),
            evaluate(arguments.natural, "natural"),
        ]
        for key in ("host", "binaries", "releasePackage"):
            require(
                runs[0][key] == runs[1][key],
                f"Controlled and natural reports disagree on {key}",
            )
        passed = all(
            check["passed"] for run in runs for check in run["checks"].values()
        )
        report = {
            "type": "provisional reference-Mac qualification",
            "passed": passed,
            "runs": runs,
        }
    except (OSError, KeyError, ValueError, TypeError, OverflowError) as error:
        passed = False
        report = {"passed": False, "invalidEvidence": str(error)}
    print(json.dumps(report, indent=2, allow_nan=False))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
