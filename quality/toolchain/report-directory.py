#!/usr/bin/env python3
"""Allocate a report and bound retained local qualification output."""

import argparse
import errno
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile
import time

MARKER = ".ctxfab-report.json"
KEEP = 2
BUDGET_BYTES = 2_000_000_000
# Leave room for the lock and the report directory entries created by publication.
METADATA_RESERVE = 1024**2


def dead(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return True
    except PermissionError:
        pass
    return False


def size(path):
    try:
        total = path.lstat().st_blocks * 512
    except FileNotFoundError:
        return 0
    if path.is_dir() and not path.is_symlink():
        for entry in path.rglob("*"):
            try:
                total += entry.lstat().st_blocks * 512
            except FileNotFoundError:
                continue
    return total


def inventory(root):
    completed = []
    try:
        unmanaged = root.parent.stat().st_blocks * 512
        unmanaged += sum(size(entry) for entry in root.parent.iterdir() if entry != root)
    except FileNotFoundError:
        return 0, completed
    try:
        unmanaged += root.stat().st_blocks * 512
        folders = list(root.iterdir())
    except FileNotFoundError:
        return unmanaged, completed
    for folder in folders:
        managed = False
        try:
            if folder.is_dir() and not folder.is_symlink():
                marker = folder / MARKER
                if marker.is_file() and not marker.is_symlink():
                    state = json.loads(marker.read_text())
                    kind, pid, created = state["kind"], state["pid"], state["created"]
                    managed = (
                        isinstance(kind, str)
                        and folder.name.startswith(f"{kind}.")
                        and isinstance(pid, int)
                        and pid > 0
                        and isinstance(created, int)
                        and (state.get("finished") is True or dead(pid))
                    )
                    if managed:
                        completed.append((created, kind, folder))
        except (FileNotFoundError, ValueError, KeyError, TypeError, OverflowError):
            pass
        if not managed:
            unmanaged += size(folder)
    return unmanaged, completed


def validate_unmanaged_size(root, unmanaged):
    if unmanaged > BUDGET_BYTES:
        raise RuntimeError(
            f"{root.parent} contains {unmanaged:,} bytes of unmanaged output. "
            "The local 2 GB budget is already exceeded."
        )


def prune(root, incoming, incoming_kind):
    unmanaged, completed = inventory(root)
    incoming_bytes = size(incoming)
    available = BUDGET_BYTES - METADATA_RESERVE - unmanaged
    can_publish = incoming_bytes <= available
    count = {}
    remaining = available - (incoming_bytes if can_publish else 0)
    for _, kind, folder in sorted(completed, reverse=True):
        retained = count.get(kind, 0)
        limit = KEEP - (can_publish and kind == incoming_kind)
        try:
            used = size(folder)
            if retained >= limit or used > remaining:
                shutil.rmtree(folder)
            else:
                remaining -= used
                count[kind] = retained + 1
        except FileNotFoundError:
            # The operator may be clearing old artifacts at the same time.
            continue
    validate_unmanaged_size(root, unmanaged)
    return can_publish


def locked(root):
    lock = (root / ".retention.lock").open("a")
    fcntl.flock(lock, fcntl.LOCK_EX)
    return lock


def allocate(root, kind, pid):
    if not re.fullmatch(r"[a-z][a-z0-9-]*", kind):
        raise ValueError("Invalid report family")
    temporary = Path(tempfile.gettempdir()).resolve()
    if os.environ.get("CI"):
        root.mkdir(parents=True, exist_ok=True)
        temporary = root
    else:
        unmanaged, _ = inventory(root)
        validate_unmanaged_size(root, unmanaged)
        if temporary.is_relative_to(root.parent.resolve()):
            temporary = Path("/tmp")
    path = Path(tempfile.mkdtemp(prefix=f"{kind}.", dir=temporary))
    (path / MARKER).write_text(json.dumps(
        {"kind": kind, "pid": pid, "created": time.time_ns()}
    ))
    return path


def finish(path, pid, destination):
    if os.environ.get("CI") or path.parent == destination:
        return
    owned = False
    try:
        marker = path / MARKER
        state = json.loads(marker.read_text())
        if state["pid"] != pid or path.is_symlink() or marker.is_symlink():
            return
        owned = True
        marker.write_text(json.dumps({**state, "finished": True}))
        destination.mkdir(parents=True, exist_ok=True)
        with locked(destination):
            published = destination / path.name
            if published.exists() or published.is_symlink():
                raise FileExistsError(published)
            if prune(destination, path, state["kind"]):
                try:
                    path.rename(published)
                except OSError as error:
                    if error.errno != errno.EXDEV:
                        raise
                    # Copy across volumes before entering the budgeted directory so the
                    # allocation on the destination filesystem is included in the check.
                    with tempfile.TemporaryDirectory(
                        prefix=".ctxfab-report-", dir=destination.parent.parent
                    ) as staging:
                        copy = Path(staging) / path.name
                        shutil.copytree(path, copy, symlinks=True)
                        if prune(destination, copy, state["kind"]):
                            copy.rename(published)
            if published.exists():
                print(f"Reports saved in {published}")
            else:
                print(f"Report omitted under the local 2 GB .quality budget: {path.name}", file=sys.stderr)
    except FileNotFoundError:
        return
    finally:
        if owned and path.is_dir():
            shutil.rmtree(path)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("family")
    parser.add_argument("pid", type=int)
    parser.add_argument("--destination", type=Path)
    args = parser.parse_args()
    try:
        if args.family == "finish":
            if args.destination is None:
                parser.error("finish requires --destination")
            finish(args.root, args.pid, args.destination)
        else:
            print(allocate(args.root, args.family, args.pid))
    except RuntimeError as error:
        parser.exit(1, f"{error}\n")
