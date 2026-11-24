"""Exercise runtime admission with a packaged helper pinned to an ad-hoc fixture ZIP."""

import argparse
import json
import os
import subprocess
import time
import zipfile
from pathlib import Path
from tempfile import TemporaryDirectory
from urllib.error import HTTPError
from urllib.request import ProxyHandler, Request, build_opener

GENERATION_PROFILE = "qwen3-4b-instruct-2507-q4-k-m-3605803b-v1"


def request(endpoint, route, payload=None, *, authenticated=True, method=None, timeout=20):
    headers = {"Content-Type": "application/json"}
    if authenticated:
        headers["Authorization"] = "Bearer " + endpoint["token"]
    body = json.dumps(payload).encode() if payload is not None else None
    opener = build_opener(ProxyHandler({}))
    try:
        with opener.open(Request(endpoint["url"] + route, body, headers, method=method), timeout=timeout) as response:
            return response.status, None if response.status == 204 else json.load(response)
    except HTTPError as error:
        with error:
            return error.code, json.load(error)


def wait_for_endpoint(data, process):
    deadline = time.monotonic() + 10
    description = data / "endpoint.json"
    while time.monotonic() < deadline and process.poll() is None:
        if description.is_file():
            return json.loads(description.read_text(encoding="utf-8"))
        time.sleep(0.05)
    raise AssertionError("The packaged helper did not publish a local session.")


def check_install(endpoint, data, archive, hosted):
    expected_bytes = archive.stat().st_size
    capabilities = {"canDownload": True} if hosted else {}
    assert request(endpoint, "/v1/runtime", authenticated=False)[0] == 401
    assert request(endpoint, "/v1/runtime") == (
        200, {"state": "availableForInstall", "archiveBytes": expected_bytes, **capabilities},
    )
    path = "/v1/runtime/install"
    assert request(endpoint, path, {"archivePath": "relative.zip"})[0] == 400
    code, rejection = request(endpoint, path, {"archivePath": str(archive)})
    assert code == 200
    assert rejection == {
        "state": "failed",
        "archiveBytes": expected_bytes,
        "error": "The archive could not be installed. Choose the exact runtime package for this app release.",
        **capabilities,
    }
    assert request(endpoint, "/v1/runtime") == (200, rejection)
    assert not list((data / "managed-runtime").iterdir())
    assert request(endpoint, "/v1/generation/models") == (200, {"profiles": []})
    return rejection


def check_capacity(endpoint, data, archive, hosted):
    code, settings = request(endpoint, "/v1/settings")
    assert code == 200 and settings["inferenceArtifactLimitGb"] == 16
    settings["inferenceArtifactLimitGb"] = 1
    assert request(endpoint, "/v1/settings", settings, method="PUT")[0] == 200
    inference = data / "inference"
    inference.mkdir(exist_ok=True)
    with (inference / "disk-budget-fixture").open("wb") as occupied:
        occupied.truncate(990_000_000)
    code, plan = request(endpoint, "/v1/inference/storage")
    assert code == 200
    assert plan["capacity"]["limitBytes"] == 1_000_000_000
    assert plan["capacity"]["usedBytes"] == 990_000_000
    assert plan["capacity"]["availableBytes"] == 10_000_000
    assert plan["capacity"]["reservedBytes"] == 0
    with zipfile.ZipFile(archive) as package:
        entries = package.infolist()
        expanded = sum(entry.file_size for entry in entries)
        bundle_bytes = sum(entry.file_size for entry in entries
                           if not entry.filename.startswith("__MACOSX/"))
    local_peak = archive.stat().st_size + expanded
    assert plan["runtime"]["localImport"] == {"peakBytes": local_peak, "fits": False}
    if hosted:
        remote_peak = local_peak + archive.stat().st_size
        assert plan["runtime"]["hostedDownload"] == {"peakBytes": remote_peak, "fits": False}
    model = next(entry for entry in plan["models"] if entry["profile"] == GENERATION_PROFILE)
    assert model == {"profile": GENERATION_PROFILE, "peakBytes": 2_497_281_120, "fits": False}
    aggregate = check_setup_plan(endpoint, archive, hosted, archive.stat().st_size + bundle_bytes)
    failure_message = "The inference storage limit or available disk space is too low."
    code, result = request(endpoint, "/v1/runtime/install", {"archivePath": str(archive)})
    assert code == 200 and result["state"] == "failed" and result["error"] == failure_message
    if hosted:
        code, result = request(endpoint, "/v1/runtime/download", method="POST")
        assert code == 200 and result["state"] == "installing"
        deadline = time.monotonic() + 5
        while result["state"] == "installing" and time.monotonic() < deadline:
            time.sleep(0.05)
            code, result = request(endpoint, "/v1/runtime")
            assert code == 200
        assert result["state"] == "failed" and result["error"] == failure_message
    code, status = request(endpoint, "/v1/status")
    assert code == 200 and status["outboundAttempts"] == 0
    assert not list((data / "managed-runtime").iterdir())
    return {"localImportRefused": True, "hostedDownloadRefused": hosted, "outboundAttempts": 0,
            "plan": plan, "aggregateSetup": aggregate}


