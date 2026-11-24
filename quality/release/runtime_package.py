"""Package the pinned Apple Silicon runtime with the platform's signing and archive tools.

Use ``finalize BUNDLE OUTPUT --team-id TEAM`` after notarization and stapling to
create the release ZIP and sourceable ``release-pins.env`` from its final bytes.
"""

import argparse
import hashlib
import json
import os
import plistlib
import re
import shutil
import subprocess
import sys
import tarfile
import zipfile
from pathlib import Path
from tempfile import TemporaryDirectory

import runtime_source

BUILD = "b12359"
UPSTREAM_SIZE = 16_907_811
UPSTREAM_SHA256 = "82b00abc740160e1b9980e6cc79c117345fa34af058c80e6c0a69aa8ddccde75"
BUNDLE_NAME = "Context Fabricator Runtime.app"
BUNDLE_ID = "org.contextfabricator.runtime.llama"
LIBRARY_NAME = re.compile(r"lib[a-z0-9.-]+\.dylib")


def command(*args):
    return subprocess.check_output(args, stderr=subprocess.STDOUT, text=True).strip()


digest = runtime_source.digest


def unpack(archive, directory):
    if archive.stat().st_size != UPSTREAM_SIZE or digest(archive) != UPSTREAM_SHA256:
        raise ValueError("The archive does not match the pinned upstream runtime.")
    with tarfile.open(archive) as package:
        package.extractall(directory, filter="data")
    root = directory / f"llama-{BUILD}"
    return root.resolve(strict=True)


def install_files(source, bundle):
    contents = bundle / "Contents"
    executable = contents / "MacOS/llama-server"
    frameworks = contents / "Frameworks"
    resources = contents / "Resources"
    for directory in (executable.parent, frameworks, resources):
        directory.mkdir(parents=True)

    shutil.copy2(source / "llama-server", executable)
    shutil.copyfile(source / "LICENSE", resources / "LICENSE-llama.cpp")
    libraries = sorted(path for path in source.iterdir() if LIBRARY_NAME.fullmatch(path.name))
    for library in libraries:
        target = library.resolve(strict=True)
        if target.parent != source or not target.is_file() or not LIBRARY_NAME.fullmatch(target.name):
            raise ValueError(f"Unsafe runtime library target for {library.name}.")
        shutil.copy2(library, frameworks / library.name, follow_symlinks=False)

    metadata = {
        "CFBundleExecutable": executable.name,
        "CFBundleIdentifier": BUNDLE_ID,
        "CFBundleName": "Context Fabricator Runtime",
        "CFBundlePackageType": "APPL",
        "CFBundleVersion": BUILD[1:],
        "CFBundleShortVersionString": "1.0.0",
        "LSMinimumSystemVersion": "14.0",
        "LSBackgroundOnly": True,
    }
    with (contents / "Info.plist").open("wb") as info:
        plistlib.dump(metadata, info)
    command("xcrun", "install_name_tool", "-add_rpath", "@executable_path/../Frameworks", executable)
    return executable, frameworks


def verify_dependencies(executable, libraries):
    for binary in [executable, *(path for path in sorted(libraries.iterdir()) if not path.is_symlink())]:
        if command("xcrun", "lipo", "-archs", binary) != "arm64":
            raise ValueError(f"Expected an arm64 Mach-O for {binary.name}.")
        dependencies = command("xcrun", "otool", "-L", binary).splitlines()[1:]
        for dependency in dependencies:
            name = dependency.strip().split(" (", maxsplit=1)[0]
            if name.startswith(("/usr/lib/", "/System/Library/")):
                continue
            if name.startswith(("@rpath/", "@loader_path/")):
                path = libraries / name.split("/", maxsplit=1)[-1]
                if path.parent == libraries and path.is_file():
                    continue
            raise ValueError(f"Unresolved or machine-specific dependency for {binary.name}: {name}.")


