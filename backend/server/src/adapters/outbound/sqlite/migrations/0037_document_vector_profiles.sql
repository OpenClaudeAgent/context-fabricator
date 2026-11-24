-- Preserve existing bytes and their ledger entries while replacing the primary key.
-- Install the ledger triggers after the copy so each retained row stays charged once.
CREATE TEMP TABLE migrate_document_vectors AS
SELECT document_id, profile, embedding FROM document_vectors;
DROP TRIGGER github_budget_document_vectors_insert;
DROP TRIGGER github_budget_document_vectors_delete;
DROP TRIGGER github_budget_document_vectors_update;
DROP TABLE document_vectors;

CREATE TABLE document_vectors (
    document_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    profile TEXT NOT NULL,
    embedding BLOB NOT NULL CHECK (length(embedding) = 4096),
    PRIMARY KEY (document_id, profile)
) STRICT;
INSERT INTO document_vectors (document_id, profile, embedding)
SELECT document_id, profile, embedding FROM migrate_document_vectors;
DROP TABLE migrate_document_vectors;

CREATE INDEX document_vectors_by_profile ON document_vectors (profile, document_id);

CREATE TRIGGER github_budget_document_vectors_insert AFTER INSERT ON document_vectors
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (CASE WHEN EXISTS (SELECT 1 FROM documents d WHERE d.id=NEW.document_id AND d.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes)) THEN length(NEW.embedding)+length(CAST(NEW.profile AS BLOB)) ELSE 0 END) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_document_vectors_delete AFTER DELETE ON document_vectors
BEGIN UPDATE github_storage_ledger SET bytes = bytes + (-(CASE WHEN EXISTS (SELECT 1 FROM documents d WHERE d.id=OLD.document_id AND d.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes)) THEN length(OLD.embedding)+length(CAST(OLD.profile AS BLOB)) ELSE 0 END)) WHERE family = 'sources'; END;

CREATE TRIGGER github_budget_document_vectors_update AFTER UPDATE ON document_vectors
BEGIN UPDATE github_storage_ledger SET bytes = bytes + ((CASE WHEN EXISTS (SELECT 1 FROM documents d WHERE d.id=NEW.document_id AND d.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes)) THEN length(NEW.embedding)+length(CAST(NEW.profile AS BLOB)) ELSE 0 END)-(CASE WHEN EXISTS (SELECT 1 FROM documents d WHERE d.id=OLD.document_id AND d.collection IN (SELECT collection FROM github_code_scopes UNION SELECT collection FROM github_graph_scopes)) THEN length(OLD.embedding)+length(CAST(OLD.profile AS BLOB)) ELSE 0 END)) WHERE family = 'sources'; END;
