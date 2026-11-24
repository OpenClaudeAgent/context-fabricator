"""Run the native Qt window scenarios against Rust and retain synthetic screenshots."""

import argparse
from contextlib import ExitStack, closing, contextmanager, nullcontext
import os
from pathlib import Path
import shlex
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time

from linux_wayland_smoke import weston
from linux_window_smoke import xvfb
from macos_bundle_smoke import LOOPBACK_SANDBOX


ROOT = Path(__file__).resolve().parents[3]
MODEL_FILE = "Qwen3-4B-Instruct-2507-Q4_K_M.gguf"
PIPELINE_SHOTS = (
    "00-overview-startup.png", "20-pipeline-retrieval.png", "21-pipeline-model-input.png",
    "22-pipeline-benchmarks.png", "23-pipeline-actions.png", "24-pipeline-results.png",
    "25-pipeline-loading.png", "26-pipeline-sources-compact-fr.png",
)
EXPECTED_SHOTS = (
    "00-overview-startup.png", "00b-overview-ready.png",
    "01-latest.png", "02-exact-trace.png", "03-pending.png", "04-abstained-and-history.png",
    "05-compact-fr-dark.png", "06-compact-fr-trace.png",
    "07-settings-wide.png", "08-settings-language.png", "09-settings-compact-fr-dark.png",
    "09b-model-library.png", "09c-model-library-wide.png",
    "10-connectors.png", "11-github-disconnected.png", "12-github-account.png",
    "13-github-picker.png", "14-github-url-entry.png", "15-github-filtered.png",
    "16-github-metadata-compact.png", "17-github-filtered-compact.png",
    "18-answers.png", "18b-answers-wide.png", "19-answer-history.png", *PIPELINE_SHOTS[1:],
)


def seed_corpus(database):
    with closing(sqlite3.connect(database, timeout=5)) as connection, connection:
        connection.executescript("""
            INSERT INTO collections(name) VALUES ('qt-window-fixture');
            INSERT INTO documents(collection, source, revision, kind, title, content)
            VALUES ('qt-window-fixture', 'src/outbound.rs', 'r-native', 'history',
                    'Retry cooldown', 'The retry cooldown is 47 seconds.');
        """)


def wait_for_file(process, path):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline and process.poll() is None:
        if path.is_file() and path.stat().st_size:
            return
        time.sleep(0.05)
    raise AssertionError(f"The fixture did not publish {path.name}.")


@contextmanager
def github_backend(scratch, backend, environment, log):
    directory = scratch / "github"
    directory.mkdir()
    (directory / "public-beta").touch()
    with subprocess.Popen(
        [sys.executable, "-B", ROOT / "quality/github/github_fixture.py", directory],
        env=environment, stdout=log, stderr=subprocess.STDOUT,
    ) as origin:
        try:
            port_file = directory / "port"
            wait_for_file(origin, port_file)
            port = int(port_file.read_text(encoding="ascii"))
            shim = scratch / "ctxfab-server-fixture"
            executable = shlex.quote(str(backend))
            shim.write_text(f'#!/bin/sh\nexec {executable} "$@" --github-fixture {port}\n',
                            encoding="utf-8")
            shim.chmod(0o700)
            yield shim
        finally:
            if origin.poll() is None:
                origin.terminate()
                try:
                    origin.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    origin.kill()
                    origin.wait(timeout=5)


@contextmanager
def launch_pipeline_backend(backend, profile, environment, log):
    with subprocess.Popen(
        [backend, profile, "--app-session"], env=environment, stdin=subprocess.PIPE,
        stdout=log, stderr=subprocess.STDOUT,
    ) as server:
        try:
            wait_for_file(server, profile / "endpoint.json")
            yield
        finally:
            server.stdin.close()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=5)


