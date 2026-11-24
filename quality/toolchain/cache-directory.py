#!/usr/bin/env python3
"""Print the local cache base or the reusable build directory for a checkout."""

import argparse
import hashlib
import os
from pathlib import Path
import sys


def cache_root():
    if override := os.environ.get("CTXFAB_CACHE_DIR"):
        return Path(override).expanduser().resolve()
    default = Path.home() / ("Library/Caches" if sys.platform == "darwin" else ".cache")
    configured = Path(os.environ.get("XDG_CACHE_HOME") or default)
    home = configured if configured.is_absolute() else default
    return (home / "ContextFabricator").resolve()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--checkout", type=Path, default=Path(__file__).resolve().parents[2]
    )
    parser.add_argument("--build", action="store_true")
    args = parser.parse_args()
    checkout = args.checkout.resolve()
    path = cache_root()
    if path.is_relative_to((checkout / ".quality").resolve()):
        parser.error("The cache directory must be outside .quality")
    if args.build:
        identity = hashlib.sha256(os.fsencode(checkout)).hexdigest()[:16]
        path = path / "quality-builds" / identity
    print(path)
