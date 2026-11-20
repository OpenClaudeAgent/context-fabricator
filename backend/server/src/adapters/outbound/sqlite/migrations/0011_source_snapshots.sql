ALTER TABLE collections ADD COLUMN snapshot_mode INTEGER NOT NULL DEFAULT 0
    CHECK (snapshot_mode IN (0, 1));

CREATE TABLE source_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    collection TEXT NOT NULL REFERENCES collections(name),
    generation INTEGER NOT NULL CHECK (generation >= 0),
    predecessor INTEGER,
    revision TEXT NOT NULL,
    tracked_ref TEXT NOT NULL,
    phase TEXT NOT NULL CHECK (phase IN ('staging', 'sealed', 'published')),
    sequence INTEGER NOT NULL DEFAULT 0 CHECK (sequence >= 0),
    cursor TEXT,
    UNIQUE (id, collection)
) STRICT;
CREATE UNIQUE INDEX one_published_snapshot ON source_snapshots(collection) WHERE phase = 'published';
CREATE UNIQUE INDEX one_pending_snapshot ON source_snapshots(collection) WHERE phase <> 'published';

CREATE TABLE source_blobs (
    collection TEXT NOT NULL REFERENCES collections(name),
    blob TEXT NOT NULL,
    content TEXT NOT NULL,
    PRIMARY KEY (collection, blob)
) WITHOUT ROWID, STRICT;

CREATE TABLE source_snapshot_files (
    snapshot_id INTEGER NOT NULL,
    collection TEXT NOT NULL,
    path TEXT NOT NULL,
    source TEXT NOT NULL,
    blob TEXT NOT NULL,
    file_url TEXT NOT NULL,
    PRIMARY KEY (snapshot_id, path),
    UNIQUE (snapshot_id, source),
    FOREIGN KEY (snapshot_id, collection) REFERENCES source_snapshots(id, collection) ON DELETE CASCADE,
    FOREIGN KEY (collection, blob) REFERENCES source_blobs(collection, blob)
) WITHOUT ROWID, STRICT;
CREATE INDEX snapshot_blob_references ON source_snapshot_files(collection, blob);
CREATE INDEX snapshot_sources ON source_snapshot_files(source);

CREATE TRIGGER invalidate_pending_snapshots AFTER UPDATE OF generation ON collections
WHEN new.generation <> old.generation AND new.snapshot_mode = 1
BEGIN
    DELETE FROM source_snapshots WHERE collection = new.name AND phase <> 'published';
    DELETE FROM source_blobs WHERE collection = new.name AND NOT EXISTS (
        SELECT 1 FROM source_snapshot_files
        WHERE collection = source_blobs.collection AND blob = source_blobs.blob
    );
END;