def check_setup_plan(endpoint, archive, hosted, retained_runtime):
    route = "/v1/inference/setup"
    selection = {"runtimeSource": "hosted" if hosted else "localArchive", "embedding": True}
    assert request(endpoint, route + "/plan", selection, authenticated=False)[0] == 401
    assert request(endpoint, route)[0] == 404
    code, plan = request(endpoint, route + "/plan", selection)
    assert code == 200 and plan["selection"]["answerProfile"] == GENERATION_PROFILE
    weights = 2_497_281_120 + 639_150_592
    assert plan["peakAdditionalBytes"] == retained_runtime + weights
    assert plan["maximumTransferBytes"] == weights + (archive.stat().st_size if hosted else 0)
    assert plan["availableBytes"] == 10_000_000 and not plan["canStart"]
    assert plan["blocker"] == "storage_limit" and plan["runtimeRequired"]
    confirmation = {"selection": plan["selection"], "operationId": "1a68cd69-6748-4769-8bdc-b0b1e9b021ff"}
    if not hosted:
        confirmation["archivePath"] = str(archive)
    code, problem = request(endpoint, route, {**confirmation, "fingerprint": "0" * 64})
    assert code == 409 and problem["code"] == "stale_inference_setup_plan"
    code, problem = request(endpoint, route, {**confirmation, "fingerprint": plan["fingerprint"]})
    assert code == 409 and problem["code"] == "inference_setup_blocked"
    assert request(endpoint, route)[0] == 404
    return plan


def check_legacy_weight(endpoint, data, legacy):
    destination = data / "inference" / "Qwen3-4B-Instruct-2507-Q4_K_M.gguf"
    subprocess.run(["/bin/cp", "-c", legacy, destination], check=True)

    def model():
        code, catalog = request(endpoint, "/v1/model-downloads")
        assert code == 200
        return next(item for item in catalog if item["profile"] == GENERATION_PROFILE)

    assert model()["state"] == "notInstalled"
    start = time.monotonic()
    assert request(endpoint, "/v1/model-downloads", {"profile": GENERATION_PROFILE})[0] == 202
    deadline = start + 60
    result = model()
    while result["state"] == "installing" and time.monotonic() < deadline:
        time.sleep(0.1)
        result = model()
    assert result["state"] == "installed" and result["downloadedBytes"] == legacy.stat().st_size
    code, plan = request(endpoint, "/v1/inference/storage")
    assert code == 200
    installed_plan = next(entry for entry in plan["models"] if entry["profile"] == GENERATION_PROFILE)
    assert installed_plan["peakBytes"] == 0 and installed_plan["fits"]
    elapsed = round((time.monotonic() - start) * 1000)
    artifact = "/v1/model-downloads/" + GENERATION_PROFILE
    code, reviewed = request(endpoint, artifact + "/removal-plan")
    assert code == 200 and reviewed["bytes"] == legacy.stat().st_size
    replacement = destination.with_suffix(".sparse-fixture")
    with replacement.open("wb") as wrong:
        wrong.truncate(legacy.stat().st_size)
    replacement.replace(destination)
    assert model()["state"] == "notInstalled"
    code, rejection = request(endpoint, artifact, {"identity": reviewed["identity"]}, method="DELETE")
    assert code == 409 and rejection["code"] == "stale_model_removal" and destination.is_file()
    code, current = request(endpoint, artifact + "/removal-plan")
    assert code == 200 and current["identity"] != reviewed["identity"]
    for _ in range(2):
        assert request(endpoint, artifact, {"identity": current["identity"]}, method="DELETE")[0] == 204
    assert not destination.exists()
    code, status = request(endpoint, "/v1/status")
    assert code == 200 and status["outboundAttempts"] == 0
    return {"bytesVerified": legacy.stat().st_size, "elapsedMilliseconds": elapsed,
            "sameSizeReplacementRefused": True, "staleRemovalRefused": True,
            "confirmedRemovalIdempotent": True, "outboundAttempts": 0}


