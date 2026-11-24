"""Exercise the packaged Linux runtime with the real local backend and API."""

import argparse
import json
import os
import shutil
import socket
import subprocess
import tempfile
import time
from pathlib import Path

from runtime_fixture import GENERATION_PROFILE, request, wait_for_endpoint


def guard_command(helper, worker, model, port):
    return [helper, "--guard-inference-worker", "generation", GENERATION_PROFILE, worker, model, str(port)]


def check_ready(helper, profile, build, environment, weights=None):
    with subprocess.Popen([helper, profile, "--app-session"], stdin=subprocess.PIPE,
                          stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env=environment) as server:
        try:
            endpoint = wait_for_endpoint(profile, server)
            assert request(endpoint, "/v1/runtime", authenticated=False)[0] == 401
            assert request(endpoint, "/v1/runtime") == (
                200, {"state": "ready", "engine": {"name": "llama.cpp", "build": build}},
            ), "The verified runtime should be available over the authenticated API."
            assert request(endpoint, "/v1/inference/storage")[0] == 200
            code, plan = request(endpoint, "/v1/inference/setup/plan",
                                 {"embedding": False, "runtimeSource": "hosted"})
            assert code == 200 and plan["canStart"] and not plan["runtimeRequired"], plan
            if weights:
                model = adopt_weights(endpoint, profile, weights)
                print("The backend verified the local catalog weights.")
                check_generation(helper, helper.parent / "runtime/llama-server", model, environment)
                assert request(endpoint, "/v1/status")[1]["outboundAttempts"] == 0
        finally:
            server.stdin.close()
            server.wait(timeout=10)
            assert server.returncode == 0, server.stderr.read().decode()
    assert not (profile / "endpoint.json").exists()


def adopt_weights(endpoint, profile, weights):
    generation_file = "Qwen3-4B-Instruct-2507-Q4_K_M.gguf"
    model = profile / "inference" / generation_file
    model.parent.mkdir(mode=0o700, exist_ok=True)
    shutil.copyfile(weights, model)
    catalog = "/v1/model-downloads"
    assert request(endpoint, catalog, {"profile": GENERATION_PROFILE})[0] == 202
    deadline = time.monotonic() + 300
    installed = None
    while time.monotonic() < deadline:
        code, models = request(endpoint, catalog)
        assert code == 200
        installed = next(entry for entry in models if entry["profile"] == GENERATION_PROFILE)
        if installed["state"] == "installed":
            return model
        if installed["state"] == "failed":
            raise AssertionError(installed)
        time.sleep(0.2)
    raise AssertionError(f"The local model was not verified within the deadline: {installed}")


def check_generation(helper, worker, model, environment):
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    token = "1" * 48
    endpoint = {"url": f"http://127.0.0.1:{port}", "token": token}
    with subprocess.Popen(guard_command(helper, worker, model, port), stdin=subprocess.PIPE,
                          stderr=subprocess.PIPE, env=environment) as guardian:
        guardian.stdin.write((token + "\n").encode())
        guardian.stdin.flush()
        try:
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline and guardian.poll() is None:
                try:
                    if request(endpoint, "/v1/models")[0] == 200:
                        break
                except OSError:
                    pass
                time.sleep(0.2)
            else:
                raise AssertionError("The guardian did not start the verified worker.")
            assert request(endpoint, "/v1/models", authenticated=False)[0] == 401
            code, result = request(endpoint, "/v1/chat/completions", {
                "model": GENERATION_PROFILE,
                "messages": [{"role": "user", "content": "Reply with the number four."}],
                "max_tokens": 5,
                "temperature": 0,
            }, timeout=180)
            assert code == 200 and result["usage"]["completion_tokens"] > 0, result
        finally:
            guardian.stdin.close()
            try:
                guardian.wait(timeout=10)
            except subprocess.TimeoutExpired:
                guardian.kill()
                guardian.wait(timeout=5)
                raise
        assert guardian.returncode == 0, guardian.stderr.read().decode()
    with socket.socket() as probe:
        assert probe.connect_ex(("127.0.0.1", port)) != 0, "The worker survived guardian shutdown."
    print("The verified CPU worker generated tokens and exited when its control pipe closed.")


def check_rejected(helper, profile, environment):
    server = subprocess.run([helper, profile, "--app-session"], input=b"", capture_output=True,
                            timeout=10, env=environment)
    assert server.returncode != 0, "The backend accepted a modified runtime."
    assert b"bundled_runtime_rejected" in server.stderr, server.stderr.decode()
    assert not (profile / "endpoint.json").exists()


def check(helper, runtime, build, weights):
    environment = {name: value for name, value in os.environ.items() if not name.startswith("CTXFAB_")}
    with tempfile.TemporaryDirectory(prefix="ctxfab-linux-runtime-") as temporary:
        root = Path(temporary)
        staged = root / "ctxfab-server"
        shutil.copy2(helper, staged)
        shutil.copytree(runtime, root / "runtime", symlinks=True)
        profile = root / "profile"
        profile.mkdir(mode=0o700)
        check_ready(staged, profile, build, environment, weights)

        worker = root / "runtime/llama-server"
        unverified = profile / "inference/wrong.gguf"
        unverified.parent.mkdir(mode=0o700, exist_ok=True)
        unverified.write_bytes(b"unverified weights")
        for rejected_worker in [Path("/usr/bin/true"), worker]:
            rejection = subprocess.run(guard_command(staged, rejected_worker, unverified, 13333),
                                       input=b"1" * 48 + b"\n", capture_output=True, timeout=10,
                                       env=environment)
            assert rejection.returncode != 0 and b"verification failed" in rejection.stderr
        with worker.open("r+b") as file:
            file.seek(-1, os.SEEK_END)
            original = file.read(1)
            file.seek(-1, os.SEEK_END)
            file.write(bytes([original[0] ^ 1]))
        check_rejected(staged, profile, environment)
        shutil.copy2(runtime / worker.name, worker)

        manifest = root / "runtime/manifest.json"
        previous = manifest.read_bytes()
        record = json.loads(previous)
        record["build"] = "0"
        manifest.write_text(json.dumps(record), encoding="utf-8")
        check_rejected(staged, profile, environment)
        manifest.write_bytes(previous)

        worker.unlink()
        worker.symlink_to(runtime / worker.name)
        check_rejected(staged, profile, environment)
        worker.unlink()
        shutil.copy2(runtime / worker.name, worker)
        check_ready(staged, profile, build, environment)
    print(f"Linux llama.cpp {build} verified through the API. Modified and redirected artifacts rejected.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("backend", type=Path)
    parser.add_argument("runtime", type=Path)
    parser.add_argument("--build", required=True)
    parser.add_argument("--weights", type=Path, help="Local copy of the catalog-pinned Qwen3 Q4_K_M weights")
    args = parser.parse_args()
    check(args.backend.resolve(), args.runtime.resolve(), args.build, args.weights)


if __name__ == "__main__":
    main()
