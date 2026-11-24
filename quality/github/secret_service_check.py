"""Verify the Linux credential adapter with a private D-Bus session and GNOME Keyring."""

import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
from contextlib import contextmanager
from unittest.mock import patch

from package_check import NAMESPACE, PRIMARY, REPLACEMENT, backend, credential_ref, wait_until


PASSWORD = b"temporary test keyring\n"
SERVICE = "org.freedesktop.secrets"


def secret_method(method, *arguments):
    return subprocess.run(
        ["gdbus", "call", "--session", "--dest", SERVICE,
         "--object-path", "/org/freedesktop/secrets",
         "--method", f"org.freedesktop.Secret.Service.{method}", *arguments],
        capture_output=True, text=True, timeout=3, check=False,
    )


def owns_service():
    owner = subprocess.run(
        ["gdbus", "call", "--session", "--dest", "org.freedesktop.DBus",
         "--object-path", "/org/freedesktop/DBus", "--method",
         "org.freedesktop.DBus.NameHasOwner", SERVICE],
        capture_output=True, text=True, timeout=3, check=True,
    )
    return owner.stdout.strip() == "(true,)"


@contextmanager
def keyring(control):
    with subprocess.Popen(
        ["gnome-keyring-daemon", "--foreground", "--unlock", "--components=secrets",
         f"--control-directory={control}"],
        stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
    ) as daemon:
        try:
            daemon.stdin.write(PASSWORD)
            daemon.stdin.close()
            until = time.monotonic() + 8
            while time.monotonic() < until:
                if owns_service():
                    response = secret_method("ReadAlias", "default")
                    if response.returncode == 0 and "objectpath '/'" not in response.stdout:
                        yield response.stdout.split("'")[1]
                        return
                if daemon.poll() is not None:
                    break
                time.sleep(0.1)
            raise AssertionError("GNOME Keyring did not publish a persistent default collection.")
        finally:
            daemon.terminate()
            daemon.wait(timeout=5)


def stored_secret(reference):
    result = subprocess.run(
        ["secret-tool", "lookup", "application", NAMESPACE, "reference", reference],
        capture_output=True, timeout=4, check=False,
    )
    if result.returncode == 1 and not result.stdout and not result.stderr:
        return None
    assert result.returncode == 0, "Secret Service lookup failed"
    return result.stdout.rstrip(b"\n")


def check_unavailable(helper, profile, port, socket_path):
    with patch.dict(os.environ, {"DBUS_SESSION_BUS_ADDRESS": f"unix:path={socket_path}"}):
        with backend(helper, profile, port) as call:
            assert call("GET", "/v1/status")["status"] == "ready"
            failure = call("PUT", "/v1/github/credential",
                           {"revision": 0, "token": PRIMARY}, expected=503)
            assert failure["code"] == "github_keychain_unavailable"
            assert call("GET", "/v1/github")["credential"] == "unconfigured"


def check(helper, work):
    data = work / "data"
    control = work / "control"
    control.mkdir(mode=0o700)
    fixture = Path(__file__).with_name("github_fixture.py")
    with subprocess.Popen([sys.executable, "-B", fixture, work]) as origin:
        try:
            port = wait_until(work / "port").strip()
            check_unavailable(helper, work / "no-keyring", port, work / "absent-bus")
            blocked_socket = work / "blocked-bus"
            with socket.socket(socket.AF_UNIX) as stalled_bus:
                stalled_bus.bind(str(blocked_socket))
                stalled_bus.listen(8)
                check_unavailable(helper, work / "stalled-keyring", port, blocked_socket)
            with keyring(control) as collection:
                with backend(helper, data, port) as call:
                    state = call("PUT", "/v1/github/credential", {"revision": 0, "token": PRIMARY})
                    original = credential_ref(data)
                    assert state["credential"] == "connected"
                    assert original and stored_secret(original) == PRIMARY.encode()

                with backend(helper, data, port) as call:
                    assert call("GET", "/v1/github/repositories")["repositories"][0]["id"] == 41
                    state = call("PUT", "/v1/github/credential",
                                 {"revision": state["revision"], "token": REPLACEMENT})
                    rotated = credential_ref(data)
                    assert rotated and rotated != original and stored_secret(original) is None
                    assert stored_secret(rotated) == REPLACEMENT.encode()

                    locked = secret_method("Lock", f"['{collection}']")
                    assert locked.returncode == 0, locked.stderr
                    rejection = call("GET", "/v1/github/repositories", expected=503)
                    assert rejection["code"] == "github_keychain_unavailable"
                    assert "Secret Service" in rejection["message"]
                    assert call("GET", "/v1/github")["credential"] == "connected"

                    detached = call("DELETE", f"/v1/github/credential?revision={state['revision']}")
                    assert detached["credential"] == "unconfigured"
            with keyring(control):
                with backend(helper, data, port) as call:
                    assert call("GET", "/v1/github")["credential"] == "unconfigured"
                    assert stored_secret(rotated) is None, "Restart did not retry the locked deletion."
            for stored in data.glob("context.sqlite3*"):
                contents = stored.read_bytes()
                assert PRIMARY.encode() not in contents and REPLACEMENT.encode() not in contents
        finally:
            origin.terminate()
            origin.wait(timeout=5)
    print("Secret Service persisted, rotated, and deleted GitHub credentials across restarts.")
    print("Locked credentials returned HTTP 503 and deferred deletion until the collection reopened.")
    print("The backend stayed ready with an unavailable or stalled session bus.")
    print("HTTP and SQLite exposed no fixture token.")


def main():
    if sys.argv[1] == "--private-bus":
        check(Path(sys.argv[2]), Path(sys.argv[3]))
        return

    helper = Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="ctxfab-secret-service-") as temporary:
        work = Path(temporary)
        runtime = work / "xdg-runtime"
        runtime.mkdir(mode=0o700)
        env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), XDG_DATA_HOME=str(work / "xdg-data"))
        # The child daemon and the backend inherit only this private session bus.
        subprocess.run(
            ["dbus-run-session", "--", sys.executable, "-B", __file__, "--private-bus", helper, work],
            env=env, check=True,
        )


if __name__ == "__main__":
    main()