def check_packaged_cli(cli, endpoint, data, archive):
    (data / "inference" / "disk-budget-fixture").unlink()
    code, settings = request(endpoint, "/v1/settings")
    assert code == 200
    settings["inferenceArtifactLimitGb"] = 16
    assert request(endpoint, "/v1/settings", settings, method="PUT")[0] == 200
    environment = {**os.environ, "CTXFAB_DATA_DIRECTORY": str(data)}

    def read(*args):
        return json.loads(subprocess.check_output([cli, *args], env=environment, timeout=15))

    plan = read("setup-plan", "localArchive", "answers")
    assert plan["canStart"] and plan["runtimeRequired"]
    direct = request(endpoint, "/v1/inference/setup/plan", plan["selection"])
    assert direct == (200, plan)
    plan_file = data.parent / "reviewed-plan.json"
    plan_file.write_text(json.dumps(plan), encoding="utf-8")
    identifier = "f015f382-88c4-4d28-bac3-5892f904f829"
    started = read("setup-start", plan_file, identifier, archive)
    assert started["operationId"] == identifier and started["phase"] == "runtime"
    deadline = time.monotonic() + 10
    status = read("setup-status")
    while status["phase"] == "runtime" and time.monotonic() < deadline:
        time.sleep(0.05)
        status = read("setup-status")
    assert status["phase"] == "failed" and not status["answerReady"]
    assert status["operationId"] == identifier and str(archive) not in json.dumps(status)
    assert read("setup-start", plan_file, identifier, archive) == status
    assert request(endpoint, "/v1/inference/setup") == (200, status)
    _, current = request(endpoint, "/v1/status")
    assert current["outboundAttempts"] == 0
    return {"matchingPlan": True, "idempotent": True, "outboundAttempts": 0, "status": status}


def run(helper, archive, hosted, legacy, cli):
    environment = {
        key: value for key, value in os.environ.items() if not key.startswith("CTXFAB_")
    }
    with TemporaryDirectory() as directory:
        data = Path(directory) / "data"
        with subprocess.Popen(
            [helper, data, "--app-session"],
            env=environment, stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
        ) as process:
            try:
                endpoint = wait_for_endpoint(data, process)
                assert endpoint["processId"] == process.pid
                rejection = check_install(endpoint, data, archive, hosted)
                capacity = check_capacity(endpoint, data, archive, hosted)
                weight = check_legacy_weight(endpoint, data, legacy) if legacy else None
                cli_result = check_packaged_cli(cli, endpoint, data, archive) if cli else None
            finally:
                process.stdin.close()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
                    raise
                assert process.returncode == 0, process.stderr.read().decode()
    guard = subprocess.run(
        [helper, "--guard-inference-worker", "generation",
         GENERATION_PROFILE, "/usr/bin/true", str(archive), "13333"],
        env=environment, input=b"1" * 48 + b"\n", capture_output=True, timeout=10, check=False,
    )
    assert guard.returncode != 0 and b"managed runtime verification failed" in guard.stderr, guard.stderr
    print(json.dumps({
        "authRequired": True,
        "relativePathRejected": True,
        "pinnedArchiveRejected": rejection,
        "noWorkerPublished": True,
        "directGuardianPathRejected": True,
        "compiledReleaseDownloadExposed": hosted,
        "capacity": capacity,
        "legacyWeight": weight,
        "packagedCli": cli_result,
    }, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("helper", type=Path)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--hosted", action="store_true")
    parser.add_argument("--legacy-weight", type=Path)
    parser.add_argument("--cli", type=Path)
    options = parser.parse_args()
    legacy = options.legacy_weight.resolve(strict=True) if options.legacy_weight else None
    cli = options.cli.resolve(strict=True) if options.cli else None
    run(options.helper.resolve(strict=True), options.archive.resolve(strict=True), options.hosted, legacy, cli)
