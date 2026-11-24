"""Exercise the signed, bundled helper against an isolated GitHub origin and Keychain namespace."""

import json
import os
import sqlite3
import subprocess
import sys
import time
from contextlib import contextmanager
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request, urlopen

NAMESPACE = "org.contextfabricator.providers.github-fixture"
PRIMARY = "github_pat_fixture_primary"
REPLACEMENT = "github_pat_fixture_replacement"


def wait_until(path):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        try:
            return path.read_text(encoding="utf-8")
        except FileNotFoundError:
            time.sleep(0.05)
    raise RuntimeError(f"The fixture did not create {path.name}.")


@contextmanager
def backend(helper, directory, port):
    endpoint_file = directory / "endpoint.json"
    with subprocess.Popen(
        [helper, directory, "--github-keychain-fixture", port],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    ) as process:
        try:
            endpoint = json.loads(wait_until(endpoint_file))
            assert endpoint["processId"] == process.pid

            def call(method, route, body=None, expected=200):
                request = Request(
                    endpoint["url"] + route,
                    data=json.dumps(body).encode() if body is not None else None,
                    headers={
                        "Authorization": "Bearer " + endpoint["token"],
                        "Content-Type": "application/json",
                    },
                    method=method,
                )
                try:
                    response = urlopen(request, timeout=8)
                except HTTPError as failure:
                    response = failure
                with response:
                    data = response.read()
                    assert response.code == expected, (method, route, response.code)
                    assert PRIMARY.encode() not in data and REPLACEMENT.encode() not in data
                    return json.loads(data)

            yield call
        finally:
            process.terminate()
            process.wait(timeout=8)
            assert not endpoint_file.exists(), "Signed backend did not withdraw discovery."


def credential_ref(directory):
    path = directory / "context.sqlite3"
    if not path.exists():
        return None
    with sqlite3.connect(path) as database:
        return database.execute(
            "SELECT credential_ref FROM github_connection WHERE singleton = 1"
        ).fetchone()[0]


def keychain(action, reference):
    return subprocess.run(
        ["/usr/bin/security", action + "-generic-password", "-s", NAMESPACE, "-a", reference],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    ).returncode == 0


def selection(name, revision, code, history, actions, graph):
    return {
        "revision": revision,
        "repository": f"fixture/{name}",
        "intervalMinutes": 90,
        "features": {
            "currentCode": code, "history": history, "actions": actions, "codeGraph": graph,
        },
    }


def check(helper, work, upgraded_helper):
    data = work / "data"
    refs = set()
    for fixture in ("actions-fixture", "actions-dynamic-workflow", "collect-history"):
        (work / fixture).touch()
    script = Path(__file__).with_name("github_fixture.py")
    with subprocess.Popen([sys.executable, script, work], stdout=subprocess.DEVNULL) as fixture:
        try:
            port = wait_until(work / "port")
            with backend(helper, data, port) as call:
                configured = call("PUT", "/v1/github/credential", {"revision": 0, "token": PRIMARY})
                assert configured["credential"] == "connected"
                original = credential_ref(data)
                assert original and keychain("find", original)
                refs.add(original)
                first = call("GET", "/v1/github/repositories")
                assert first["nextPage"] == 2 and first["repositories"][0]["id"] == 41
                second = call("GET", "/v1/github/repositories?page=2")
                assert second["repositories"][0]["id"] == 42
                call("PUT", "/v1/github/repositories", selection("alpha", 1, True, True, True, False))
                configured = call("PUT", "/v1/github/repositories", selection("beta", 2, False, False, True, True))
                assert configured["repositories"][1]["contents"] == "available"
                assert configured["repositories"][1]["actions"] == "unavailable"

            with backend(upgraded_helper, data, port) as call:
                persisted = call("GET", "/v1/github")
                assert persisted == configured
                assert call("GET", "/v1/github/repositories")["repositories"][0]["id"] == 41
                actions = call("POST", "/v1/github/repositories/41/actions")
                assert actions["repository"] == "fixture/alpha"
                assert actions["workflows"][0]["definition"] == "name: CI\n"
                assert actions["workflows"][1]["path"] == "dynamic/dependabot/dependabot-updates"
                assert actions["workflows"][1]["definition"] is None
                assert actions["runs"][0]["commitProvenance"] == "verifiedRepositoryCommit"
                saved = call("GET", "/v1/github/repositories/41/actions")
                assert saved["snapshot"] == actions
                (work / "expire").touch()
                rejection = call("GET", "/v1/github/repositories", expected=409)
                assert rejection["code"] == "github_reconnect"
                expired = call("GET", "/v1/github")
                assert expired["credential"] == "reconnectRequired"
                assert all(not repo["verified"] for repo in expired["repositories"])
                replaced = call(
                    "PUT", "/v1/github/credential", {"revision": expired["revision"], "token": REPLACEMENT}
                )
                replacement = credential_ref(data)
                assert replacement and replacement != original
                refs.add(replacement)
                assert keychain("find", replacement) and not keychain("find", original)
                assert call("GET", "/v1/github/repositories")["repositories"][0]["id"] == 41
                ended = call("DELETE", f"/v1/github/credential?revision={replaced['revision']}")
                assert ended["credential"] == "unconfigured" and len(ended["repositories"]) == 2
                assert not keychain("find", replacement)

            for saved in data.glob("context.sqlite3*"):
                blob = saved.read_bytes()
                assert PRIMARY.encode() not in blob and REPLACEMENT.encode() not in blob
            transition = "signed helper replacement" if upgraded_helper != helper else "restart"
            print(f"Packaged signed helper kept both repository configurations across {transition}.")
            print("Keychain stored, reused, rotated and deleted only isolated fixture credentials.")
            print("Actions published a verified workflow and run after reopening the Keychain credential.")
            print("GitHub expiry and denied Actions access were visible; HTTP and SQLite contained no returned token.")
        finally:
            fixture.terminate()
            current = credential_ref(data)
            if current:
                refs.add(current)
            for reference in refs:
                if keychain("find", reference):
                    assert keychain("delete", reference), "Fixture Keychain cleanup failed."


if __name__ == "__main__":
    os.environ["LLVM_PROFILE_FILE"] = str(Path(sys.argv[2]) / "helper-%p.profraw")
    helper = Path(sys.argv[1])
    upgraded_helper = Path(sys.argv[3]) if len(sys.argv) > 3 else helper
    check(helper, Path(sys.argv[2]), upgraded_helper)
