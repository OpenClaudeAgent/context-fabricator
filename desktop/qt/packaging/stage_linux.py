"""Assemble an Ubuntu amd64 package from the Qt install tree and pinned release binaries."""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from hashlib import file_digest, md5
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "quality/release"))

import app_notices
import linux_runtime
import runtime_source


PACKAGE = "context-fabricator-qt"
TARGET = "x86_64-unknown-linux-gnu"
PREFIX = Path("usr/lib") / PACKAGE
QT_VERSION = "6.12.0"
ICU_NOTICE_PIN = "f3005e195ff74d8812cc1f182a1c446fab678d70a10e3dada497585befee5416"
ICU_NOTICE_URL = "https://raw.githubusercontent.com/unicode-org/icu/release-73-2/icu4c/LICENSE"
ELF = b"\x7fELF"
SONAME = re.compile(r"\(SONAME\).*\[([^\]]+?)\.so\.([^\]]+)\]")


def output(*args, cwd=None):
    environment = dict(os.environ, LC_ALL="C.UTF-8")
    environment.pop("LD_LIBRARY_PATH", None)
    return subprocess.check_output(args, cwd=cwd, env=environment, text=True).strip()


def verify_runtime_pin(runtime, backend):
    linux_runtime.verify_worker(runtime / "llama-server")
    runtime_source.verify_runtime_notices(runtime, runtime_source.LINUX_BUILD_FLAGS)
    manifest = runtime / "manifest.json"
    record = json.loads(manifest.read_text(encoding="utf-8"))
    worker = runtime / "llama-server"
    if record != {"bytes": worker.stat().st_size, "sha256": runtime_source.digest(worker),
                  "build": runtime_source.LINUX_BUILD_FLAGS["LLAMA_BUILD_NUMBER"]}:
        raise ValueError("The runtime manifest does not identify the staged worker.")
    pin = runtime_source.digest(manifest)
    if pin.encode("ascii") not in backend.read_bytes():
        raise ValueError("Build the backend with this runtime manifest SHA-256 before packaging.")
    return pin


def stage_app(package, qt, backend, cli, runtime):
    app = package / PREFIX
    binaries = app / "bin"
    binaries.mkdir(parents=True)
    for source, name in ((qt / "bin/ctxfab_qt", "ctxfab_qt"), (backend, "ctxfab-server"), (cli, "ctxfab")):
        shutil.copyfile(source, binaries / name)
        (binaries / name).chmod(0o755)
    shutil.copyfile(qt / "bin/qt.conf", binaries / "qt.conf")
    shutil.copytree(runtime, binaries / "runtime", symlinks=True)
    for directory in ("lib", "plugins", "qml"):
        shutil.copytree(qt / directory, app / directory, symlinks=True)
    for path in app.rglob("*"):
        if path.is_symlink() and (path.readlink().is_absolute() or not path.resolve(strict=True).is_relative_to(app)):
            raise ValueError(f"The application tree contains an external symlink: {path.relative_to(app)}")
    for plugin in ("libqxcb.so", "libqwayland.so"):
        if not (app / "plugins/platforms" / plugin).is_file():
            raise ValueError(f"Missing deployed Qt platform plugin: {plugin}")

    public_bin = package / "usr/bin"
    public_bin.mkdir(parents=True)
    launcher = public_bin / PACKAGE
    launcher.write_text(f'#!/bin/sh\nexec /{PREFIX}/bin/ctxfab_qt "$@"\n', encoding="utf-8")
    launcher.chmod(0o755)
    (public_bin / "ctxfab").symlink_to(Path("..") / "lib" / PACKAGE / "bin/ctxfab")
    desktop = package / "usr/share/applications/org.contextfabricator.qt.desktop"
    desktop.parent.mkdir(parents=True)
    shutil.copyfile(Path(__file__).with_name(desktop.name), desktop)
    subprocess.run(["desktop-file-validate", desktop], check=True)
    icons = ROOT / "desktop/macos/App/Assets.xcassets/AppIcon.appiconset"
    for size in (32, 128, 256):
        icon = package / f"usr/share/icons/hicolor/{size}x{size}/apps/org.contextfabricator.qt.png"
        icon.parent.mkdir(parents=True)
        shutil.copyfile(icons / f"icon_{size}x{size}@1x.png", icon)
    return app


def elf_files(root):
    for path in sorted(root.rglob("*")):
        if path.is_file() and not path.is_symlink():
            with path.open("rb") as file:
                if file.read(4) == ELF:
                    yield path


def dependencies(work, app, debian):
    binaries = list(elf_files(app))
    for name in ("ctxfab_qt", "ctxfab-server", "ctxfab"):
        binary = app / "bin" / name
        if binary not in binaries or "Advanced Micro Devices X86-64" not in output("readelf", "-h", binary):
            raise ValueError(f"Expected an x86-64 ELF executable: {name}")
    libraries = app / "lib"
    private = set()
    for file in (path for path in binaries if path.parent == libraries):
        match = SONAME.search(output("readelf", "-d", file))
        if match:
            private.add(f"{match[1]} {match[2]} {PACKAGE}\n")
    (debian / "shlibs.local").write_text("".join(sorted(private)), encoding="utf-8")
    (debian / "control").write_text(f"Source: {PACKAGE}\n\nPackage: {PACKAGE}\nArchitecture: amd64\n",
                                   encoding="utf-8")
    search = ["dpkg-shlibdeps", "-O", f"-x{PACKAGE}", f"-l{libraries}"]
    result = output(*search, *(f"-e{file}" for file in binaries), cwd=work)
    label = "shlibs:Depends="
    if not result.startswith(label) or not result[len(label):] or "\n" in result:
        raise ValueError(f"Invalid shared library dependencies: {result!r}")
    return result[len(label):] + ", fonts-dejavu-core"


