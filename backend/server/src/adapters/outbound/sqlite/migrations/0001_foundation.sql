CREATE TABLE settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0),
    answer_language TEXT NOT NULL DEFAULT 'auto' CHECK (answer_language IN ('auto', 'en', 'fr')),
    diagnostic_retention_hours INTEGER NOT NULL DEFAULT 24 CHECK (diagnostic_retention_hours BETWEEN 1 AND 168)
) STRICT;
INSERT INTO settings (id) VALUES (1);

CREATE TABLE diagnostics (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp INTEGER NOT NULL,
    code TEXT NOT NULL,
    message TEXT NOT NULL
) STRICT;

CREATE TABLE collections (
    name TEXT PRIMARY KEY,
    generation INTEGER NOT NULL DEFAULT 0,
    cursor TEXT
) STRICT;

CREATE TABLE documents (
    id INTEGER PRIMARY KEY,
    collection TEXT NOT NULL REFERENCES collections(name),
    source TEXT NOT NULL,
    revision TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('current', 'history', 'workflow')),
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    UNIQUE (collection, source, revision, kind)
) STRICT;
CREATE VIRTUAL TABLE documents_fts USING fts5(title, content, content='documents', content_rowid='id');
CREATE TRIGGER documents_insert AFTER INSERT ON documents BEGIN
    INSERT INTO documents_fts (rowid, title, content) VALUES (new.id, new.title, new.content);
END;
CREATE TRIGGER documents_delete AFTER DELETE ON documents BEGIN
    INSERT INTO documents_fts (documents_fts, rowid, title, content)
    VALUES ('delete', old.id, old.title, old.content);
END;
CREATE TRIGGER documents_update AFTER UPDATE ON documents BEGIN
    INSERT INTO documents_fts (documents_fts, rowid, title, content)
    VALUES ('delete', old.id, old.title, old.content);
    INSERT INTO documents_fts (rowid, title, content) VALUES (new.id, new.title, new.content);
END;