def run(app, backend, output, *, wayland=False, offscreen=False, pipeline_only=False,
        profile_dir=None):
    start = time.monotonic()
    linux = sys.platform == "linux"
    if not linux and sys.platform != "darwin":
        raise SystemExit("Window scenarios require Linux or macOS.")
    if wayland and not linux:
        raise SystemExit("Wayland window scenarios require Linux.")
    offscreen = offscreen or pipeline_only
    if wayland and offscreen:
        raise SystemExit("Choose Wayland or the offscreen platform.")
    screenshots = PIPELINE_SHOTS if pipeline_only else EXPECTED_SHOTS
    output.mkdir(parents=True, exist_ok=True)
    for filename in screenshots:
        (output / filename).unlink(missing_ok=True)
    geometry = "1600x1000x24" if wayland else "1024x768x24"
    display = xvfb(geometry) if linux and not offscreen else nullcontext()
    sandbox = () if linux else LOOPBACK_SANDBOX
    with (
        tempfile.TemporaryDirectory(prefix="ctxfab-qt-window-") as directory,
        display as number,
        ExitStack() as services,
    ):
        scratch = Path(directory)
        profile = scratch / "profile"
        profile.mkdir(mode=0o700)
        assets = profile / "inference"
        assets.mkdir(mode=0o700)
        worker = assets / "llama-server"
        shutil.copyfile(ROOT / "quality/answers/mock_worker.py", worker)
        worker.chmod(0o700)
        (assets / MODEL_FILE).touch()
        marker = scratch / "ready"
        endpoint = profile / "endpoint.json"
        environment = {
            key: value for key, value in os.environ.items()
            if not key.startswith(("CTXFAB_", "QT_", "QML", "LD_", "DYLD_", "LLVM_PROFILE_"))
        }
        environment.update(
            CTXFAB_WINDOW_EVIDENCE=str(output),
            CTXFAB_WINDOW_READY=str(marker),
            XDG_CONFIG_HOME=directory,
            XDG_DATA_HOME=directory,
            XDG_RUNTIME_DIR=directory,
            LLVM_PROFILE_FILE=str((profile_dir or scratch) / "window-%m-%p.profraw"),
            QT_FATAL_WARNINGS="1",
            QT_QPA_PLATFORM="xcb" if linux else "cocoa",
            OMP_THREAD_LIMIT="1",
        )
        if pipeline_only:
            environment["CTXFAB_WINDOW_PIPELINE_ONLY"] = "1"
        if offscreen:
            environment.update(QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software")
            if not linux:
                # Qt's macOS offscreen plugin reports its platform font-alias scan.
                environment["QT_LOGGING_RULES"] = "qt.qpa.fonts.warning=false"
        elif linux:
            environment.update(DISPLAY=number, QT_QUICK_BACKEND="software", LANG="C.UTF-8")
        if wayland:
            environment["XDG_SESSION_TYPE"] = "wayland"
            socket = services.enter_context(weston(
                scratch, environment, output / "weston.log", nested=True,
            ))
            environment.pop("DISPLAY")
            environment.update(WAYLAND_DISPLAY=socket, QT_QPA_PLATFORM="wayland")

        log = output / "window.log"
        try:
            with log.open("w", encoding="utf-8") as handle, ExitStack() as fixture:
                if pipeline_only:
                    fixture.enter_context(launch_pipeline_backend(backend, profile, environment, handle))
                    command = [app, "--endpoint-file", endpoint]
                else:
                    shim = fixture.enter_context(github_backend(scratch, backend, environment, handle))
                    command = [*sandbox, app, "--data-directory", profile, "--backend", shim]
                with subprocess.Popen(
                    command, env=environment, stdout=handle, stderr=subprocess.STDOUT,
                ) as process:
                    try:
                        wait_for_file(process, endpoint)
                        seed_corpus(profile / "context.sqlite3")
                        marker.touch()
                        assert process.wait(timeout=75) == 0, "The window scenarios failed."
                    finally:
                        if process.poll() is None:
                            process.kill()
                            process.wait(timeout=5)
            assert not endpoint.exists(), "Closing the window test left the session published."
        except (AssertionError, subprocess.TimeoutExpired) as error:
            raise SystemExit(f"{error}\n{log.read_text(encoding='utf-8', errors='replace')}") from error

    for filename in screenshots:
        with (output / filename).open("rb") as screenshot:
            assert screenshot.read(8) == b"\x89PNG\r\n\x1a\n", filename
    coverage = "pipeline flows" if pipeline_only else "eight interface languages"
    print(f"Verified {len(screenshots)} dark captures and {coverage} "
          f"in {time.monotonic() - start:.1f} s.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path, help="Built ctxfab_qt_window_tests executable")
    parser.add_argument("backend", type=Path, help="Local Debug ctxfab-server for the fixture model")
    parser.add_argument("output", type=Path, help="Output directory for the screenshots and test log")
    parser.add_argument("--wayland", action="store_true", help="Use a nested Weston compositor on Linux")
    parser.add_argument("--offscreen", action="store_true", help="Run the full window scenarios offscreen")
    parser.add_argument("--pipeline-only", action="store_true", help="Run the pipeline flow offscreen")
    args = parser.parse_args()
    if args.wayland and (args.offscreen or args.pipeline_only):
        parser.error("Choose Wayland or the offscreen platform.")
    run(args.app.resolve(strict=True), args.backend.resolve(strict=True), args.output.resolve(),
        wayland=args.wayland, offscreen=args.offscreen, pipeline_only=args.pipeline_only)
