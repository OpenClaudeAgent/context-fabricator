"""Qualify trusted desktop release packages against disposable data."""

import argparse
import json
import os
import re
import subprocess
import time
from pathlib import Path
from tempfile import TemporaryDirectory
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import ProxyHandler, Request, build_opener

from app_distribution import (
    CLI,
    HELPER,
    MAIN,
    app_version,
    restore_app,
    verify_notarization,
)
from runtime_package import BUILD, digest


def request(endpoint, route, token=None, body=None, *, method=None):
    opener = build_opener(ProxyHandler({}))
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    action = Request(endpoint + route, data=json.dumps(body).encode() if body else None,
                     headers=headers, method=method or ("PUT" if body else "GET"))
    with opener.open(action, timeout=5) as response:
        return json.load(response)


def check_privacy(endpoint, token, restart):
    marker = f"ctxfabm11privatefixture{int(restart)}"
    enabled = request(endpoint, "/v1/telemetry", token, {"command": "enableDebug"})
    assert enabled["health"]["debugUntil"] > 0
    assert request(endpoint, "/v1/search?" + urlencode({"query": marker}), token) == []
    try:
        request(endpoint, "/v1/answers", token, {"query": marker}, method="POST")
    except HTTPError as error:
        with error:
            assert error.code == 503
            assert marker.encode() not in error.read()
    else:
        raise AssertionError("An empty installation answered the qualification query.")
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        diagnostics = request(endpoint, "/v1/telemetry", token)
        retained = json.dumps(diagnostics)
        assert marker not in retained and token not in retained
        if {"api_query", "api_answer"} <= {span["name"] for span in diagnostics["traces"]}:
            assert request(endpoint, "/v1/telemetry", token, {"command": "disableDebug"})[
                "health"]["debugUntil"] == 0
            return marker
        time.sleep(0.05)
    raise AssertionError("The optimized helper did not retain both diagnostic spans.")


def check_index(cli, environment, endpoint, token, retained):
    path = "/v1/retrieval/index"
    previous = request(endpoint, path, token)
    restart = retained is not None
    assert previous["enabled"] is not restart
    fields = ("enabled", "epoch", "profile")
    if retained is not None:
        assert all(retained[field] == previous[field] for field in fields)
    command = "enable-embedding-index" if restart else "delete-embedding-index"
    updated = json.loads(subprocess.check_output(
        [cli, command], env=environment, timeout=5,
    ))
    assert updated["enabled"] is restart and updated["epoch"] > previous["epoch"]
    if not restart:
        assert (updated["removedVectors"], updated["removedBytes"]) == (0, 0)
    saved = request(endpoint, path, token)
    assert all(updated[field] == saved[field] for field in fields)
    return saved


def run_session(bundle, data, environment, report, phase, retained=None, *, legacy=False):
    helper, cli = bundle / HELPER, bundle / CLI
    log_path = report / f"backend-{phase}.log"
    start = time.monotonic()
    with log_path.open("w", encoding="utf-8") as log, subprocess.Popen(
        [helper, data, "--app-session"], env=environment,
        stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT,
    ) as process:
        try:
            deadline = start + 10
            description = data / "endpoint.json"
            while not description.is_file() and time.monotonic() < deadline and process.poll() is None:
                time.sleep(0.05)
            connection = json.loads(description.read_text(encoding="utf-8"))
            ready_ms = round((time.monotonic() - start) * 1_000)
            assert connection["processId"] == process.pid
            endpoint, token = connection["url"], connection["token"]
            cli_environment = {**environment, "CTXFAB_DATA_DIRECTORY": str(data)}
            assert endpoint.startswith("http://127.0.0.1:")
            try:
                request(endpoint, "/v1/status")
            except HTTPError as error:
                assert error.code == 401
                error.close()
            else:
                raise AssertionError("The optimized helper returned status without a capability.")
            status = json.loads(subprocess.check_output(
                [cli, "status"], env=cli_environment, timeout=5,
            ))
            assert (status["status"], status["apiVersion"], status["connectedSources"]) == (
                "ready", 1, 0)
            runtime = json.loads(subprocess.check_output(
                [cli, "runtime-status"],
                env=cli_environment, timeout=5,
            ))
            assert runtime == request(endpoint, "/v1/runtime", token)
            plan = None
            if legacy:
                assert runtime["state"] in ("unavailable", "availableForInstall")
            else:
                assert runtime["state"] == "ready"
                assert runtime["engine"] == {"name": "llama.cpp", "build": BUILD[1:]}
                plan = request(endpoint, "/v1/inference/setup/plan", token, {
                    "answerProfile": "qwen3-4b-instruct-2507-q4-k-m-3605803b-v1",
                    "embedding": False, "runtimeSource": "hosted",
                }, method="POST")
                assert plan["runtimeRequired"] is False
                assert plan["canStart"] is True
                assert plan["maximumTransferBytes"] == 2_497_281_120
            storage = json.loads(subprocess.check_output(
                [cli, "storage-plan"], env=cli_environment, timeout=5,
            ))
            api_storage = request(endpoint, "/v1/inference/storage", token)
            # Free disk space can change between the two independent requests.
            assert storage["capacity"]["volumeFreeBytes"] > 0
            assert api_storage["capacity"]["volumeFreeBytes"] > 0
            assert storage["models"] == api_storage["models"]
            assert {k: v for k, v in storage["capacity"].items() if k != "volumeFreeBytes"} == {
                k: v for k, v in api_storage["capacity"].items() if k != "volumeFreeBytes"
            }
            settings = request(endpoint, "/v1/settings", token)
            if retained is None:
                assert settings["answerLanguage"] == "auto"
                response = request(endpoint, "/v1/settings", token, {
                    "revision": settings["revision"], "answerLanguage": "fr",
                    "diagnosticRetentionHours": 48,
                })
                assert response["revision"] == settings["revision"] + 1
                assert response["answerLanguage"] == "fr"
                assert response["diagnosticRetentionHours"] == 48
                settings = request(endpoint, "/v1/settings", token)
                assert settings == response
            else:
                assert all(settings[field] == retained["settings"][field] for field in (
                    "revision", "answerLanguage", "diagnosticRetentionHours",
                ))
            rss = int(subprocess.check_output(["/bin/ps", "-o", "rss=", "-p", str(process.pid)]))
            index = check_index(cli, cli_environment, endpoint, token,
                                retained and retained["embeddingIndex"])
            marker = check_privacy(endpoint, token, retained is not None)
            assert request(endpoint, "/v1/status", token)["outboundAttempts"] == 0
        finally:
            process.stdin.close()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
                raise
            assert process.returncode == 0
        assert not description.exists()
        log.flush()
        for file in (*data.glob("*.sqlite3*"), log_path):
            contents = file.read_bytes()
            assert marker.encode() not in contents and token.encode() not in contents, file.name
        return {"phase": phase, "readyMilliseconds": ready_ms, "residentKiB": rss,
                "status": status, "runtime": runtime, "inferenceSetupPlan": plan, "artifactStorage": storage,
                "settings": settings, "embeddingIndex": index, "queryAndCredentialRedacted": True}


