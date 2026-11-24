"""Build and embed the pinned source runtime in a locally built desktop app."""

import fcntl
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

import runtime_package
import runtime_source


def recipe_key():
    digest = hashlib.sha256()
    root = Path(__file__).parent
    for path in [root / name for name in ("embed_runtime.py", "runtime_source.py", "runtime_package.py")] + sorted((root / "notices/runtime").iterdir()):
        digest.update(path.name.encode() + b"\0" + path.read_bytes())
    for command in (["xcodebuild", "-version"], ["xcrun", "clang", "--version"],
                    ["cmake", "--version"], ["ninja", "--version"], ["sw_vers", "-productVersion"]):
        digest.update(subprocess.check_output(command))
    return digest.hexdigest()


def verify_cached(package):
    bundle = package / runtime_package.BUNDLE_NAME
    record = json.loads((package / "cache-pin.json").read_text())
    if list(runtime_package.bundle_pin(bundle)) != record["bundlePin"]:
        raise ValueError("Cached runtime bundle does not match its recorded manifest.")
    runtime_source.verify_notices(bundle)
    executable = bundle / "Contents/MacOS/llama-server"
    libraries = bundle / "Contents/Frameworks"
    runtime_package.verify_dependencies(executable, libraries)
    runtime_package.verify_native_metadata(executable, libraries)
    runtime_package.command("/usr/bin/codesign", "--verify", "--deep", "--strict", bundle)
    detail = runtime_package.command("/usr/bin/codesign", "-dv", "--verbose=4", bundle).splitlines()
    if f"Identifier={runtime_package.BUNDLE_ID}" not in detail:
        raise ValueError("Cached runtime has an unexpected signing identifier.")
    if "build 12359, commit b1b770d90" not in runtime_package.command(executable, "--version"):
        raise ValueError("Cached runtime has an unexpected version.")


def main(bundle, cache):
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / f"llama-cpp-{runtime_source.COMMIT}-{runtime_source.SOURCE_ARCHIVE_SHA256}.tar.gz"
    # One process publishes verified cache entries at a time, including source downloads.
    with (cache / ".lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if archive.exists() and (archive.is_symlink() or runtime_source.digest(archive) != runtime_source.SOURCE_ARCHIVE_SHA256):
            raise ValueError(f"Cached runtime source failed SHA-256 verification: {archive}")
        if not archive.exists():
            url = f"https://codeload.github.com/ggml-org/llama.cpp/tar.gz/{runtime_source.COMMIT}"
            with tempfile.NamedTemporaryFile(dir=cache, prefix=".download-", delete=False) as temporary:
                temporary_path = Path(temporary.name)
                try:
                    with urllib.request.urlopen(url, timeout=120) as response:
                        shutil.copyfileobj(response, temporary)
                    temporary.close()
                    if runtime_source.digest(temporary_path) != runtime_source.SOURCE_ARCHIVE_SHA256:
                        raise ValueError("Downloaded runtime source failed SHA-256 verification.")
                    os.replace(temporary_path, archive)
                finally:
                    temporary_path.unlink(missing_ok=True)
        cached = cache / f"runtime-{runtime_source.COMMIT}-{recipe_key()}"
        if not cached.exists():
            with tempfile.TemporaryDirectory(prefix=".embedded-runtime-", dir=cache) as temporary:
                output = Path(temporary) / "package"
                runtime_package.package(archive, output, "-", None, api_source=True)
                (output / "cache-pin.json").write_text(json.dumps({"bundlePin": runtime_package.bundle_pin(
                    output / runtime_package.BUNDLE_NAME)}) + "\n")
                verify_cached(output)
                output.rename(cached)
        verify_cached(cached)
        helpers = bundle / "Contents/Helpers"
        helpers.mkdir(parents=True, exist_ok=True)
        destination = helpers / runtime_package.BUNDLE_NAME
        if destination.exists() or destination.is_symlink():
            if destination.is_symlink() or not destination.is_dir():
                raise ValueError(f"Unexpected existing runtime: {destination}")
            shutil.rmtree(destination)
        runtime_package.command("/usr/bin/ditto", cached / runtime_package.BUNDLE_NAME, destination)


if __name__ == "__main__":
    main(Path(sys.argv[1]), Path(sys.argv[2]))
