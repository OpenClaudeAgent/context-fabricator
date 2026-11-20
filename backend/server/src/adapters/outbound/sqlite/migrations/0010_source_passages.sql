-- Preserve row identities and their existing embeddings while expanding document identity.
CREATE TEMP TABLE migrate_vectors AS SELECT * FROM document_vectors;
DROP TRIGGER lexical_collection_access;
DROP TRIGGER documents_insert;
DROP TRIGGER documents_delete;
DROP TRIGGER documents_update;
DROP TRIGGER invalidate_document_vector;
DROP VIEW lexical_documents;
DROP TABLE documents_fts;
DROP TABLE document_vectors;

ALTER TABLE documents RENAME TO documents_before_passages;
CREATE TABLE documents (
    id INTEGER PRIMARY KEY,
    collection TEXT NOT NULL REFERENCES collections(name),
    source TEXT NOT NULL,
    revision TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('current', 'history', 'workflow')),
    tracked_ref TEXT NOT NULL DEFAULT '' CHECK (kind = 'current' OR tracked_ref = ''),
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    passage_start INTEGER NOT NULL DEFAULT -1 CHECK (passage_start >= -1),
    source_location TEXT,
    publication_id INTEGER,
    CHECK ((passage_start = -1) = (source_location IS NULL)),
    CHECK ((passage_start = -1) = (publication_id IS NULL)),
    UNIQUE (collection, source, kind, tracked_ref, revision, passage_start)
) STRICT;
INSERT INTO documents (id, collection, source, revision, kind, tracked_ref, title, content)
SELECT id, collection, source, revision, kind, tracked_ref, title, content FROM documents_before_passages;
DROP TABLE documents_before_passages;

CREATE UNIQUE INDEX one_current_revision_per_ref
    ON documents (collection, source, tracked_ref)
    WHERE kind = 'current' AND tracked_ref <> '' AND passage_start = -1;

CREATE TABLE document_vectors (
    document_id INTEGER PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
    profile TEXT NOT NULL,
    embedding BLOB NOT NULL CHECK (length(embedding) = 4096)
) STRICT;

CREATE TRIGGER invalidate_document_vector AFTER UPDATE OF title, content ON documents
WHEN old.title IS NOT new.title OR old.content IS NOT new.content
BEGIN
    DELETE FROM document_vectors WHERE document_id = new.id;
END;

INSERT INTO document_vectors SELECT * FROM migrate_vectors;
DROP TABLE migrate_vectors;

CREATE INDEX documents_by_provenance ON documents (source, revision, kind, tracked_ref, passage_start);

-- Use the same owner preference as evidence resolution. BM25 sees one row per
-- provenance identity, regardless of how many collections own it.
CREATE VIEW lexical_documents AS
    SELECT d.id, d.source, d.revision, d.kind, d.tracked_ref, d.passage_start, d.title, d.content
    FROM documents d
    WHERE d.id = (
        SELECT candidate.id
        FROM documents candidate
        JOIN collections c ON c.name = candidate.collection
        WHERE (candidate.source, candidate.revision, candidate.kind, candidate.tracked_ref, candidate.passage_start)
            = (d.source, d.revision, d.kind, d.tracked_ref, d.passage_start)
        ORDER BY c.access_lost, candidate.id LIMIT 1
    );

CREATE VIRTUAL TABLE documents_fts USING fts5(title, content, content='', contentless_delete=1);

INSERT INTO documents_fts (rowid, title, content)
SELECT id, title, content FROM lexical_documents;
INSERT INTO documents_fts (documents_fts) VALUES ('integrity-check');

CREATE TRIGGER documents_insert AFTER INSERT ON documents
WHEN EXISTS (SELECT 1 FROM lexical_documents WHERE id = new.id)
BEGIN
    DELETE FROM documents_fts WHERE rowid IN (
        SELECT id FROM documents
        WHERE (source, revision, kind, tracked_ref, passage_start) = (new.source, new.revision, new.kind, new.tracked_ref, new.passage_start)
          AND id <> new.id
    );
    INSERT INTO documents_fts (rowid, title, content) VALUES (new.id, new.title, new.content);
END;

CREATE TRIGGER documents_delete AFTER DELETE ON documents
WHEN EXISTS (SELECT 1 FROM documents_fts WHERE rowid = old.id)
BEGIN
    DELETE FROM documents_fts WHERE rowid = old.id;
    INSERT INTO documents_fts (rowid, title, content)
    SELECT id, title, content FROM lexical_documents
    WHERE (source, revision, kind, tracked_ref, passage_start) = (old.source, old.revision, old.kind, old.tracked_ref, old.passage_start);
END;

CREATE TRIGGER documents_update AFTER UPDATE OF title, content ON documents
WHEN EXISTS (SELECT 1 FROM documents_fts WHERE rowid = new.id)
BEGIN
    DELETE FROM documents_fts WHERE rowid = old.id;
    INSERT INTO documents_fts (rowid, title, content) VALUES (new.id, new.title, new.content);
END;

CREATE TRIGGER lexical_collection_access AFTER UPDATE OF access_lost ON collections
WHEN new.access_lost <> old.access_lost
BEGIN
    DELETE FROM documents_fts WHERE rowid IN (
        SELECT d.id
        FROM documents affected
        JOIN documents d USING (source, revision, kind, tracked_ref, passage_start)
        WHERE affected.collection = new.name
          AND NOT EXISTS (SELECT 1 FROM lexical_documents WHERE id = d.id)
    );
    INSERT INTO documents_fts (rowid, title, content)
    SELECT d.id, d.title, d.content
    FROM documents affected
    JOIN lexical_documents d USING (source, revision, kind, tracked_ref, passage_start)
    WHERE affected.collection = new.name
      AND NOT EXISTS (SELECT 1 FROM documents_fts WHERE rowid = d.id);
END;
