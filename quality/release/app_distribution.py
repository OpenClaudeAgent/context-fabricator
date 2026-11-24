"""Stage the built macOS app for Developer ID signing and ZIP distribution."""

import argparse
import json
import plistlib
import re
import stat
import subprocess
from pathlib import Path
from tempfile import TemporaryDirectory

from app_notices import verify_notices
from runtime_package import (BUNDLE_ID as RUNTIME_ID, BUNDLE_NAME as RUNTIME_NAME,
                             LIBRARY_NAME, bundle_pin, command, digest,
                             verify_dependencies, verify_native_metadata)
import runtime_source

APP_NAME = "Context Fabricator.app"
APP_ID = "org.contextfabricator.desktop"
MAIN = Path("Contents/MacOS/Context Fabricator")
HELPER = Path("Contents/MacOS/ctxfab-server")
CLI = Path("Contents/MacOS/ctxfab")
FRAMEWORKS = Path("Contents/Frameworks")
RUNTIME = Path("Contents/Helpers") / RUNTIME_NAME
RUNTIME_MAIN = RUNTIME / "Contents/MacOS/llama-server"
RUNTIME_FRAMEWORKS = RUNTIME / "Contents/Frameworks"
EXECUTABLES = {MAIN: APP_ID, HELPER: "org.contextfabricator.backend", CLI: "org.contextfabricator.cli"}
MACH_O = {bytes.fromhex(kind) for kind in (
    "feedface", "cefaedfe", "feedfacf", "cffaedfe", "cafebabe", "bebafeca", "cafebabf", "bfbafeca",
)}


def app_version(bundle):
    if bundle.name != APP_NAME or bundle.is_symlink() or not bundle.is_dir():
        raise ValueError("Unexpected macOS app bundle name or type.")
    with (bundle / "Contents/Info.plist").open("rb") as contents:
        info = plistlib.load(contents)
    if info.get("CFBundleIdentifier") != APP_ID or info.get("CFBundleExecutable") != MAIN.name:
        raise ValueError("Unexpected desktop app identity.")
    versions = [info.get(name, "") for name in ("CFBundleShortVersionString", "CFBundleVersion")]
    if not all(isinstance(value, str) and re.fullmatch(r"[0-9]+(?:\.[0-9]+){0,2}", value)
               for value in versions):
        raise ValueError("The Release app must declare a numeric marketing version and build number.")
    return versions


def native_members(bundle, *, allow_legacy_runtime=False):
    paths = set()
    for file in (bundle / "Contents").rglob("*"):
        if file.is_symlink() or not file.is_file():
            continue
        with file.open("rb") as contents:
            if contents.read(4) in MACH_O:
                paths.add(file.relative_to(bundle))
    if not EXECUTABLES.keys() <= paths:
        raise ValueError("The app, backend and CLI executables are required in the distribution bundle.")
    if RUNTIME_MAIN not in paths and not (allow_legacy_runtime and not (bundle / RUNTIME).exists()):
        raise ValueError("The integrated runtime executable is required.")
    companion_libraries = paths - EXECUTABLES.keys() - {RUNTIME_MAIN}
    if any(not ((path.parent == FRAMEWORKS and path.suffix == ".dylib") or
                (path.parent == RUNTIME_FRAMEWORKS and LIBRARY_NAME.fullmatch(path.name)))
           for path in companion_libraries):
        raise ValueError("Unexpected nested code layout in the desktop app.")
    return sorted(paths)


