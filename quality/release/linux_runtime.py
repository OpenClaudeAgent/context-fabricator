"""Build the pinned Linux x86-64 CPU worker and print its runtime manifest pin."""

import argparse
import json
import os
import platform
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

import runtime_source


SYSTEM_LIBRARIES = {"ld-linux-x86-64.so.2", "libc.so.6", "libgcc_s.so.1", "libm.so.6", "libstdc++.so.6"}


def verify_worker(worker):
    metadata = subprocess.check_output(["readelf", "-h", "-d", "-n", worker], text=True,
                                       env=dict(os.environ, LC_ALL="C"))
    dependencies = set(re.findall(r"\(NEEDED\).*\[([^\]]+)\]", metadata))
    if (not re.search(r"Machine:\s+Advanced Micro Devices X86-64", metadata)
            or "x86 ISA needed: x86-64-baseline" not in metadata
            or not dependencies or dependencies - SYSTEM_LIBRARIES
            or re.search(r"\((?:RPATH|RUNPATH)\)", metadata)):
        raise ValueError(f"Unexpected Linux runtime loader metadata or dependencies: {dependencies}")
    flags = runtime_source.LINUX_BUILD_FLAGS
    version = subprocess.check_output([worker, "--version"], text=True, stderr=subprocess.STDOUT)
    if f"build {flags['LLAMA_BUILD_NUMBER']}, commit {flags['LLAMA_BUILD_COMMIT']}" not in version:
        raise ValueError(f"Unexpected Linux runtime version: {version.strip()}")


def stage_runtime(archive, destination):
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        raise ValueError("Build the Linux runtime on x86-64 Linux.")
    if destination.exists() or destination.is_symlink():
        raise ValueError("Choose a new Linux runtime destination.")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="ctxfab-llama-") as build:
        flags = runtime_source.LINUX_BUILD_FLAGS
        source, binaries = runtime_source.build_server(archive, Path(build), flags=flags)
        with tempfile.TemporaryDirectory(prefix=".runtime-", dir=destination.parent) as staging:
            runtime = Path(staging) / "runtime"
            runtime.mkdir(mode=0o755)
            worker = runtime / "llama-server"
            shutil.copy2(binaries / worker.name, worker)
            subprocess.run(["strip", "--strip-unneeded", worker], check=True)
            verify_worker(worker)
            shutil.copyfile(binaries / "LICENSE", runtime / "LICENSE-llama.cpp")
            runtime_source.stage_notices(source, binaries.parent, runtime, flags=flags)
            manifest = runtime / "manifest.json"
            manifest.write_text(json.dumps({
                "bytes": worker.stat().st_size,
                "sha256": runtime_source.digest(worker),
                "build": flags["LLAMA_BUILD_NUMBER"],
            }, indent=2) + "\n", encoding="utf-8")
            pin = runtime_source.digest(manifest)
            runtime.rename(destination)
    return pin


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    pin = stage_runtime(args.archive.resolve(), args.destination.absolute())
    print(f"CTXFAB_LINUX_RUNTIME_MANIFEST_SHA256={pin}")


if __name__ == "__main__":
    main()
