"""Stage license and notice texts from the inputs of the macOS Release build."""

import argparse
import hashlib
import json
import re
import shutil
import subprocess
from pathlib import Path
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[2]
SUPPLEMENTS = Path(__file__).with_name("notices")
TARGET = "aarch64-apple-darwin"
DOCUMENT = re.compile(r"^(license|licence|copying|notice|unlicense|copyright)(?:$|[-_.])", re.I)
SAFE_NAME = re.compile(r"[A-Za-z0-9._+-]+")
MISSING_CRATE_DOCUMENTS = {
    ("defmt", "0.3.101"): (
        "defmt", "knurling-rs/defmt", "719491ca3529abd07205165b9b10e4d988552bc2", "LICENSE-MIT"),
    ("opentelemetry", "0.33.1"): (
        "opentelemetry", "open-telemetry/opentelemetry-rust", "372ebbc74d4dba71bb9820732f57f7316b4e7a1e", "LICENSE"),
    ("opentelemetry_sdk", "0.33.1"): (
        "opentelemetry", "open-telemetry/opentelemetry-rust", "372ebbc74d4dba71bb9820732f57f7316b4e7a1e", "LICENSE"),
    ("tree-sitter-rust", "0.24.0"): (
        "tree-sitter-rust", "tree-sitter/tree-sitter-rust", "18b0515fca567f5a10aee9978c6d2640e878671a", "LICENSE"),
    ("tree-sitter-java", "0.23.5"): (
        "tree-sitter-java", "tree-sitter/tree-sitter-java", "94703d5a6bed02b98e438d7cad1136c01a60ba2c", "LICENSE"),
}


def run(*args):
    return subprocess.check_output(args)


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def cargo_dependencies(workspace, binary, target, root):
    metadata = json.loads(run("cargo", "metadata", "--manifest-path", root / workspace / "Cargo.toml",
                              "--locked", "--format-version", "1", "--filter-platform", target))
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    [root] = [package_id for package_id in metadata["workspace_members"]
              if any(target["name"] == binary and "bin" in target["kind"]
                     for target in packages[package_id]["targets"])]
    pending = [root]
    seen = set()
    while pending:
        current = pending.pop()
        if current in seen:
            continue
        seen.add(current)
        package = packages[current]
        node = nodes[current]
        if package["source"]:
            yield package, node["features"]
        pending.extend(dep["pkg"] for dep in node["deps"]
                       if any(kind["kind"] is None for kind in dep["dep_kinds"]))


def cargo_documents(package, features):
    root = Path(package["manifest_path"]).parent
    documents = {path.name: path.read_bytes() for path in root.iterdir()
                 if path.is_file() and DOCUMENT.match(path.name)}
    if package["license_file"]:
        path = root / package["license_file"]
        documents[path.name] = path.read_bytes()
    source = package["repository"] or package["source"]
    if not documents:
        key = (package["name"], package["version"])
        supplement = MISSING_CRATE_DOCUMENTS.get(key)
        if supplement is None:
            raise ValueError(f"No license document for {key[0]} {key[1]}.")
        name, repository, revision, document = supplement
        if read_json(root / ".cargo_vcs_info.json")["git"]["sha1"] != revision:
            raise ValueError(f"Different upstream revision for {key[0]} {key[1]}.")
        documents["LICENSE"] = (SUPPLEMENTS / f"LICENSE-{name}.txt").read_bytes()
        source = f"https://github.com/{repository}/blob/{revision}/{document}"
    if package["name"] == "libsqlite3-sys" and "bundled" in features:
        amalgamation = (root / "sqlite3/sqlite3.c").read_bytes()
        version = re.search(rb'^\*\* version ([0-9.]+)\.', amalgamation, re.M)
        if version is None:
            raise ValueError("Cannot identify the bundled SQLite version.")
        documents["NOTICE-SQLite.txt"] = (
            f"This package includes the SQLite {version.group(1).decode('ascii')} amalgamation.\n"
            "SQLite declares the library code public domain. https://sqlite.org/copyright.html\n"
            f"Amalgamation SHA-256 {hashlib.sha256(amalgamation).hexdigest()}\n"
        ).encode()
    return source, documents


def swift_pins():
    pins = {}
    for lock in ("desktop/macos/Package.resolved", "desktop/macos/Client/Package.resolved"):
        for pin in read_json(ROOT / lock)["pins"]:
            if pins.setdefault(pin["identity"], pin) != pin:
                raise ValueError(f"Swift package locks disagree for {pin['identity']}.")
    return pins


def swift_documents(checkout, pin):
    revision = pin["state"]["revision"]
    if run("git", "-C", checkout, "rev-parse", "HEAD").decode().strip() != revision:
        raise ValueError(f"Swift checkout does not match the lock: {pin['identity']}.")
    files = run("git", "-C", checkout, "ls-tree", "--name-only", "-z", revision).decode()
    documents = {name: run("git", "-C", checkout, "show", f"{revision}:{name}")
                 for name in files.split("\0") if DOCUMENT.match(name)}
    if not documents:
        raise ValueError(f"No license document for Swift package {pin['identity']}.")
    if pin["identity"] == "yams":
        documents["LICENSE-libyaml.txt"] = (SUPPLEMENTS / "LICENSE-libyaml.txt").read_bytes()
    return documents