def verify_signature(item, identifier, team, executable):
    identity_flags = []
    if team is not None:
        requirement = (
            "anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists "
            "and certificate leaf[field.1.2.840.113635.100.6.1.13] exists "
            f'and certificate leaf[subject.OU] = "{team}"'
        )
        identity_flags = [f"-R={requirement}"]
    command("/usr/bin/codesign", "--verify", "--strict", "--all-architectures", *identity_flags, item)
    detail = command("/usr/bin/codesign", "-dv", "--verbose=4", item).splitlines()
    if identifier and f"Identifier={identifier}" not in detail:
        raise ValueError(f"Unexpected code-signing identifier for {item.name}.")
    if team is not None:
        if not any(line.startswith("Timestamp=") for line in detail):
            raise ValueError(f"Secure signing timestamp required for {item.name}.")
        if executable and not any(line.startswith("CodeDirectory ") and "runtime" in line for line in detail):
            raise ValueError(f"Hardened Runtime required for {item.name}.")


def verify_app(bundle, team=None, *, allow_legacy_runtime=False):
    app_version(bundle)
    command("/usr/bin/codesign", "--verify", "--deep", "--strict", "--all-architectures", bundle)
    runtime = bundle / RUNTIME
    if runtime.exists() or runtime.is_symlink() or not allow_legacy_runtime:
        if runtime.is_symlink() or not runtime.is_dir():
            raise ValueError("The integrated runtime bundle is required.")
        frameworks = bundle / RUNTIME_FRAMEWORKS
        if frameworks.is_symlink() or not frameworks.is_dir():
            raise ValueError("Invalid integrated runtime frameworks directory.")
        libraries = list(frameworks.iterdir())
        if not libraries or not any(path.is_file() and not path.is_symlink() for path in libraries):
            raise ValueError("The integrated runtime requires native libraries.")
        if any(not LIBRARY_NAME.fullmatch(path.name) or not path.is_file() for path in libraries):
            raise ValueError("Unexpected integrated runtime framework.")
        for path in runtime.rglob("*"):
            if path.is_symlink():
                target = path.readlink()
                if (path.parent != bundle / RUNTIME_FRAMEWORKS or len(target.parts) != 1
                        or not LIBRARY_NAME.fullmatch(str(target))
                        or path.resolve(strict=True).parent != path.parent.resolve(strict=True)
                        or not LIBRARY_NAME.fullmatch(path.resolve(strict=True).name)):
                    raise ValueError(f"Unexpected runtime symlink: {path}.")
        with (runtime / "Contents/Info.plist").open("rb") as contents:
            info = plistlib.load(contents)
        if info.get("CFBundleIdentifier") != RUNTIME_ID or info.get("CFBundleExecutable") != "llama-server":
            raise ValueError("Unexpected integrated runtime identity.")
        verify_dependencies(bundle / RUNTIME_MAIN, bundle / RUNTIME_FRAMEWORKS)
        verify_native_metadata(bundle / RUNTIME_MAIN, bundle / RUNTIME_FRAMEWORKS)
        runtime_source.verify_notices(runtime)
        verify_signature(runtime, RUNTIME_ID, team, True)
    for path in native_members(bundle, allow_legacy_runtime=allow_legacy_runtime):
        if command("xcrun", "lipo", "-archs", bundle / path) != "arm64":
            raise ValueError(f"Expected arm64 code in {path}.")
        identity = EXECUTABLES.get(path)
        if path != RUNTIME_MAIN:
            verify_signature(bundle if path == MAIN else bundle / path, identity, team, identity is not None)
    verify_notices(bundle / "Contents/Resources/ThirdPartyNotices")


def sign_app(bundle, identity, team):
    args = ["/usr/bin/codesign", "--force", "--sign", identity]
    args += ["--timestamp=none"] if team is None else ["--timestamp", "--options", "runtime"]
    for path in native_members(bundle):
        file = bundle / path
        architectures = command("xcrun", "lipo", "-archs", file).split()
        if architectures != ["arm64"]:
            if path.parent != FRAMEWORKS or "arm64" not in architectures:
                raise ValueError(f"Expected arm64 desktop executable in {path}.")
            thin = file.with_suffix(".arm64")
            command("xcrun", "lipo", file, "-thin", "arm64", "-output", thin)
            thin.chmod(stat.S_IMODE(file.stat().st_mode))
            thin.replace(file)
        if path in {MAIN, RUNTIME_MAIN}:
            continue
        identity_args = ["--identifier", EXECUTABLES[path]] if path in EXECUTABLES else [
            "--preserve-metadata=identifier",
        ]
        command(*args, *identity_args, file)
    command(*args, "--identifier", RUNTIME_ID, bundle / RUNTIME)
    command(*args, "--identifier", APP_ID, "--preserve-metadata=entitlements", bundle)