def read_package(package, destination, team, *, allow_legacy_runtime=False):
    manifest = json.loads((package / "manifest.json").read_text(encoding="utf-8"))
    if not isinstance(manifest, dict):
        raise TypeError("The desktop package manifest must be an object.")
    name = manifest.get("archive")
    if not isinstance(name, str) or not re.fullmatch(
        r"context-fabricator-[0-9.]+-[0-9.]+-macos-arm64(?:-local|-notarization)?\.zip", name,
    ):
        raise ValueError("Unexpected desktop package archive name.")
    archive = package / name
    sha256 = digest(archive)
    if archive.stat().st_size != manifest.get("archiveBytes") or sha256 != manifest.get("archiveSHA256"):
        raise ValueError(f"Desktop archive does not match its manifest: {name}")
    bundle = restore_app(archive, destination, team, allow_legacy_runtime=allow_legacy_runtime)
    if team is not None:
        verify_notarization(bundle)
    version, build = app_version(bundle)
    if (version, build) != (manifest.get("version"), manifest.get("build")):
        raise ValueError(f"Extracted desktop version does not match its manifest: {name}")
    binaries = [bundle / path for path in (MAIN, HELPER, CLI)]
    return bundle, {
        "version": version, "build": build, "archive": name, "archiveSHA256": sha256,
        "binaries": [
            {"file": path.name, "size": path.stat().st_size, "sha256": digest(path)}
            for path in binaries
        ],
    }


def version_key(package):
    def components(value):
        parts = tuple(map(int, value.split(".")))
        return parts + (0,) * (3 - len(parts))

    return (components(package["version"]), components(package["build"]))


def run(package, report, previous=None, team=None):
    if not __debug__:
        raise ValueError("Release qualification requires Python assertions.")
    environment = {key: value for key, value in os.environ.items() if not key.startswith("CTXFAB_")}
    with TemporaryDirectory(prefix="ctxfab-release-package-") as temporary:
        root = Path(temporary)
        candidate, candidate_info = read_package(package, root / "candidate", team)
        original, original_info = candidate, candidate_info
        if previous is not None:
            original, original_info = read_package(previous, root / "previous", team, allow_legacy_runtime=True)
            if version_key(candidate_info) <= version_key(original_info):
                raise ValueError("The upgrade candidate must be newer than the previous package.")
        fixture_data = root / "rejected"
        rejected = subprocess.run(
            [candidate / HELPER, fixture_data, "--app-session", "--github-fixture", "1"],
            env=environment, capture_output=True, timeout=5, check=False)
        assert rejected.returncode != 0 and b"usage:" in rejected.stderr
        assert not fixture_data.exists()
        data = root / "production"
        first = run_session(original, data, environment, report, "initial",
                            legacy=not (original / "Contents/Helpers/Context Fabricator Runtime.app").exists())
        mode = "upgrade" if previous is not None else "restart"
        second = run_session(candidate, data, environment, report, mode, first)
    with (report / "results.json").open("w", encoding="utf-8") as output:
        json.dump({"mode": mode, "candidate": candidate_info,
                   "previous": original_info if previous is not None else None,
                   "binaries": candidate_info["binaries"], "sessions": [first, second],
                   "debugFixtureRejected": True},
                  output, indent=2)
        output.write("\n")
    print(f"Optimized Apple Silicon helper and CLI passed local startup, authorization and {mode}.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path, help="Trusted package directory produced by app_distribution.py")
    parser.add_argument("report", type=Path, help="Existing directory for the qualification report")
    parser.add_argument("--previous", type=Path, help="Trusted older package directory for an upgrade test")
    parser.add_argument("--team-id", help="Require Developer ID from this Apple team and notarization on both packages")
    options = parser.parse_args()
    if options.team_id is not None and not re.fullmatch(r"[A-Z0-9]{10}", options.team_id):
        parser.error("Invalid Apple Team ID.")
    try:
        run(options.package, options.report, options.previous, options.team_id)
    except subprocess.CalledProcessError as error:
        parser.exit(1, f"{error}\n{error.output or ''}\n")
    except (OSError, TypeError, ValueError) as error:
        parser.exit(1, f"{error}\n")
