"""Stage a local, ad hoc-signed Qt app with its backend, CLI, and pinned runtime."""

import argparse
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


QT_PROJECT = Path(__file__).resolve().parents[1]
REPOSITORY = QT_PROJECT.parents[1]
APP_ID = "org.contextfabricator.qt"
APP_EXECUTABLE = "ctxfab_qt"
COCOA_PLUGIN = Path("platforms/libqcocoa.dylib")
CODESIGN = "/usr/bin/codesign"


def deploy_qt(app, sdk):
    cocoa = app / "Contents/PlugIns" / COCOA_PLUGIN
    cocoa.parent.mkdir(parents=True)
    shutil.copy2(sdk / "plugins" / COCOA_PLUGIN, cocoa)
    deploy = subprocess.run(
        [sdk / "bin/macdeployqt", app, f"-qmldir={QT_PROJECT / 'src'}",
         f"-libpath={sdk / 'lib'}", f"-executable={cocoa}", "-no-plugins",
         "-no-strip", "-codesign=-"],
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, check=False,
    )
    if deploy.stdout:
        print(deploy.stdout, end="")
    # macdeployqt can report unresolved libraries while returning status zero.
    if deploy.returncode or any(line.startswith("ERROR:") for line in deploy.stdout.splitlines()):
        raise SystemExit("Qt macOS deployment failed.")


def stage(app, sdk, backend, cli, destination):
    if destination.suffix != ".app" or destination.exists() or destination.is_symlink():
        raise SystemExit("Choose a new output path ending in .app.")
    with (app / "Contents/Info.plist").open("rb") as plist:
        info = plistlib.load(plist)
    if info.get("CFBundleIdentifier") != APP_ID or info.get("CFBundleExecutable") != APP_EXECUTABLE:
        raise SystemExit(f"Pass a Qt build with bundle identifier {APP_ID} and executable {APP_EXECUTABLE}.")
    for executable in (app / "Contents/MacOS" / APP_EXECUTABLE, sdk / "bin/macdeployqt", backend, cli):
        if not executable.is_file() or not os.access(executable, os.X_OK):
            raise SystemExit(f"Missing executable: {executable}")

    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".qt-staging-", dir=destination.parent) as temporary:
        staged = Path(temporary) / destination.name
        subprocess.run(["/usr/bin/ditto", app, staged], check=True)
        deploy_qt(staged, sdk)

        for source, name, identifier in (
            (backend, "ctxfab-server", "org.contextfabricator.backend"),
            (cli, "ctxfab", "org.contextfabricator.cli"),
        ):
            target = staged / "Contents/MacOS" / name
            shutil.copy2(source, target)
            subprocess.run(
                [CODESIGN, "--force", "--sign", "-", "--identifier", identifier, target], check=True
            )

        cache = Path(subprocess.check_output(
            [sys.executable, "-B", REPOSITORY / "quality/toolchain/cache-directory.py"],
            text=True,
        ).strip())
        subprocess.run(
            [sys.executable, "-B", REPOSITORY / "quality/release/embed_runtime.py",
             staged, cache / "runtime-source-cache"], check=True,
        )
        subprocess.run([CODESIGN, "--force", "--sign", "-", staged], check=True)
        subprocess.run([CODESIGN, "--verify", "--deep", "--strict", staged], check=True)
        staged.rename(destination)
    print(f"Staged macOS bundle: {destination}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", required=True, type=Path)
    parser.add_argument("--qt", required=True, type=Path, help="Qt macOS SDK root with QtSvg")
    parser.add_argument("--backend", required=True, type=Path)
    parser.add_argument("--cli", required=True, type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    stage(args.app.resolve(), args.qt.resolve(), args.backend.resolve(),
          args.cli.resolve(), args.destination.absolute())


if __name__ == "__main__":
    main()
