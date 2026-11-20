ALTER TABLE collections ADD COLUMN access_lost INTEGER NOT NULL DEFAULT 0
    CHECK (access_lost IN (0, 1));

DROP TRIGGER documents_insert;
DROP TRIGGER documents_delete;
DROP TRIGGER documents_update;
DROP TABLE documents_fts;
ALTER TABLE documents RENAME TO documents_v1;

CREATE TABLE documents (
    id INTEGER PRIMARY KEY,
    collection TEXT NOT NULL REFERENCES collections(name),
    source TEXT NOT NULL,
    revision TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('current', 'history', 'workflow')),
    tracked_ref TEXT NOT NULL DEFAULT '' CHECK (kind = 'current' OR tracked_ref = ''),
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    UNIQUE (collection, source, kind, tracked_ref, revision)
) STRICT;

INSERT INTO documents (id, collection, source, revision, kind, title, content)
SELECT id, collection, source, revision, kind, title, content FROM documents_v1;
DROP TABLE documents_v1;

CREATE UNIQUE INDEX one_current_revision_per_ref
    ON documents (collection, source, tracked_ref)
    WHERE kind = 'current' AND tracked_ref <> '';

CREATE VIRTUAL TABLE documents_fts USING fts5(title, content, content='documents', content_rowid='id');
CREATE TRIGGER documents_insert AFTER INSERT ON documents BEGIN
    INSERT INTO documents_fts (rowid, title, content) VALUES (new.id, new.title, new.content);
END;
CREATE TRIGGER documents_delete AFTER DELETE ON documents BEGIN
    INSERT INTO documents_fts (documents_fts, rowid, title, content)
    VALUES ('delete', old.id, old.title, old.content);
END;
CREATE TRIGGER documents_update AFTER UPDATE OF title, content ON documents BEGIN
    INSERT INTO documents_fts (documents_fts, rowid, title, content)
    VALUES ('delete', old.id, old.title, old.content);
    INSERT INTO documents_fts (rowid, title, content) VALUES (new.id, new.title, new.content);
END;

INSERT INTO documents_fts (documents_fts) VALUES ('rebuild');
INSERT INTO documents_fts (documents_fts, rank) VALUES ('integrity-check', 1);
