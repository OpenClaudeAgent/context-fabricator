"""Seed or verify a disposable profile using the installed Ubuntu Qt package."""

import argparse
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
from contextlib import closing
from pathlib import Path
from uuid import uuid4

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "desktop/qt/tests"))
sys.path.insert(0, str(ROOT / "quality/github"))

from linux_wayland_smoke import check_window
from linux_window_smoke import (
    INSTALLED_APP, INSTALLED_BACKEND, INSTALLED_CLI, check_isolation, session_request,
)
from secret_service_check import keyring, secret_method


APPLICATION = "org.contextfabricator.providers"
TOKEN = b"github_pat_fixture_offline_upgrade"
LOGIN = "upgrade-fixture"
RETENTION = 73
COLLECTION = "qt-upgrade-fixture"
SOURCE = "src/upgrade_probe.rs"
REVISION = "retained-before-upgrade"
SEARCH = "/v1/search?query=upgradecontinuitymarker&mode=bm25"
MANIFEST = "qualification.json"


def set_up_fixture(database, reference):
    subprocess.run(
        ["secret-tool", "store", "--label=Context Fabricator upgrade fixture",
         "application", APPLICATION, "reference", reference],
        input=TOKEN, check=True, timeout=7,
    )
    with closing(sqlite3.connect(database, timeout=7)) as connection, connection:
        connection.execute("INSERT INTO collections(name) VALUES (?)", (COLLECTION,))
        connection.execute(
            "INSERT INTO documents(collection, source, revision, kind, title, content) "
            "VALUES (?, ?, ?, 'history', ?, ?)",
            (COLLECTION, SOURCE, REVISION, "Upgrade fixture",
             "upgradecontinuitymarker remains searchable after installation."),
        )
        connection.execute(
            "UPDATE github_connection SET credential_ref = ?, login = ?, revision = 1 "
            "WHERE singleton = 1",
            (reference, LOGIN),
        )


def check_api(session, collection):
    denied = session_request(dict(session, token=""), SEARCH, expected=401)
    hits = session_request(session, SEARCH)
    assert [(item["source"], item["revision"]) for item in hits] == [(SOURCE, REVISION)], hits
    binding = session_request(session, "/v1/github")
    assert (binding["credential"], binding["login"], binding["revision"]) == (
        "connected", LOGIN, 1,
    ), binding

    # This reaches the provider adapter only after Secret Service supplied the existing item.
    offline = session_request(session, "/v1/github/repositories", expected=503)
    assert offline["code"] == "github_unavailable", offline
    locked = secret_method("Lock", f"['{collection}']")
    assert locked.returncode == 0, locked.stderr
    rejected = session_request(session, "/v1/github/repositories", expected=503)
    assert rejected["code"] == "github_keychain_unavailable", rejected
    for response in (denied, hits, binding, offline, rejected):
        assert TOKEN not in json.dumps(response).encode()


def check_persistence(profile, reference):
    database = profile / "context.sqlite3"
    with closing(sqlite3.connect(f"{database.as_uri()}?mode=ro", uri=True, timeout=7)) as connection:
        assert connection.execute("PRAGMA quick_check").fetchall() == [("ok",)]
        assert connection.execute(
            "SELECT credential_ref FROM github_connection WHERE singleton = 1"
        ).fetchone() == (reference,)
    assert profile.stat().st_mode & 0o077 == 0
    assert (profile / "endpoint.json").stat().st_mode & 0o777 == 0o600
    for file in profile.glob("context.sqlite3*"):
        assert TOKEN not in file.read_bytes(), f"Found plaintext in {file.name}"


def exercise(root, build, seed, runtime):
    product_root = root / "product"
    profile = product_root / "context-fabricator"
    manifest = root / MANIFEST
    reference = f"github/{uuid4()}" if seed else json.loads(manifest.read_text())["credentialRef"]
    control = runtime / "keyring"
    control.mkdir(mode=0o700)

    with keyring(control) as collection:
        def inspect_session(session):
            if seed:
                set_up_fixture(profile / "context.sqlite3", reference)
            item = subprocess.run(
                ["secret-tool", "lookup", "application", APPLICATION, "reference", reference],
                capture_output=True, check=True, timeout=7,
            )
            assert item.stdout.rstrip(b"\n") == TOKEN, "The isolated keyring item changed."
            check_api(session, collection)
            check_persistence(profile, reference)

        check_window(INSTALLED_APP, INSTALLED_BACKEND, INSTALLED_CLI, build, product_root,
                     RETENTION if seed else None, None if seed else RETENTION,
                     inspect_session=inspect_session)
    if seed:
        manifest.write_text(json.dumps({"credentialRef": reference}), encoding="utf-8")
    print("The installed package opened the retained corpus and settings with the private keyring.")
    print("GitHub stayed offline, locked credentials returned the expected problem, and no fixture token appeared in HTTP or SQLite.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("stage", choices=("seed", "verify"))
    parser.add_argument("profile_root", type=Path)
    parser.add_argument("--build", required=True)
    parser.add_argument("--private-bus", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    check_isolation()
    root = args.profile_root.resolve(strict=True)
    seed = args.stage == "seed"
    if args.private_bus:
        exercise(root, args.build, seed, Path(os.environ["XDG_RUNTIME_DIR"]))
        return

    assert (root / MANIFEST).is_file() if not seed else not any(root.iterdir()), (
        "Use a new empty profile root for seed and its saved fixture root for verify."
    )
    (root / "product").mkdir(mode=0o700, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="ctxfab-package-keyring-") as runtime:
        environment = {key: value for key, value in os.environ.items() if not key.startswith("DBUS_")}
        environment.update(XDG_RUNTIME_DIR=runtime, XDG_DATA_HOME=str(root / "secrets"),
                           XDG_CONFIG_HOME=str(root / "keyring-config"))
        subprocess.run(
            ["dbus-run-session", "--", sys.executable, "-B", __file__, args.stage,
             root, "--build", args.build, "--private-bus"],
            env=environment, check=True,
        )


if __name__ == "__main__":
    main()
