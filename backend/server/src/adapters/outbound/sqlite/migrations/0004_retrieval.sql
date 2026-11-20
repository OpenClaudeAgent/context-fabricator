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