def verify_native_metadata(executable, libraries):
    binaries = [executable, *(path for path in sorted(libraries.iterdir()) if not path.is_symlink())]
    for binary in binaries:
        entries = command("xcrun", "otool", "-l", binary).splitlines()
        minimum = None
        for index, entry in enumerate(entries):
            if entry.strip() == "cmd LC_BUILD_VERSION":
                platform = entries[index + 2].strip().lower()
                minimum = entries[index + 3].strip()
                if platform not in {"platform 1", "platform macos"} or minimum not in {
                    "minos 14.0", "minos 14.0.0",
                }:
                    raise ValueError(f"Unexpected minimum macOS version in {binary.name}.")
            if entry.strip() == "cmd LC_RPATH":
                path = entries[index + 2].strip().split(" (offset ", maxsplit=1)[0]
                if path not in {"path @loader_path", "path @executable_path/../Frameworks"}:
                    raise ValueError(f"Unexpected library search path in {binary.name}: {path}.")
        if minimum is None:
            raise ValueError(f"Missing Mach-O build version in {binary.name}.")


def sign(bundle, libraries, identity, team):
    signing = ["/usr/bin/codesign", "--force", "--sign", identity]
    signing += ["--timestamp=none"] if team is None else ["--options", "runtime", "--timestamp"]
    native_libraries = [path for path in sorted(libraries.iterdir()) if not path.is_symlink()]
    for library in native_libraries:
        command(*signing, library)
    command(*signing, bundle)
    check = ["/usr/bin/codesign", "--verify", "--strict"]
    if team:
        requirement = (
            "anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists "
            "and certificate leaf[field.1.2.840.113635.100.6.1.13] exists "
            f'and certificate leaf[subject.OU] = "{team}"'
        )
        for library in native_libraries:
            command(*check, f"-R={requirement}", library)
        command(*check, f'-R={requirement} and identifier "{BUNDLE_ID}"', bundle)
    command(*check, "--deep", bundle)


def manifest(bundle):
    members = []
    for path in sorted(bundle.rglob("*")):
        name = path.relative_to(bundle).as_posix()
        if path.is_symlink():
            members.append({"path": name, "link": str(path.readlink())})
        elif path.is_file():
            members.append({"path": name, "size": path.stat().st_size, "sha256": digest(path)})
    return members


def bundle_pin(bundle):
    """Hash a stable, type-aware manifest; count file and symlink logical bytes."""
    entries = sorted(bundle.rglob("*"), key=lambda path: os.fsencode(path.relative_to(bundle)))
    result = hashlib.sha256()
    logical_bytes = 0
    for path in entries:
        name = os.fsencode(path.relative_to(bundle)).hex()
        if path.is_symlink():
            target = os.fsencode(os.readlink(path))
            logical_bytes += len(target)
            record = f"L {name} {target.hex()}\n"
        elif path.is_dir():
            record = f"D {name}\n"
        elif path.is_file():
            size = path.stat().st_size
            logical_bytes += size
            record = f"F {name} {size} {digest(path)}\n"
        else:
            raise ValueError(f"Unsupported runtime bundle entry: {path}.")
        result.update(record.encode("ascii"))
    return logical_bytes, result.hexdigest()


