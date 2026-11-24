"""Preserve pinned public passage identities and qualified embedding vectors."""

import argparse
import hashlib
import json
import os
import sqlite3
from pathlib import Path

REFERENCE = Path(__file__).resolve().parents[1] / "uv-reference.json"
PROFILE = "qwen3-embedding-0.6b-q8_0-06507c7b-d1024-v1"
SELECT = """SELECT d.id,d.title,d.content,d.revision,d.source_location FROM documents d
          ORDER BY d.title,d.passage_start"""


def verify_publication(db, reference=REFERENCE):
    ref = json.loads(reference.read_text())
    assert db.execute("SELECT DISTINCT revision FROM documents").fetchall() == [
        (ref["commit"],)
    ]
    paths = db.execute("""SELECT f.path,f.blob,s.revision FROM source_snapshot_files f JOIN source_snapshots s
                       ON s.id=f.snapshot_id WHERE s.phase='published' ORDER BY f.path""")
    manifest = hashlib.sha256()
    count = 0
    for path, blob, revision in paths:
        assert revision == ref["commit"]
        manifest.update(f"{path}\0{blob}\n".encode())
        count += 1
    assert {"paths": count, "manifestSha256": manifest.hexdigest()} == ref[
        "expectedPublication"
    ]
    return ref


def identity(row):
    _, path, content, _, encoded = row
    loc = json.loads(encoded)
    assert loc["path"] == path
    return (
        path,
        loc["blob"],
        loc["startByte"],
        loc["endByte"],
        hashlib.sha256(path.encode() + b"\0" + content.encode()).digest(),
    )


def verify_passage(db, row, cache):
    _, path, content, revision, encoded = row
    loc = json.loads(encoded)
    if path not in cache:
        found = db.execute("""SELECT f.blob,b.content FROM source_snapshot_files f
            JOIN source_snapshots s ON s.id=f.snapshot_id
            JOIN source_blobs b ON b.collection=f.collection AND b.blob=f.blob
            WHERE s.phase='published' AND s.revision=? AND f.path=?""", (revision, path)).fetchall()
        assert len(found) == 1
        blob, text = found[0]
        raw = text.encode()
        assert hashlib.sha1(f"blob {len(raw)}\0".encode() + raw).hexdigest() == blob
        cache[path] = blob, raw
    blob, raw = cache[path]
    start, end = loc["startByte"], loc["endByte"]
    assert loc["path"] == path and loc["blob"] == blob
    assert 0 <= start < end <= len(raw) and raw[start:end] == content.encode()


def export(data, output, reference=REFERENCE):
    if output.exists():
        raise FileExistsError(output)
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_suffix(".partial")
    with temporary.open("xb"):
        pass
    temporary.chmod(0o600)
    try:
        with (
            sqlite3.connect(
                (data / "context.sqlite3").resolve().as_uri() + "?mode=ro", uri=True
            ) as src,
            sqlite3.connect(temporary) as dest,
        ):
            src.execute("BEGIN")
            ref = verify_publication(src, reference)
            # The backend treats an absent singleton as its locked Qwen default.
            # A present alternate selection must never be exported as this profile.
            selected = src.execute("SELECT profile FROM embedding_selection").fetchall()
            assert selected in ([], [(PROFILE,)])
            dest.execute(
                "CREATE TABLE metadata(commit_id TEXT, profile TEXT, manifest_sha256 TEXT)"
            )
            dest.execute(
                "INSERT INTO metadata VALUES(?,?,?)",
                (ref["commit"], PROFILE, ref["expectedPublication"]["manifestSha256"]),
            )
            dest.execute("""CREATE TABLE vectors(path TEXT, blob TEXT, start_byte INTEGER, end_byte INTEGER,
                         input_sha256 BLOB, embedding BLOB NOT NULL CHECK(length(embedding)=4096),
                         PRIMARY KEY(path,start_byte)) WITHOUT ROWID""")
            n, cache = 0, {}
            for row in src.execute(SELECT):
                verify_passage(src, row, cache)
                v = src.execute(
                    "SELECT profile, embedding FROM document_vectors WHERE document_id=?",
                    (row[0],),
                ).fetchone()
                assert v and v[0] == PROFILE and len(v[1]) == 4096
                dest.execute(
                    "INSERT INTO vectors VALUES(?,?,?,?,?,?)", (*identity(row), v[1])
                )
                n += 1
            assert n > 0 and src.execute(
                "SELECT count(*) FROM document_vectors"
            ).fetchone() == (n,)
            dest.commit()
        temporary.chmod(0o400)
        temporary.replace(output)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise
    with output.open("rb") as f:
        digest = hashlib.file_digest(f, "sha256").hexdigest()
    return {
        "file": output.name,
        "sha256": digest,
        "bytes": output.stat().st_size,
        "rows": n,
        "profile": PROFILE,
        "sourceCommit": ref["commit"],
        "manifestSha256": ref["expectedPublication"]["manifestSha256"],
        "fields": [
            "path",
            "Git blob",
            "start/end bytes",
            "input digest",
            "embedding payload",
        ],
    }


def restore_into_fixture(data, archive, expected_digest, reference=REFERENCE):
    """Called by a benchmark owner while its isolated helper is stopped."""
    with archive.open("rb") as f:
        assert hashlib.file_digest(f, "sha256").hexdigest() == expected_digest
    path = data / "context.sqlite3"
    assert path.is_file()
    with (
        sqlite3.connect(path) as dst,
        sqlite3.connect(
            archive.resolve().as_uri() + "?mode=ro&immutable=1", uri=True
        ) as src,
    ):
        dst.execute("BEGIN IMMEDIATE")
        ref = verify_publication(dst, reference)
        assert src.execute("SELECT * FROM metadata").fetchall() == [
            (ref["commit"], PROFILE, ref["expectedPublication"]["manifestSha256"])
        ]
        assert dst.execute("SELECT count(*) FROM document_vectors").fetchone() == (0,)
        n, cache = 0, {}
        for row in dst.execute(SELECT):
            verify_passage(dst, row, cache)
            key = identity(row)
            saved = src.execute(
                "SELECT path,blob,start_byte,end_byte,input_sha256,embedding FROM vectors WHERE path=? AND start_byte=?",
                (key[0], key[2]),
            ).fetchone()
            assert saved and saved[:-1] == key and len(saved[-1]) == 4096
            dst.execute(
                "INSERT INTO document_vectors(document_id,profile,embedding) VALUES(?,?,?)",
                (row[0], PROFILE, saved[-1]),
            )
            n += 1
        assert n > 0 and src.execute(
            "SELECT count(*) FROM vectors"
        ).fetchone() == (n,)
        return n


if __name__ == "__main__":
    if not __debug__:
        raise SystemExit("Assertions must be enabled.")
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("data", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--reference", type=Path, default=REFERENCE)
    args = parser.parse_args()
    print(json.dumps(export(args.data, args.output, args.reference), indent=2))
