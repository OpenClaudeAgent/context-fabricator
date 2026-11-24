"""Exercise the retained vector cache against a fresh temporary copy of the fixture."""

import argparse
import json
import sqlite3
import tempfile
from pathlib import Path

from checkpoint import REFERENCE, restore_into_fixture


def roundtrip(data, report, reference=REFERENCE, descriptor_path=None):
    descriptor = json.loads((descriptor_path or report / "public-vectors.json").read_text())
    with tempfile.TemporaryDirectory(prefix="ctxfab-vector-replay-") as temp:
        local = Path(temp)
        with (
            sqlite3.connect(
                (data / "context.sqlite3").resolve().as_uri() + "?mode=ro", uri=True
            ) as live,
            sqlite3.connect(local / "context.sqlite3") as copy,
        ):
            live.backup(copy, pages=256, sleep=0.02)
            copy.execute("DELETE FROM document_vectors")
            copy.commit()
        wrong_pin = json.loads(reference.read_text())
        wrong_pin["expectedPublication"]["manifestSha256"] = "0" * 64
        wrong_reference = local / "wrong-reference.json"
        wrong_reference.write_text(json.dumps(wrong_pin))
        try:
            restore_into_fixture(local, report / descriptor["file"], descriptor["sha256"], wrong_reference)
        except AssertionError:
            pass
        else:
            raise AssertionError("A wrong publication pin was accepted")
        with sqlite3.connect(local / "context.sqlite3") as copy:
            assert copy.execute("SELECT count(*) FROM document_vectors").fetchone() == (0,)
        restored = restore_into_fixture(
            local, report / descriptor["file"], descriptor["sha256"], reference
        )
        with sqlite3.connect(local / "context.sqlite3") as copy:
            copy.execute(
                "ATTACH DATABASE ? AS archived",
                (
                    (report / descriptor["file"]).resolve().as_uri()
                    + "?immutable=1&mode=ro",
                ),
            )
            mismatch = copy.execute("""SELECT count(*) FROM documents d JOIN document_vectors v ON v.document_id=d.id
                 JOIN archived.vectors a ON a.path=d.title AND a.start_byte=d.passage_start
                 WHERE v.embedding<>a.embedding""").fetchone()[0]
            assert mismatch == 0
            assert copy.execute("SELECT count(*) FROM document_vectors").fetchone() == (
                restored,
            )
            assert copy.execute("PRAGMA quick_check").fetchall() == [("ok",)]
    return {
        "sourceCommit": descriptor["sourceCommit"],
        "checkpointSha256": descriptor["sha256"],
        "rowsRestored": restored,
        "vectorPayloadMismatches": mismatch,
        "databaseQuickCheck": "ok",
        "fixtureCopyRemoved": True,
        "wrongManifestPinRejectedWithoutWrites": True,
    }


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Assertions must be enabled.")
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("data", type=Path)
    p.add_argument("report", type=Path)
    p.add_argument("--reference", type=Path, default=REFERENCE)
    args = p.parse_args()
    print(json.dumps(roundtrip(args.data, args.report, args.reference), indent=2))