def safe_name(name):
    if SAFE_NAME.fullmatch(name) is None or name in {".", ".."}:
        raise ValueError(f"Invalid notice path component: {name!r}.")
    return name


def write_component(stage, family, name, version, source, documents, **details):
    directory = Path(family) / f"{safe_name(name)}-{safe_name(version)}"
    (stage / directory).mkdir(parents=True)
    files = {}
    for filename, data in sorted(documents.items()):
        if not data.strip():
            raise ValueError(f"Empty license document for {name}: {filename}.")
        path = directory / safe_name(filename)
        (stage / path).write_bytes(data)
        files[path.as_posix()] = hashlib.sha256(data).hexdigest()
    return {"family": family, "name": name, "version": version,
            "source": source, "documents": files, **details}


def rust_components(stage, target, root=ROOT):
    rust = {}
    for workspace, binary in (("backend", "ctxfab-server"), ("cli", "ctxfab")):
        for package, features in cargo_dependencies(workspace, binary, target, root):
            key = (package["name"], package["version"])
            previous, enabled, consumers = rust.setdefault(key, (package, set(), set()))
            if previous["source"] != package["source"]:
                raise ValueError(f"Conflicting Cargo sources for {key[0]} {key[1]}.")
            enabled.update(features)
            consumers.add(binary)
    entries = []
    for (name, version), (package, features, consumers) in sorted(rust.items()):
        source, documents = cargo_documents(package, features)
        entries.append(write_component(stage, "rust", name, version, source, documents,
                                       executables=sorted(consumers), license=package["license"]))
    return entries


def stage_notices(output, checkouts):
    swift = {path.name.lower(): path for path in checkouts.iterdir() if path.is_dir()}
    output.parent.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix=".app-notices-", dir=output.parent) as temporary:
        stage = Path(temporary) / "notices"
        entries = rust_components(stage, TARGET)
        for name, pin in sorted(swift_pins().items()):
            documents = swift_documents(swift[name], pin)
            state = pin["state"]
            entries.append(write_component(stage, "swift", name, state.get("version") or state["revision"],
                                           pin["location"], documents, revision=state["revision"]))
        (stage / "README.txt").write_text(
            "Context Fabricator third-party notices\n\n"
            "Rust notices cover the locked normal dependency graph of the arm64 helper and CLI.\n"
            "Swift notices cover all packages in both Xcode and client locks, including build tools.\n"
            "The separate inference runtime and downloaded models have their own licenses.\n"
            "See manifest.json for package origins and the local paths to their notices.\n",
            encoding="utf-8")
        (stage / "manifest.json").write_text(json.dumps(entries, indent=2) + "\n", encoding="utf-8")
        verify_notices(stage)
        if output.exists():
            shutil.rmtree(output)
        stage.rename(output)


def verify_notices(directory):
    entries = read_json(directory / "manifest.json")
    documents = {}
    for entry in entries:
        if not entry["documents"]:
            raise ValueError(f"No bundled notice files for {entry['name']}.")
        for path, digest in entry["documents"].items():
            parts = path.split("/")
            if len(parts) != 3 or path in documents:
                raise ValueError(f"Invalid or duplicate notice entry: {path}.")
            for part in parts:
                safe_name(part)
            documents[path] = digest
    expected = set(documents) | {"manifest.json", "README.txt"}
    bundled = {path.relative_to(directory).as_posix()
               for path in directory.rglob("*") if path.is_file() or path.is_symlink()}
    if bundled != expected or {entry["family"] for entry in entries} != {"rust", "swift"}:
        raise ValueError("Missing or unexpected files in the app notices.")
    for path, digest in documents.items():
        file = directory / path
        if file.is_symlink() or hashlib.sha256(file.read_bytes()).hexdigest() != digest:
            raise ValueError(f"Notice checksum mismatch: {path}.")
    print(f"Verified {len(documents)} app notice documents for {len(entries)} packages.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path, nargs="?")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--swift-checkouts", type=Path, help="Assemble from these resolved Xcode checkouts.")
    mode.add_argument("--check-rust-target", help="Check all Rust notices for this target before building.")
    args = parser.parse_args()
    if args.check_rust_target is not None:
        if args.directory is not None:
            parser.error("--check-rust-target uses a temporary directory.")
        with TemporaryDirectory(prefix="ctxfab-rust-notices-") as temporary:
            entries = rust_components(Path(temporary), args.check_rust_target)
            print(f"Checked Rust notices for {len(entries)} packages on {args.check_rust_target}.")
    elif args.directory is None:
        parser.error("Provide the notice directory to assemble or verify.")
    elif args.swift_checkouts is None:
        verify_notices(args.directory)
    else:
        stage_notices(args.directory, args.swift_checkouts)