def stage_notices(package, qt_sdk, icu_notice, cargo_source):
    docs = package / "usr/share/doc" / PACKAGE
    docs.mkdir(parents=True)
    shutil.copyfile(ROOT / "LICENSE", docs / "copyright")
    for license_name in ("GPL-3", "LGPL-3"):
        shutil.copyfile(Path("/usr/share/common-licenses") / license_name, docs / f"{license_name}.txt")
    if runtime_source.digest(icu_notice) != ICU_NOTICE_PIN:
        raise ValueError("ICU 73.2's license file does not match its pin.")
    shutil.copyfile(icu_notice, docs / "ICU-73.2.txt")
    qt_notices = docs / "qt"
    qt_notices.mkdir()
    sources = [f"Qt {QT_VERSION} is linked dynamically. Qt SDK SBOMs are included here.\n",
               "Qt's shared libraries and plugins can be replaced in a private extracted package tree.\n"]
    for module in ("qtbase", "qtdeclarative", "qtwayland"):
        file = f"{module}-{QT_VERSION}.spdx.json"
        shutil.copyfile(qt_sdk / "sbom" / file, qt_notices / file)
        sources.append(f"{module} sources: https://code.qt.io/cgit/qt/{module}.git/tag/?h=v{QT_VERSION}\n")
    sources.append(f"ICU 73.2 license: {ICU_NOTICE_URL}\n")
    (qt_notices / "README.txt").write_text("".join(sources), encoding="utf-8")
    rust_notices = docs / "rust-notices"
    entries = app_notices.rust_components(rust_notices, TARGET, cargo_source)
    (rust_notices / "manifest.json").write_text(json.dumps(entries, indent=2) + "\n", encoding="utf-8")


def payload_metadata(package, controls):
    installed_size = 0
    with (controls / "md5sums").open("w", encoding="utf-8") as inventory:
        for path in sorted((package / "usr").rglob("*")):
            if path.is_file() and not path.is_symlink():
                with path.open("rb") as file:
                    digest = file_digest(file, lambda: md5(usedforsecurity=False)).hexdigest()
                inventory.write(f"{digest}  {path.relative_to(package)}\n")
                installed_size += (path.stat().st_size + 1023) // 1024
    return installed_size


def create_deb(args):
    if output("dpkg", "--print-architecture") != "amd64":
        raise ValueError("Build the package on amd64 Debian or Ubuntu.")
    subprocess.run(["dpkg", "--validate-version", args.version], check=True)
    if re.fullmatch(r"[^<>\r\n]+ <[^<>\s]+@[^<>\s]+>", args.maintainer) is None:
        raise ValueError("Specify the maintainer as a name and email on one line.")
    destination = args.destination / f"{PACKAGE}_{args.version}_amd64.deb"
    destination.parent.mkdir(parents=True, exist_ok=True)
    if destination.exists() or destination.is_symlink():
        raise ValueError(f"Choose a new package output: {destination}")
    pin = verify_runtime_pin(args.runtime, args.backend)
    with tempfile.TemporaryDirectory(prefix="ctxfab-deb-") as temporary:
        work = Path(temporary)
        debian = work / "debian"
        package = debian / PACKAGE
        app = stage_app(package, args.qt_tree, args.backend, args.cli, args.runtime)
        stage_notices(package, args.qt_sdk, args.icu_license, args.cargo_source)
        controls = package / "DEBIAN"
        controls.mkdir()
        depends = dependencies(work, app, debian)
        installed_size = payload_metadata(package, controls)
        (controls / "control").write_text(
            f"Package: {PACKAGE}\nVersion: {args.version}\nArchitecture: amd64\n"
            f"Maintainer: {args.maintainer}\nSection: devel\nPriority: optional\nInstalled-Size: {installed_size}\n"
            f"Depends: {depends}\nRecommends: gnome-keyring, fonts-noto-cjk\n"
            "Homepage: https://github.com/OpenClaudeAgent/context-fabricator\n"
            "Description: Local engineering search and inference desktop\n"
            " Context Fabricator's Qt desktop, local backend, CLI and CPU inference runtime.\n",
            encoding="utf-8",
        )
        with tempfile.TemporaryDirectory(prefix=".ctxfab-deb-", dir=destination.parent) as publishing:
            packed = Path(publishing) / destination.name
            subprocess.run(["dpkg-deb", "--root-owner-group", "--threads-max=1", "-Zzstd", "-z6", "-b", package, packed],
                           check=True)
            os.link(packed, destination)
    print(f"Created {destination}\nRuntime manifest SHA-256 {pin}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("qt-tree", "qt-sdk", "backend", "cli", "runtime", "icu-license"):
        parser.add_argument(f"--{name}", type=lambda value: Path(value).resolve(strict=True), required=True)
    parser.add_argument("--cargo-source", type=Path, default=ROOT)
    parser.add_argument("--version", required=True)
    parser.add_argument("--maintainer", default="Context Fabricator Contributors <context-fabricator@localhost>")
    parser.add_argument("destination", type=Path)
    create_deb(parser.parse_args())


if __name__ == "__main__":
    main()
