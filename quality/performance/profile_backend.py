"""Measure a packaged macOS backend on a temporary, unconnected profile."""

import argparse
import hashlib
import json
import math
import platform
import statistics
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from tempfile import TemporaryDirectory
from urllib.parse import urlsplit

quality = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(quality / "github"))

from dogfood import BackendSession, backend, client_environment, save, wait_until
from process_sampling import ProcessSampler

WORKERS = 3
REQUESTS_PER_WORKER = 45
ROUNDS = 3
SAMPLE_INTERVAL_SECONDS = 0.1
QUIET_SECONDS = 3
REQUEST_SPACING_SECONDS = 0.025
STATUS = "/v1/status"
SEARCH = "/v1/search?query=headlessprofile&mode=bm25"


def validate_status(status):
    if (
        status["status"] != "ready"
        or status["connectedSources"] != 0
        or status["outboundAttempts"] != 0
    ):
        raise RuntimeError("Expected a ready, disconnected backend without outbound attempts.")


def request_burst(api):
    results = []
    for index in range(REQUESTS_PER_WORKER):
        route = STATUS if index % 3 == 0 else SEARCH
        started = time.monotonic()
        response, _ = api("GET", route)
        latency_ms = (time.monotonic() - started) * 1000
        if route == STATUS:
            validate_status(response)
        elif response != []:
            raise RuntimeError("Expected no search hits in the fresh profile.")
        results.append(latency_ms)
        time.sleep(REQUEST_SPACING_SECONDS)
    return results


def load_phase(api):
    with ThreadPoolExecutor(max_workers=WORKERS) as pool:
        futures = [pool.submit(request_burst, api) for _ in range(WORKERS)]
        return sorted(latency for future in futures for latency in future.result())


def idle_phase(_api):
    time.sleep(QUIET_SECONDS)
    return []


def measure(api, label, run):
    with ProcessSampler(api.process_id, interval=SAMPLE_INTERVAL_SECONDS) as sampler:
        latencies = run(api)
    sample = sampler.report()
    if sample["missing_counters"] or sample["sample_count"] < 2 or len(sample["processes"]) != 1:
        raise RuntimeError(f"Incomplete resource measurement for {label}: {sample['missing_counters']}")
    helper = sample["processes"][0]
    residents = sampler.processes()
    if helper["pid"] != api.process_id or not residents:
        raise RuntimeError(f"Backend process absent in measurement for {label}.")
    result = {
        "phase": label,
        "requestCount": len(latencies),
        "elapsedSeconds": sample["elapsed_seconds"],
        "processSamples": helper["samples"],
        "meanCpuPercentOfOneCore": helper["cpu_time_delta_ns"] / (sample["elapsed_seconds"] * 1e7),
        "peakSampledCpuPercentOfOneCore": helper["peak_sampled_cpu_percent"],
        "peakResidentBytes": helper["peak_rss_bytes"],
        "endingResidentBytes": residents[0]["rss_bytes"],
        "peakPhysicalFootprintBytes": helper["peak_footprint_bytes"],
        "processWriteDeltaBytes": helper["write_delta_bytes"],
    }
    if latencies:
        result["requestLatencyMs"] = {
            "median": statistics.median(latencies),
            "p95": latencies[math.ceil(len(latencies) * 0.95) - 1],
            "max": latencies[-1],
        }
    return result


def measure_leaks(bundle, report):
    helper = bundle / "Contents/MacOS/ctxfab-server"
    log_path = report / "leaks.log"
    with TemporaryDirectory(prefix="ctxfab-leaks-") as temp, log_path.open("w") as output:
        data = Path(temp) / "profile"
        endpoint_path = data / "endpoint.json"
        command = ["/usr/bin/leaks", "--quiet", "--noContent", "--atExit", "--",
                   helper, data, "--app-session"]
        with subprocess.Popen(
            command, stdin=subprocess.PIPE, stdout=output, stderr=subprocess.STDOUT,
            env=client_environment(),
        ) as tool:
            def ready():
                if tool.poll() is not None:
                    raise RuntimeError("Leak checker could not launch the backend. See leaks.log.")
                return json.loads(endpoint_path.read_text()) if endpoint_path.is_file() else None

            try:
                endpoint = wait_until(10, ready)
                url = urlsplit(endpoint["url"])
                if url.scheme != "http" or url.hostname != "127.0.0.1":
                    raise RuntimeError("Leak check requires a loopback-only backend.")
                api = BackendSession(endpoint, 0)
                validate_status(api("GET", STATUS)[0])
                load_phase(api)
                validate_status(api("GET", STATUS)[0])
            finally:
                tool.stdin.close()
                try:
                    tool.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    tool.kill()
                    tool.wait(timeout=5)
                    raise
        if tool.returncode != 0 or endpoint_path.exists():
            raise RuntimeError(f"Leak check returned {tool.returncode} or exited uncleanly. See leaks.log.")
    if "0 leaks for 0 total leaked bytes" not in log_path.read_text():
        raise RuntimeError("Expected a complete zero-leak report. See leaks.log.")


def run(bundle, report):
    subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", bundle], check=True)
    with (bundle / "Contents/MacOS/ctxfab-server").open("rb") as helper:
        fingerprint = hashlib.file_digest(helper, "sha256").hexdigest()
    report.mkdir(parents=True, exist_ok=True)
    with (
        TemporaryDirectory(prefix="ctxfab-profile-") as temp,
        backend(bundle, Path(temp) / "profile", None, report) as api,
    ):
        validate_status(api("GET", STATUS)[0])
        phases = [measure(api, "idle-before", idle_phase)]
        for index in range(1, ROUNDS + 1):
            phases.append(measure(api, f"load-{index}", load_phase))
            phases.append(measure(api, f"idle-after-{index}", idle_phase))
        validate_status(api("GET", STATUS)[0])

    save(report, "profile.json", {
        "backendSHA256": fingerprint,
        "system": platform.platform(),
        "sampleIntervalSeconds": SAMPLE_INTERVAL_SECONDS,
        "clientsPerLoadPhase": WORKERS,
        "requestsPerClientPerLoadPhase": REQUESTS_PER_WORKER,
        "requestSpacingSeconds": REQUEST_SPACING_SECONDS,
        "outboundAttempts": 0,
        "phases": phases,
    })


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("bundle", type=Path, help="Path to a previously built Context Fabricator.app")
    parser.add_argument("report", type=Path, help="Output directory for the summary and helper log")
    parser.add_argument("--leaks", action="store_true", help="Scan a fresh helper at exit with Apple leaks")
    args = parser.parse_args()
    run(args.bundle.resolve(), args.report.resolve())
    if args.leaks:
        measure_leaks(args.bundle.resolve(), args.report.resolve())


if __name__ == "__main__":
    main()
