"""Exercise the real Qt window, XDG discovery, and backend shutdown on X11."""

import argparse
import http.client
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import tempfile
import time
from contextlib import contextmanager
from urllib.parse import urlsplit


INSTALLED_APP = Path("/usr/bin/context-fabricator-qt")
INSTALLED_BACKEND = Path("/usr/lib/context-fabricator-qt/bin/ctxfab-server")
INSTALLED_CLI = Path("/usr/bin/ctxfab")


def check_isolation():
    assert os.geteuid() != 0, "Exercise the package under an unprivileged account."
    interfaces = Path("/sys/class/net")
    up = {name for _, name in socket.if_nameindex()
          if int((interfaces / name / "flags").read_text(), 16) & 1}
    assert up == {"lo"}, "Run this scenario with only the loopback interface active."


@contextmanager
def xvfb(geometry="1024x768x24"):
    reader, writer = os.pipe()
    try:
        display = subprocess.Popen(
            ["Xvfb", "-displayfd", str(writer), "-screen", "0", geometry, "-nolisten", "tcp", "-ac"],
            pass_fds=(writer,),
        )
    except BaseException:
        os.close(reader)
        raise
    finally:
        os.close(writer)
    try:
        with os.fdopen(reader, "r", encoding="ascii") as reply:
            # A cold Xvfb must compile the XKB keymap before reporting readiness.
            ready, _, _ = select.select([reply], [], [], 15)
            if not ready:
                raise AssertionError("Xvfb did not publish a display within 15 seconds")
            display_number = reply.readline().strip()
        if not display_number.isdecimal():
            raise AssertionError(f"Xvfb closed its readiness pipe without a display (exit: {display.poll()})")
        yield ":" + display_number
    finally:
        display.terminate()
        display.wait(timeout=3)


def expect_ready(owner, cli, environment, log=None):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if owner.poll() is not None:
            message = log.read_text(errors="replace")[-6000:] if log else owner.stderr.read()
            raise AssertionError(message)
        status = subprocess.run(
            [cli, "status"], env=environment, capture_output=True, text=True, timeout=3
        )
        if status.returncode == 0:
            assert json.loads(status.stdout)["status"] == "ready"
            return
        time.sleep(0.05)
    raise AssertionError(f"CLI did not discover the Qt session: {status.stderr}")


def mapped_files(pid):
    lines = (Path("/proc") / str(pid) / "maps").read_text().splitlines()
    records = (line.split(maxsplit=5) for line in lines)
    return {Path(record[5]) for record in records if len(record) == 6 and record[5].startswith("/")}


def session_request(session, route, method="GET", body=None, *, expected=200):
    address = urlsplit(session["url"])
    assert address.scheme == "http" and address.hostname == "127.0.0.1" and address.port
    connection = http.client.HTTPConnection(address.hostname, address.port, timeout=3)
    try:
        headers = {"Authorization": f"Bearer {session['token']}"}
        if body is not None:
            headers["Content-Type"] = "application/json"
        connection.request(method, route, None if body is None else json.dumps(body), headers)
        response = connection.getresponse()
        assert response.status == expected, (method, route, response.status)
        return json.load(response)
    finally:
        connection.close()


def verify_installed(owner, backend, session, build, platform_plugin="libqxcb.so"):
    assert backend in mapped_files(session["processId"]), "The staged backend executable was not mapped."
    assert session_request(session, "/v1/runtime") == {
        "state": "ready", "engine": {"name": "llama.cpp", "build": build},
    }
    files = mapped_files(owner.pid)
    prefix = backend.parent.parent
    assert prefix / "bin/ctxfab_qt" in files, "The installed Qt executable was not mapped."
    frameworks = {path for path in files if path.name.startswith("libQt6")}
    assert frameworks and all(path.is_relative_to(prefix) for path in frameworks), frameworks
    assert any(path.name == platform_plugin and path.is_relative_to(prefix) for path in files)


def installed_environment():
    return {key: value for key, value in os.environ.items()
            if not key.startswith(("CTXFAB_", "QT_", "QML", "LD_"))}


def verify_window(app, backend, cli, runtime_build=None, *, inspect_session=None):
    with tempfile.TemporaryDirectory(prefix="ctxfab-window-") as isolated, xvfb() as display:
        profile = Path(isolated) / "context-fabricator"
        endpoint = profile / "endpoint.json"
        environment = installed_environment() if runtime_build else os.environ.copy()
        environment.update(
            DISPLAY=display,
            LANG="C.UTF-8",
            QT_QPA_PLATFORM="xcb",
            QT_QUICK_BACKEND="software",
            QT_FATAL_WARNINGS="1",
            XDG_CONFIG_HOME=isolated,
            XDG_DATA_HOME=isolated,
            XDG_RUNTIME_DIR=isolated,
        )
        environment.pop("CTXFAB_DATA_DIRECTORY", None)
        command = [app] if runtime_build else [app, "--backend", backend]
        log = Path(isolated) / "window.log"
        with log.open("w", encoding="utf-8") as output, subprocess.Popen(
            command, env=environment, stderr=output,
        ) as owner:
            try:
                expect_ready(owner, cli, environment, log)
                window_ids = subprocess.check_output(
                    ["xdotool", "search", "--onlyvisible", "--name", "^Context Fabricator$"],
                    env=environment, text=True, timeout=3,
                ).splitlines()
                assert len(window_ids) == 1
                session = json.loads(endpoint.read_text(encoding="utf8"))
                process_id = session["processId"]
                if runtime_build:
                    verify_installed(owner, backend, session, runtime_build)
                if inspect_session is not None:
                    inspect_session(session, profile)
                subprocess.run(
                    ["xdotool", "windowfocus", window_ids[0], "key", "--clearmodifiers", "ctrl+w"],
                    env=environment, check=True, timeout=3,
                )
                assert owner.wait(timeout=8) == 0, log.read_text(errors="replace")[-6000:]
                assert not endpoint.exists(), "Closing Qt left the backend session published"
                try:
                    os.kill(process_id, 0)
                except ProcessLookupError:
                    pass
                else:
                    raise AssertionError("The backend survived closing the last Linux window")
            finally:
                if owner.poll() is None:
                    owner.kill()
                    owner.wait(timeout=3)
    print("Qt opened its X11 window, the CLI discovered its XDG session, and Ctrl+W stopped both processes.")
    if runtime_build:
        print(f"The installed tree used its bundled Qt libraries and verified llama.cpp {runtime_build}.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    parser.add_argument("backend", type=Path)
    parser.add_argument("cli", type=Path)
    parser.add_argument("--bundled-runtime-build")
    args = parser.parse_args()
    verify_window(args.app.absolute(), args.backend.resolve(), args.cli.absolute(), args.bundled_runtime_build)
