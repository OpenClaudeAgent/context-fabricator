"""Verify a staged Cocoa bundle with the embedded CLI, backend, and llama.cpp."""

import argparse
import http.client
import json
import os
import subprocess
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit


LOOPBACK_SANDBOX = (
    "/usr/bin/sandbox-exec", "-p",
    '(version 1) (allow default) (deny network-outbound (remote ip "*:*")) '
    '(allow network-outbound (remote ip "localhost:*"))',
)


def runtime_status(session):
    url = urlsplit(session["url"])
    assert url.scheme == "http" and url.hostname == "127.0.0.1" and url.port, url
    connection = http.client.HTTPConnection(url.hostname, url.port, timeout=3)
    try:
        connection.request("GET", "/v1/runtime", headers={
            "Authorization": f"Bearer {session['token']}",
        })
        response = connection.getresponse()
        assert response.status == 200, response.status
        return json.load(response)
    finally:
        connection.close()


def wait_until_ready(owner, cli, endpoint, env, log):
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        if owner.poll() is not None:
            raise AssertionError(f"Packaged app exited with status {owner.returncode}. See {log}")
        if endpoint.is_file():
            status = subprocess.run([cli, "status"], env=env, capture_output=True, timeout=3)
            if status.returncode == 0:
                assert json.loads(status.stdout)["status"] == "ready"
                return json.loads(endpoint.read_text(encoding="utf-8"))
        time.sleep(0.05)
    raise AssertionError(f"The packaged backend did not become ready. See {log}")


def wait_for_shutdown(endpoint, pid):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            if not endpoint.exists():
                return
        time.sleep(0.05)
    raise AssertionError("The embedded backend did not clean up after the app exited.")


def verify_loaded_libraries(bundle, pid):
    mapped = subprocess.check_output(
        ["/usr/sbin/lsof", "-n", "-w", "-a", "-p", str(pid), "-d", "txt", "-Fn"],
        text=True, timeout=5,
    )
    libraries = [line[1:] for line in mapped.splitlines() if line.startswith("n")]
    for relative in ("Contents/Frameworks/QtCore.framework", "Contents/PlugIns/platforms/libqcocoa.dylib"):
        assert any(path.startswith(str(bundle / relative)) for path in libraries), relative
    for library in libraries:
        if ".framework/" in library and "/Qt" in library:
            assert library.startswith(str(bundle) + "/"), f"External Qt framework loaded: {library}"
    return "\n".join(libraries)


def verify_bundle(bundle, log, loopback_only):
    binaries = bundle / "Contents/MacOS"
    with tempfile.TemporaryDirectory(prefix="ctxfab-qt-bundle-") as temporary:
        profile = Path(temporary) / "profile"
        endpoint = profile / "endpoint.json"
        env = {key: value for key, value in os.environ.items()
               if not key.startswith(("CTXFAB_", "QT_", "QML", "DYLD_", "LLVM_PROFILE_"))}
        env["CTXFAB_DATA_DIRECTORY"] = str(profile)
        gui_env = dict(env, QT_QPA_PLATFORM="cocoa", QT_FATAL_WARNINGS="1",
                       LLVM_PROFILE_FILE=str(Path(temporary) / "bundle.profraw"))

        executable = (binaries / "ctxfab_qt",)
        command = LOOPBACK_SANDBOX + executable if loopback_only else executable
        with log.open("wb") as output, subprocess.Popen(command, env=gui_env, stderr=output) as owner:
            try:
                session = wait_until_ready(owner, binaries / "ctxfab", endpoint, env, log)
                pid = session["processId"]
                command = subprocess.check_output(
                    ["/bin/ps", "-ww", "-p", str(pid), "-o", "comm="], text=True, timeout=3,
                )
                assert Path(command.strip()) == binaries / "ctxfab-server", command
                runtime = runtime_status(session)
                assert runtime["state"] == "ready" and not runtime.get("canDownload"), runtime
                engine = runtime["engine"]
                assert engine["name"] == "llama.cpp" and engine["build"], engine
                libraries = verify_loaded_libraries(bundle, owner.pid)
            finally:
                if owner.poll() is None:
                    owner.kill()
                owner.wait(timeout=5)
            wait_for_shutdown(endpoint, pid)

    with log.open("a", encoding="utf-8") as output:
        output.write(f"\nMapped executable files:\n{libraries}\n")
    print(f"Bundled Cocoa, authenticated Rust session, and llama.cpp {engine['build']} verified.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--loopback-only", action="store_true", help="Restrict app and helpers to loopback IP connections",
    )
    parser.add_argument("bundle", type=Path)
    parser.add_argument("log", type=Path)
    args = parser.parse_args()
    verify_bundle(args.bundle.resolve(), args.log, args.loopback_only)


if __name__ == "__main__":
    main()