def verify_final_bundle(bundle, team):
    """Validate the stapled release before and after the archive round trip."""
    if bundle.name != BUNDLE_NAME or bundle.is_symlink() or not bundle.is_dir():
        raise ValueError("Unexpected runtime bundle name or type.")
    contents = bundle / "Contents"
    executable = contents / "MacOS/llama-server"
    frameworks = contents / "Frameworks"
    for directory in (contents, executable.parent, frameworks, contents / "Resources"):
        if directory.is_symlink() or not directory.is_dir():
            raise ValueError(f"Invalid runtime directory: {directory}.")
    for required in (executable, contents / "Info.plist", contents / "Resources/LICENSE-llama.cpp"):
        if required.is_symlink() or not required.is_file():
            raise ValueError(f"Invalid runtime file: {required}.")
    if not os.access(executable, os.X_OK):
        raise ValueError("Runtime executable is not executable.")
    with (contents / "Info.plist").open("rb") as info:
        metadata = plistlib.load(info)
    if (metadata.get("CFBundleExecutable") != "llama-server"
            or metadata.get("CFBundleIdentifier") != BUNDLE_ID
            or metadata.get("CFBundleVersion") != BUILD[1:]):
        raise ValueError("Unexpected runtime bundle identity.")
    libraries = []
    for path in sorted(frameworks.iterdir()):
        if not LIBRARY_NAME.fullmatch(path.name):
            raise ValueError(f"Unexpected runtime framework: {path.name}.")
        if path.is_symlink():
            target = path.readlink()
            if len(target.parts) != 1 or not LIBRARY_NAME.fullmatch(str(target)):
                raise ValueError(f"Unsafe runtime library link: {path.name}.")
            resolved = path.resolve(strict=True)
            if resolved.parent != frameworks or not resolved.is_file() or not LIBRARY_NAME.fullmatch(resolved.name):
                raise ValueError(f"Unsafe runtime library target: {path.name}.")
        elif path.is_file():
            libraries.append(path)
        else:
            raise ValueError(f"Invalid runtime framework: {path.name}.")
    if not libraries:
        raise ValueError("Runtime contains no native libraries.")
    requirement = (
        "anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists "
        "and certificate leaf[field.1.2.840.113635.100.6.1.13] exists "
        f'and certificate leaf[subject.OU] = "{team}"'
    )
    for path in bundle.rglob("*"):
        if path.is_symlink():
            if path.parent != frameworks:
                raise ValueError(f"Unexpected runtime symlink: {path}.")
        elif path.is_file() and "Mach-O" in command("/usr/bin/file", "-b", path):
            if command("xcrun", "lipo", "-archs", path) != "arm64":
                raise ValueError(f"Expected an arm64 Mach-O for {path.name}.")
            command("/usr/bin/codesign", "--verify", "--strict", f"-R={requirement}", path)
        elif not path.is_file() and not path.is_dir():
            raise ValueError(f"Unexpected runtime entry: {path}.")
    verify_dependencies(executable, frameworks)
    verify_native_metadata(executable, frameworks)
    runtime_source.verify_notices(bundle)
    command("/usr/bin/codesign", "--verify", "--strict", "--deep",
            f'-R={requirement} and identifier "{BUNDLE_ID}"', bundle)
    command("xcrun", "stapler", "validate", bundle)


def finalize(bundle, output, team):
    """Archive an already notarized and stapled bundle, without signing or submission."""
    if output.exists():
        raise ValueError("Final output already exists.")
    verify_final_bundle(bundle, team)
    with TemporaryDirectory(prefix=".runtime-final-", dir=output.parent) as directory:
        stage = Path(directory)
        release = stage / "release"
        release.mkdir()
        archive_name = f"context-fabricator-llama-{BUILD}-macos-arm64.zip"
        archive = release / archive_name
        command("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", bundle, archive)
        extracted = stage / "roundtrip"
        extracted.mkdir()
        command("ditto", "-x", "-k", archive, extracted)
        entries = list(extracted.iterdir())
        if len(entries) != 1 or entries[0].name != BUNDLE_NAME:
            raise ValueError("Final archive has an unexpected layout.")
        restored = entries[0]
        verify_final_bundle(restored, team)
        bundle_bytes, bundle_hash = bundle_pin(restored)
        if (bundle_bytes, bundle_hash) != bundle_pin(bundle):
            raise ValueError("Archive round trip changed runtime bundle contents.")
        with zipfile.ZipFile(archive) as package_file:
            expanded_bytes = sum(item.file_size for item in package_file.infolist())
            if not 0 < len(package_file.infolist()) <= 10_000:
                raise ValueError("Final archive has an invalid entry count.")
        archive_bytes = archive.stat().st_size
        if not 0 < archive_bytes <= 512 * 1024 * 1024 or not 0 < bundle_bytes <= expanded_bytes <= 4 * 1024 * 1024 * 1024:
            raise ValueError("Final runtime archive size is invalid.")
        pins = {
            "archive": archive_name,
            "archiveBytes": archive_bytes,
            "archiveSHA256": digest(archive),
            "archiveExpandedBytes": expanded_bytes,
            "bundleBytes": bundle_bytes,
            "bundleManifestSHA256": bundle_hash,
            "appleTeamID": team,
            "bundleId": BUNDLE_ID,
            "signature": "developerIDSignedNotarizedStapled",
            "members": manifest(restored),
        }
        (release / "manifest.json").write_text(json.dumps(pins, indent=2) + "\n", encoding="utf-8")
        names = {
            "CTXFAB_RUNTIME_ARCHIVE_BYTES": str(archive_bytes),
            "CTXFAB_RUNTIME_ARCHIVE_SHA256": pins["archiveSHA256"],
            "CTXFAB_RUNTIME_ARCHIVE_EXPANDED_BYTES": str(expanded_bytes),
            "CTXFAB_RUNTIME_BUNDLE_BYTES": str(bundle_bytes),
            "CTXFAB_RUNTIME_BUNDLE_MANIFEST_SHA256": bundle_hash,
            "CTXFAB_RUNTIME_APPLE_TEAM_ID": team,
        }
        (release / "release-pins.env").write_text(
            "".join(f"export {name}={value}\n" for name, value in names.items()), encoding="ascii"
        )
        release.rename(output)