def verify_notarization(bundle):
    command("xcrun", "stapler", "validate", bundle)
    command("/usr/sbin/spctl", "--assess", "--type", "execute", bundle)


def restore_app(archive, directory, team=None, *, allow_legacy_runtime=False):
    command("/usr/bin/ditto", "-x", "-k", archive, directory)
    bundle = directory / APP_NAME
    if list(directory.iterdir()) != [bundle]:
        raise ValueError("The desktop archive must contain exactly one app bundle.")
    verify_app(bundle, team, allow_legacy_runtime=allow_legacy_runtime)
    return bundle


def create_release(source, output, identity, team, *, finalize):
    if output.exists():
        raise ValueError("The desktop package output already exists.")
    version, build = app_version(source)
    command("/usr/bin/codesign", "--verify", "--deep", "--strict", source)
    with TemporaryDirectory(prefix=".desktop-release-", dir=output.parent) as directory:
        root = Path(directory)
        artifact = root / "artifact"
        artifact.mkdir()
        bundle = artifact / APP_NAME
        command("/usr/bin/ditto", source, bundle)
        if not finalize:
            sign_app(bundle, identity, team)
        verify_app(bundle, team)
        if finalize:
            verify_notarization(bundle)
        label = "" if finalize else "-notarization" if team else "-local"
        archive = artifact / f"context-fabricator-{version}-{build}-macos-arm64{label}.zip"
        command("/usr/bin/ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", bundle, archive)
        restored = restore_app(archive, root / "roundtrip", team)
        if bundle_pin(bundle) != bundle_pin(restored):
            raise ValueError("The desktop archive changed the bundle layout or contents.")
        if finalize:
            verify_notarization(restored)
        manifest = {"version": version, "build": build, "archive": archive.name,
                    "archiveBytes": archive.stat().st_size, "archiveSHA256": digest(archive),
                    "nativeMembers": [path.as_posix() for path in native_members(restored)],
                    "notarized": finalize, "signature": "developerID" if team else "adHoc"}
        (artifact / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n",
                                                encoding="utf-8")
        artifact.rename(output)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("stage", "finalize", "verify"))
    parser.add_argument("bundle", type=Path)
    parser.add_argument("output", type=Path, nargs="?")
    parser.add_argument("--signing-identity", default="-")
    parser.add_argument("--team-id")
    options = parser.parse_args()
    if options.team_id and not re.fullmatch(r"[A-Z0-9]{10}", options.team_id):
        parser.error("Invalid Apple Team ID.")
    try:
        if options.action == "verify":
            if options.output is not None:
                parser.error("Verify only takes an app bundle.")
            verify_app(options.bundle, options.team_id)
            if options.team_id:
                verify_notarization(options.bundle)
        else:
            if options.output is None:
                parser.error("Provide a desktop package output directory.")
            if options.action == "stage" and ((options.signing_identity == "-") != (options.team_id is None)):
                parser.error("Provide both the Developer ID Application identity and Team ID.")
            if options.action == "finalize" and not options.team_id:
                parser.error("Finalization requires the release Apple Team ID.")
            create_release(options.bundle.resolve(strict=True), options.output.resolve(),
                           options.signing_identity, options.team_id, finalize=(options.action == "finalize"))
    except subprocess.CalledProcessError as error:
        parser.exit(error.returncode, error.output)
    except (OSError, ValueError) as error:
        parser.exit(1, str(error) + "\n")
