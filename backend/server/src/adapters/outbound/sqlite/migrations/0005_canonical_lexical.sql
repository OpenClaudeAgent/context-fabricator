DROP TRIGGER documents_insert;
DROP TRIGGER documents_delete;
DROP TRIGGER documents_update;
DROP TABLE documents_fts;

CREATE INDEX documents_by_provenance ON documents (source, revision, kind, tracked_ref);

-- Use the same owner preference as evidence resolution. BM25 sees one row per
-- provenance identity, regardless of how many collections own it.
CREATE VIEW lexical_documents AS
    SELECT d.id, d.source, d.revision, d.kind, d.tracked_ref, d.title, d.content
    FROM documents d
    WHERE d.id = (
        SELECT candidate.id
        FROM documents candidate
        JOIN collections c ON c.name = candidate.collection
        WHERE (candidate.source, candidate.revision, candidate.kind, candidate.tracked_ref)
            = (d.source, d.revision, d.kind, d.tracked_ref)
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
        WHERE (source, revision, kind, tracked_ref) = (new.source, new.revision, new.kind, new.tracked_ref)
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
    WHERE (source, revision, kind, tracked_ref) = (old.source, old.revision, old.kind, old.tracked_ref);
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
        JOIN documents d USING (source, revision, kind, tracked_ref)
        WHERE affected.collection = new.name
          AND NOT EXISTS (SELECT 1 FROM lexical_documents WHERE id = d.id)
    );
    INSERT INTO documents_fts (rowid, title, content)
    SELECT d.id, d.title, d.content
    FROM documents affected
    JOIN lexical_documents d USING (source, revision, kind, tracked_ref)
    WHERE affected.collection = new.name
      AND NOT EXISTS (SELECT 1 FROM documents_fts WHERE rowid = d.id);
END;