def package(archive, output, identity, team, *, api_source=False):
    with TemporaryDirectory(prefix=".runtime-", dir=output.parent) as directory:
        stage = Path(directory)
        upstream_sha = UPSTREAM_SHA256
        if api_source:
            source, binaries = runtime_source.build_server(archive, stage)
            upstream_sha = runtime_source.SOURCE_ARCHIVE_SHA256
        else:
            binaries = unpack(archive, stage)
        bundle = stage / BUNDLE_NAME
        executable, frameworks = install_files(binaries, bundle)
        verify_dependencies(executable, frameworks)
        if api_source:
            verify_native_metadata(executable, frameworks)
            runtime_source.stage_notices(source, binaries.parent, bundle / "Contents/Resources")
            runtime_source.verify_notices(bundle)
        sign(bundle, frameworks, identity, team)
        version = command(executable, "--version")
        if "build 12359, commit b1b770d90" not in version:
            raise ValueError("The staged executable did not report the pinned build.")
        archive_name = f"context-fabricator-llama-{BUILD}-macos-arm64.zip"
        command("ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", bundle, stage / archive_name)
        bundle_bytes, bundle_manifest_sha256 = bundle_pin(bundle)
        with zipfile.ZipFile(stage / archive_name) as archive_file:
            archive_expanded_bytes = sum(entry.file_size for entry in archive_file.infolist())
        if not 0 < bundle_bytes <= archive_expanded_bytes <= 4 * 1024 * 1024 * 1024:
            raise ValueError("Runtime package expanded size is invalid.")
        metadata = {
            "archiveExpandedBytes": archive_expanded_bytes,
            "bundleBytes": bundle_bytes,
            "bundleManifestSHA256": bundle_manifest_sha256,
            "upstreamSHA256": upstream_sha,
            "bundleId": BUNDLE_ID,
            "signature": "adHoc" if team is None else "developerIDSignedUnnotarized",
            "archive": archive_name,
            "archiveBytes": (stage / archive_name).stat().st_size,
            "archiveSHA256": digest(stage / archive_name),
            "members": manifest(bundle),
        }
        output.mkdir()
        bundle.rename(output / bundle.name)
        (stage / archive_name).rename(output / archive_name)
        (output / "manifest.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
        (output / "version.log").write_text(version + "\n", encoding="utf-8")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "finalize":
        parser = argparse.ArgumentParser(description="Archive an already notarized and stapled runtime bundle.")
        parser.add_argument("bundle", type=Path)
        parser.add_argument("output", type=Path)
        parser.add_argument("--team-id", required=True, help="Apple Developer ID Team ID")
        options = parser.parse_args(sys.argv[2:])
        if not re.fullmatch(r"[A-Z0-9]{10}", options.team_id):
            parser.error("Invalid Apple Team ID.")
        finalize(options.bundle.resolve(strict=True), options.output.resolve(), options.team_id)
    else:
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument("archive", type=Path)
        parser.add_argument("output", type=Path)
        parser.add_argument("--api-source", action="store_true", help="Build the pinned API-only source archive.")
        parser.add_argument("--signing-identity", default="-", help="Developer ID Application identity")
        parser.add_argument("--team-id", help="Apple Developer ID Team ID")
        options = parser.parse_args()
        if (options.signing_identity == "-") != (options.team_id is None):
            parser.error("Provide both a Developer ID Application identity and its Team ID.")
        if options.team_id and not re.fullmatch(r"[A-Z0-9]{10}", options.team_id):
            parser.error("Invalid Apple Team ID.")
        package(options.archive.resolve(strict=True), options.output.resolve(),
                options.signing_identity, options.team_id, api_source=options.api_source)
