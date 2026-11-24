"""Open an installed Qt application in native Wayland and exercise its local session."""

import argparse
from contextlib import contextmanager
import json
import os
import re
import subprocess
import tempfile
import time
from pathlib import Path

from linux_window_smoke import expect_ready, installed_environment, session_request, verify_installed


SOCKET = "ctxfab-test-wayland"
BUFFER = re.compile(r"\bwl_surface[@#]\d+\.attach\(wl_buffer[@#]\d+")
APP_ID = re.compile(r'\bxdg_toplevel[@#]\d+\.set_app_id\("([^"]+)"\)')
HISTORY = "/v1/answer-history/settings"


def wait_for(process, condition, description, log=None):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline and process.poll() is None:
        if condition():
            return
        time.sleep(0.05)
    failure = log.read_text(errors="replace")[-6000:] if log else ""
    raise AssertionError(f"{description}\n{failure}")


def stop(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
            raise


@contextmanager
def weston(runtime, environment, log, *, nested=False):
    # The nested X11 backend supplies a seat so the compositor can activate the test window.
    display = ["--backend=x11-backend.so", "--shell=desktop-shell.so",
               "--width=1440", "--height=960"] if nested else [
                   "--backend=headless-backend.so", "--shell=kiosk-shell.so",
               ]
    command = ["weston", *display, "--renderer=pixman", f"--socket={SOCKET}",
               "--idle-time=0", "--no-config", f"--log={log}"]
    with subprocess.Popen(command, env=environment) as compositor:
        try:
            wait_for(compositor, (runtime / SOCKET).is_socket,
                     "Weston did not publish a socket.", log)
            yield SOCKET
        finally:
            stop(compositor)


def expect_shutdown(endpoint, pid):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if not endpoint.exists() and not (Path("/proc") / str(pid)).exists():
            return
        time.sleep(0.05)
    raise AssertionError("The backend survived termination of the Wayland client.")


def check_history(session, update, expected):
    if update is not None:
        settings = session_request(session, HISTORY)
        settings.update(enabled=True, retentionDays=update)
        assert session_request(session, HISTORY, "PUT", settings)["retentionDays"] == update
        expected = update
    if expected is not None:
        settings = session_request(session, HISTORY)
        assert settings["enabled"] and settings["retentionDays"] == expected, settings


def check_window(app, backend, cli, build, profile_root, update, expected, *, inspect_session=None):
    with tempfile.TemporaryDirectory(prefix="ctxfab-wayland-") as temporary:
        isolated = Path(temporary)
        endpoint = profile_root / "context-fabricator/endpoint.json"
        environment = installed_environment()
        environment.pop("DISPLAY", None)
        environment.update(LANG="C.UTF-8", XDG_RUNTIME_DIR=temporary, XDG_SESSION_TYPE="wayland")
        with weston(isolated, environment, isolated / "weston.log"):
            verify_client(app, backend, cli, build, profile_root, endpoint, environment,
                          isolated, update, expected, inspect_session)


def verify_client(app, backend, cli, build, profile_root, endpoint, environment, isolated, update,
                  expected, inspect_session):
    environment.update(WAYLAND_DISPLAY=SOCKET, QT_QPA_PLATFORM="wayland", QT_QUICK_BACKEND="software",
                       QT_FATAL_WARNINGS="1", WAYLAND_DEBUG="client", XDG_DATA_HOME=str(profile_root),
                       XDG_CONFIG_HOME=str(profile_root / "config"))
    log = isolated / "wayland.log"
    with log.open("wb") as output, subprocess.Popen([app], env=environment, stderr=output) as owner:
        try:
            wait_for(owner, lambda: BUFFER.search(log.read_text(errors="replace")),
                     "Qt did not attach a rendered buffer to the Wayland surface.", log)
            app_ids = APP_ID.findall(log.read_text(errors="replace"))
            assert "org.contextfabricator.qt" in app_ids, f"Unexpected Wayland desktop IDs: {app_ids}"
            expect_ready(owner, cli, environment, log)
            session = json.loads(endpoint.read_text(encoding="utf-8"))
            verify_installed(owner, backend, session, build, "libqwayland.so")
            check_history(session, update, expected)
            if inspect_session is not None:
                inspect_session(session)
        finally:
            stop(owner)
    expect_shutdown(endpoint, session["processId"])
    print(f"Wayland displayed Qt's buffer with its installed platform plugin and verified llama.cpp {build}.")
    print("The CLI found the Wayland-owned XDG profile, and the backend stopped after Qt terminated.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    parser.add_argument("backend", type=Path)
    parser.add_argument("cli", type=Path)
    parser.add_argument("--build", required=True)
    parser.add_argument("--profile-root", type=Path)
    retention = parser.add_mutually_exclusive_group()
    retention.add_argument("--set-retention-days", type=int)
    retention.add_argument("--expect-retention-days", type=int)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="ctxfab-wayland-profile-") as profile:
        root = (args.profile_root or Path(profile)).resolve(strict=True)
        check_window(args.app.absolute(), args.backend.resolve(strict=True), args.cli.absolute(),
                     args.build, root, args.set_retention_days, args.expect_retention_days)


if __name__ == "__main__":
    main()
